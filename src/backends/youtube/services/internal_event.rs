#[derive(Debug, Clone, PartialEq)]
pub enum InternalEvent {
    TrackChanged { position: i32 },
    IdleChanged { idle: bool },
    EndFile { reason: String },
    PlaybackStarted,
}
