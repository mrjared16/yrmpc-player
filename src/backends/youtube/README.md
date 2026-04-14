# YouTube Backend

YouTube Music integration for yrmpc.

## Module Structure

```
rmpc/src/backends/youtube/
├── mod.rs                  # Module entry, YoutubeBackend
├── api.rs                  # API trait implementations
├── client.rs               # YouTube Music client wrapper
├── config.rs               # Backend configuration
├── details.rs              # Song/album/artist details fetching
├── error.rs                # Error types
├── url_resolver.rs         # Video ID → stream URL resolution
│
├── extractor/              # URL extraction (ytx/yt-dlp)
│   ├── mod.rs
│   ├── cached.rs           # URL caching layer
│   ├── fallback.rs         # ytx → yt-dlp fallback chain
│   ├── ytx.rs              # Fast Rust extractor (~200ms)
│   └── ytdlp.rs            # Python fallback (~4s)
│
├── mpv/                    # MPV player integration
│   ├── mod.rs
│   └── ipc.rs              # JSON IPC over Unix socket
│
├── server/                 # Backend daemon
│   ├── mod.rs
│   ├── orchestrator.rs     # Prefetch coordination
│   └── handlers/           # Command handlers
│       ├── mod.rs
│       ├── playback.rs     # Play/pause/seek
│       ├── queue.rs        # Queue manipulation
│       ├── queue_events.rs # Queue change events
│       ├── search.rs       # YouTube Music search
│       ├── status.rs       # Playback status
│       └── options.rs      # Repeat/shuffle modes
│
├── services/               # Core services
│   ├── mod.rs
│   ├── api_service.rs      # YouTube Music API calls
│   ├── playback_service.rs # Playback control
│   ├── playback_state.rs   # State machine
│   ├── queue_service.rs    # Queue management
│   ├── audio_prefetcher.rs # Audio prefetch (being replaced)
│   └── internal_event.rs   # Internal event types
│
└── audio/                  # Audio streaming (NEW - see below)
    ├── mod.rs              # Module exports
    ├── cache.rs            # AudioCache (prefix files)
    ├── mpv_source.rs       # MpvAudioSource trait
    ├── range_set.rs        # Byte range tracking
    └── sources/
        ├── mod.rs
        ├── concat.rs       # ConcatSource (DEFAULT)
        └── proxy/          # ProxySource (FUTURE)
```

## Audio Streaming (audio/)

### Architecture

The audio module uses a **Strategy Pattern** for pluggable audio sources:

```
┌─────────────────────────────────────────────────────────┐
│                   MpvAudioSource (Trait)                │
│                                                         │
│  ┌─────────────────────────┐  ┌───────────────────────┐│
│  │ ConcatSource (DEFAULT)  │  │ ProxySource (FUTURE)  ││
│  │ - concat+subfile proto  │  │ - HTTP server         ││
│  │ - Byte-perfect playback │  │ - Offline mode        ││
│  │ - Stateless, ~50 lines  │  │ - Metrics, URL refresh││
│  └─────────────────────────┘  └───────────────────────┘│
│                       │                                 │
│                       ▼                                 │
│              ┌─────────────────┐                        │
│              │   AudioCache    │                        │
│              │ (Shared)        │                        │
│              │ - Prefix files  │                        │
│              │ - LRU eviction  │                        │
│              └─────────────────┘                        │
└─────────────────────────────────────────────────────────┘
```

### Components

#### MpvAudioSource Trait

Defines the interface for audio source strategies:

```rust
pub struct MpvInput {
    pub url: String,
    pub mpv_args: Vec<String>,
}

pub trait MpvAudioSource: Send {
    fn startup(&mut self) -> anyhow::Result<()> { Ok(()) }
    fn shutdown(&mut self) {}
    fn build_mpv_input(&mut self, video_id: &str) -> anyhow::Result<MpvInput>;
}
```

#### ConcatSource (DEFAULT)

Uses ffmpeg's concat+subfile protocol for byte-perfect playback:

```
concat:/cache/{video_id}.m4a|subfile,,start,{BYTE_OFFSET},end,0,,:${YOUTUBE_URL}
       └── Cached prefix ──┘  └── Stream remainder at byte offset ──┘
```

**Why concat+subfile over EDL**:
- EDL uses TIME offsets → 5-50ms audible gap at junction
- concat+subfile uses BYTE offsets → byte-perfect (verified via PCM MD5)

Required MPV args:
```
--demuxer-lavf-o=protocol_whitelist=file,http,https,tcp,tls,crypto,subfile,concat
```

#### AudioCache

Manages prefix files for instant playback start:

| Property | Value |
|----------|-------|
| Location | `~/.cache/rmpc/audio/{video_id}.m4a` |
| Prefix size | ~200KB per song |
| Budget | <200MB total |
| Eviction | LRU when budget exceeded |

**Key API**:
- `ensure_prefix(video_id)` → Downloads prefix if not cached, returns path
- `get_content_length(video_id)` → Returns total file size for byte offset

### Key Decision: concat+subfile over EDL

See [ADR-001](../../../docs/adr/ADR-001-audio-streaming-architecture.md) for full rationale.

**Summary**: EDL's time-based offsets don't align with audio frame boundaries, causing audible gaps. The concat+subfile protocol uses byte offsets, achieving byte-perfect playback verified via PCM MD5 comparison.

## Services

### PlaybackService

Handles playback commands (play, pause, seek, stop). Integrates with MpvAudioSource to build URLs for MPV.

**Key methods**:
- `handle_play(song)` → Calls `MpvAudioSource::build_mpv_input()`, sends to MPV
- `handle_seek(position)` → Seeks within current track
- `handle_stop()` → Stops playback

### QueueService

Manages the play queue state. Handles add, remove, reorder, and clear operations.

**Key methods**:
- `add_songs(songs, position)` → Add songs to queue
- `remove_songs(indices)` → Remove songs from queue
- `move_song(from, to)` → Reorder queue

### UrlResolver

Resolves video IDs to stream URLs using configured extractor policy:
- `primary=ytx, fallback=true` → `ytx → yt-dlp`
- `primary=ytdlp, fallback=true` → `yt-dlp → ytx`
- `fallback=false` → primary only

**Caching**: URLs are cached with TTL to avoid repeated extraction calls.

## Configuration

```toml
# ~/.config/rmpc/youtube.toml
[api.extractor]
primary = "ytx"   # default
fallback = true    # default
```

Legacy fallback path (still supported): `~/.config/yrmpc/youtube.toml`

## Debugging

| Symptom | Likely Cause | Check |
|---------|--------------|-------|
| Song won't start | URL extraction failed | `RUST_LOG=debug`, check ytx/yt-dlp |
| Long delay | ytx failed, using yt-dlp | Verify ytx installed |
| "No cookies found" | Auth issue | Check `~/.config/rmpc/cookie.txt` |
| Audio gap at start | Prefix not cached | Check AudioCache logs |
| "protocol_whitelist" | Missing MPV args | Verify MpvInput.mpv_args |

## See Also

- [ADR-001](../../../docs/adr/ADR-001-audio-streaming-architecture.md) - Architecture decision
- [audio-streaming.md](../../../docs/arch/audio-streaming.md) - Detailed audio architecture
- [playback-engine.md](../../../docs/arch/playback-engine.md) - Overall engine
- [playback-flow.md](../../../docs/arch/playback-flow.md) - End-to-end flow
