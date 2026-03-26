# AGENTS.md - rmpc crate guide

This file provides **local overrides for the `rmpc/` submodule**.

**Inherit all repository-wide rules from** `../AGENTS.md`.
If this file conflicts with the parent file, prefer the more specific rule here for work inside `rmpc/`.

---

## Scope

`rmpc/` is the main Rust crate for yrmpc.
Run cargo commands here, keep paths relative to this directory when possible, and treat the parent repo docs as the source of higher-level architecture and workflow guidance.

---

## Quick Start

```bash
cargo build
../restart_daemon_debug.sh
./target/debug/rmpc --config ../config/rmpc.ron
```

**Verify changes**:

```bash
cargo fmt
cargo clippy
cargo nextest run
```

> Use debug builds for normal development. Only use `--release` after the user has verified behavior and you need release-only validation.

---

## Critical Cargo Rules

- `rmpc/` **is** the cargo root for the main application.
- Run cargo commands from this directory, not from the parent repo root.
- Prefer `cargo nextest run` over `cargo test` for normal verification.

Examples:

```bash
cargo build
cargo clippy
cargo nextest run
```

---

## Key Local Paths

| Purpose | Path |
|---------|------|
| UI controller | `src/ui/panes/navigator.rs` |
| Dispatcher | `src/backends/dispatcher.rs` |
| YouTube backend | `src/backends/youtube/` |
| Playback orchestrator | `src/backends/youtube/server/orchestrator.rs` |
| Playback coordinator | `src/backends/youtube/server/playback_coordinator.rs` |
| Queue state machine | `src/shared/play_queue/mod.rs` |
| Dev config from parent repo | `../config/rmpc.ron` |

---

## Playback / Queue Notes

- The current playback contract is documented in `../docs/arch/playback-flow.md`.
- `PlaybackCoordinator` owns in-flight current-track identity during immediate startup.
- `PlayQueue.current_id` is advisory until playback is confirmed.
- Stale `TrackChanged(-1)` during teardown must not restore the previous track.
- Queue mutation reconciliation is delta-based; preserve unchanged future entries.

---

## Runtime / Manual Verification

- Backend code changes usually require restarting the daemon via `../restart_daemon_debug.sh`.
- UI-only changes usually do not require daemon restart.
- For YouTube playback regressions, inspect `/tmp/rmpcd.log` and validate:
  - extraction cache hits vs misses
  - relay session behavior
  - prefix cache hits / tee-prefix promotion
  - current-track ownership during immediate play

---

## Don’ts

- Don’t reintroduce old `backlog` workflow instructions; this repo uses `br` from the parent project guidance.
- Don’t run cargo from the parent repo root.
- Don’t treat `MusicBackend` as the preferred abstraction; use the newer API traits documented in `../AGENTS.md`.
- Don’t assume old playback docs are canonical; prefer `../docs/arch/playback-flow.md`.
