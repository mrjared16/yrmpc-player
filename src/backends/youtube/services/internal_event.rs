#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InternalEvent {
    TrackChanged { position: i32 },
    IdleChanged { idle: bool },
    EndFile { reason: String },
}
