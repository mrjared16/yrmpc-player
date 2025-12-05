//! YouTube backend client - connects to server via IPC.
//! Implements MusicBackend trait for use in TUI.

use std::{
    collections::HashMap,
    io::{BufReader, BufWriter},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};

use super::protocol::{BrowseEntry, ServerCommand, ServerResponse, SongData, framing};
use crate::{
    domain::{PlaybackState, QueuePosition, Song, Status},
    mpd::{
        commands::{
            Decoder, LsInfoEntry, Output, Playlist, SaveMode, SeekPosition, ValueChange,
            lsinfo::Dir,
            status::OnOffOneshot,
        },
        mpd_client::{Filter, SingleOrRange, Tag},
        version::Version,
    },
    player::backend::MusicBackend,
};

/// Client that connects to YouTube server
#[derive(Debug)]
pub struct YouTubeClient {
    reader: BufReader<UnixStream>,
    writer: BufWriter<UnixStream>,
}

impl YouTubeClient {
    /// Connect to server at given socket path
    pub fn connect(socket_path: &Path) -> Result<Self> {
        // Try to connect to existing daemon
        let stream = match UnixStream::connect(socket_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("\n❌ YouTube daemon not running!\n");
                eprintln!("Start the daemon first:");
                eprintln!("  systemctl --user start rmpcd");
                eprintln!("");
                eprintln!("Or install as a service:");
                eprintln!("  ./setup/rmpcd-install");
                eprintln!("");
                eprintln!("Or run manually:");
                eprintln!("  rmpcd");
                eprintln!("");
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        };

        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;

        log::info!("Connected to YouTube daemon at {}", socket_path.display());

        Ok(Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: BufWriter::new(stream),
        })
    }

    /// Send command and receive response
    fn request(&mut self, cmd: ServerCommand) -> Result<ServerResponse> {
        framing::write_message(&mut self.writer, &cmd)?;
        framing::read_message(&mut self.reader)
    }

    /// Send command and expect Ok response
    fn request_ok(&mut self, cmd: ServerCommand) -> Result<()> {
        match self.request(cmd)? {
            ServerResponse::Ok => Ok(()),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    /// Ping the server
    pub fn ping(&mut self) -> Result<()> {
        match self.request(ServerCommand::Ping)? {
            ServerResponse::Pong => Ok(()),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    /// Shutdown the server
    pub fn shutdown(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Shutdown)
    }

    /// Try to clone the underlying stream (for idle connections)
    pub fn try_clone_stream(&self) -> Result<UnixStream> {
        // We need to clone from either reader or writer
        // Since BufReader/BufWriter don't expose the stream, we'll return an error for now
        // This is used for idle connections which YouTube client handles differently
        anyhow::bail!("Stream cloning not supported for YouTube client")
    }

    /// Enter idle mode (no-op for YouTube, handled by server)
    pub fn enter_idle(&mut self) -> Result<()> {
        // YouTube daemon handles idle internally
        Ok(())
    }

    /// Read idle response
    pub fn read_response(&mut self) -> Result<Vec<crate::mpd::commands::IdleEvent>> {
        // YouTube daemon doesn't use MPD's idle protocol
        Ok(vec![])
    }

    /// Reconnect to server
    pub fn reconnect(&mut self) -> Result<()> {
        // TODO: Implement reconnection logic
        anyhow::bail!("Reconnection not yet implemented for YouTube client")
    }
}

impl MusicBackend for YouTubeClient {
    fn backend_name(&self) -> &'static str {
        "YouTube"
    }

    // === Playback Control ===

    fn play(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Play)
    }

    fn pause(&mut self, state: bool) -> Result<()> {
        if state {
            self.request_ok(ServerCommand::Pause)
        } else {
            self.request_ok(ServerCommand::Play)
        }
    }

    fn stop(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Stop)
    }

    fn next(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Next)
    }

    fn previous(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Previous)
    }

    fn seek_current(&mut self, position: SeekPosition) -> Result<()> {
        match position {
            SeekPosition::Absolute(secs) => {
                self.request_ok(ServerCommand::SeekAbsolute(secs as f64))
            }
            SeekPosition::Relative(secs) => {
                self.request_ok(ServerCommand::SeekRelative(secs as f64))
            }
        }
    }

    // === Status ===

    fn get_status(&mut self) -> Result<Status> {
        match self.request(ServerCommand::GetStatus)? {
            ServerResponse::Status(s) => Ok(s.to_status()),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn current_song(&mut self) -> Result<Option<Song>> {
        match self.request(ServerCommand::GetCurrentSong)? {
            ServerResponse::Song(s) => Ok(s.map(|sd| sd.to_song())),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn playlist_info(&mut self) -> Result<Vec<Song>> {
        match self.request(ServerCommand::GetPlaylist)? {
            ServerResponse::Playlist(songs) => Ok(songs.into_iter().map(|sd| sd.to_song()).collect()),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    // === Queue Management ===

    fn add(&mut self, uri: &str, position: Option<QueuePosition>) -> Result<()> {
        let pos = position.map(|p| match p {
            QueuePosition::Absolute(n) => n as u32,
            QueuePosition::Relative(n) => n as u32,
            QueuePosition::End => u32::MAX, // Append to end
            QueuePosition::Next => 0,       // After current
        });
        self.request_ok(ServerCommand::Add { uri: uri.to_string(), position: pos })
    }

    fn delete_id(&mut self, id: u32) -> Result<()> {
        self.request_ok(ServerCommand::DeleteId(id))
    }

    fn clear(&mut self) -> Result<()> {
        self.request_ok(ServerCommand::Clear)
    }

    fn move_id(&mut self, from: u32, to: u32) -> Result<()> {
        self.request_ok(ServerCommand::MoveId { from, to })
    }

    fn play_id(&mut self, id: u32) -> Result<()> {
        self.request_ok(ServerCommand::PlayId(id))
    }

    // === Volume ===

    fn volume(&mut self) -> Result<u8> {
        match self.request(ServerCommand::GetVolume)? {
            ServerResponse::Volume(v) => Ok(v),
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn set_volume(&mut self, volume: ValueChange) -> Result<()> {
        match volume {
            ValueChange::Set(v) => self.request_ok(ServerCommand::SetVolume(v as u8)),
            ValueChange::Increase(d) => self.request_ok(ServerCommand::AdjustVolume(d as i8)),
            ValueChange::Decrease(d) => self.request_ok(ServerCommand::AdjustVolume(-(d as i8))),
        }
    }

    // === Search/Browse ===

    fn search(&mut self, filter: &[Filter]) -> Result<Vec<Song>> {
        let query = filter
            .iter()
            .find_map(|f| if !f.value.is_empty() { Some(f.value.as_ref()) } else { None })
            .unwrap_or("");

        if query.is_empty() {
            return Ok(vec![]);
        }

        match self.request(ServerCommand::Search { query: query.to_string() })? {
            ServerResponse::SearchResults(songs) => {
                Ok(songs.into_iter().map(|sd| sd.to_song()).collect())
            }
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    fn find(&mut self, filter: &[Filter], _window: Option<(u32, u32)>) -> Result<Vec<Song>> {
        self.search(filter)
    }

    fn lsinfo(&mut self, path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        let path = path.unwrap_or("");
        if path.is_empty() {
            return Ok(vec![]);
        }

        match self.request(ServerCommand::Browse { path: path.to_string() })? {
            ServerResponse::BrowseResults(entries) => {
                Ok(entries
                    .into_iter()
                    .map(|e| match e {
                        BrowseEntry::Dir { name, path } => LsInfoEntry::Dir(Dir {
                            name,
                            full_path: path,
                            last_modified: chrono::Utc::now(),
                        }),
                        BrowseEntry::File(sd) => LsInfoEntry::File(sd.to_song()),
                    })
                    .collect())
            }
            ServerResponse::Error(e) => Err(anyhow!(e)),
            other => Err(anyhow!("Unexpected response: {:?}", other)),
        }
    }

    // === Playback Options (stubs) ===

    fn repeat(&mut self, _repeat: bool) -> Result<()> {
        Ok(())
    }

    fn random(&mut self, _random: bool) -> Result<()> {
        Ok(())
    }

    fn single(&mut self, _single: OnOffOneshot) -> Result<()> {
        Ok(())
    }

    fn consume(&mut self, _consume: OnOffOneshot) -> Result<()> {
        Ok(())
    }

    fn crossfade(&mut self, _seconds: u32) -> Result<()> {
        Ok(())
    }

    fn shuffle(&mut self, _range: Option<SingleOrRange>) -> Result<()> {
        Ok(())
    }

    // === Library (stubs) ===

    fn list_all(&mut self, _path: Option<&str>) -> Result<Vec<LsInfoEntry>> {
        Ok(vec![])
    }

    fn list_tag(&mut self, _tag: Tag, _filter: Option<&[Filter]>) -> Result<Vec<String>> {
        Ok(vec![])
    }

    fn count(&mut self, _filter: &[Filter]) -> Result<(usize, Duration)> {
        Ok((0, Duration::ZERO))
    }

    fn get_library(
        &mut self,
        _category: crate::player::LibraryCategory,
    ) -> Result<Vec<LsInfoEntry>> {
        Ok(vec![])
    }

    fn get_search_suggestions(&mut self, _query: String) -> Result<Vec<String>> {
        Ok(vec![])
    }

    // === Playlist Management (stubs) ===

    fn list_playlists(&mut self) -> Result<Vec<Playlist>> {
        Ok(vec![])
    }

    fn playlist_info_name(&mut self, _name: &str) -> Result<Vec<Song>> {
        Ok(vec![])
    }

    fn load_playlist(&mut self, _name: &str, _position: Option<QueuePosition>) -> Result<()> {
        Ok(())
    }

    fn save_queue_as_playlist(&mut self, _name: &str, _mode: Option<SaveMode>) -> Result<()> {
        Ok(())
    }

    fn delete_playlist(&mut self, _name: &str) -> Result<()> {
        Ok(())
    }

    fn rename_playlist(&mut self, _old: &str, _new: &str) -> Result<()> {
        Ok(())
    }

    fn add_to_playlist(&mut self, _playlist: &str, _uri: &str) -> Result<()> {
        Ok(())
    }

    fn delete_from_playlist(&mut self, _playlist: &str, _position: u32) -> Result<()> {
        Ok(())
    }

    fn move_in_playlist(&mut self, _playlist: &str, _from: SingleOrRange, _to: u32) -> Result<()> {
        Ok(())
    }

    // === Stickers (stubs) ===

    fn list_stickers(&mut self, _uri: &str) -> Result<HashMap<String, String>> {
        Ok(HashMap::new())
    }

    fn set_sticker(&mut self, _uri: &str, _key: &str, _value: &str) -> Result<()> {
        Ok(())
    }

    fn delete_sticker(&mut self, _uri: &str, _key: &str) -> Result<()> {
        Ok(())
    }

    // === Database (stubs) ===

    fn update(&mut self, _path: Option<&str>) -> Result<u32> {
        Ok(0)
    }

    fn rescan(&mut self, _path: Option<&str>) -> Result<u32> {
        Ok(0)
    }

    // === System Info ===

    fn version(&self) -> Version {
        Version { major: 0, minor: 1, patch: 0 }
    }

    fn outputs(&mut self) -> Result<Vec<Output>> {
        Ok(vec![])
    }

    fn decoders(&mut self) -> Result<Vec<Decoder>> {
        Ok(vec![])
    }

    fn partitions(&mut self) -> Result<Vec<String>> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    // Integration tests would start a server and connect client
    // For unit tests, we'd mock the socket connection
}
