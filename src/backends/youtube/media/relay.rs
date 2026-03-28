//! Relay transport contract on top of the shared preparation core.
//!
//! The preparation boundary stays transport-neutral: `PreparedMedia` describes
//! staged local bytes plus an upstream URL. The Relay boundary turns that into
//! one localhost-facing session URL without changing the prepared artifact.
//!
//! Contract summary for Task 9+:
//! - input contract: `PreparedMedia::StagedPrefix`
//! - player URL shape: `http://127.0.0.1:<port>/relay/sessions/{session_id}/stream`
//! - range contract: full-body or single byte range only; multi-range is
//!   rejected before streaming starts
//! - reconnect ownership: relay owns upstream reconnect/refresh once a session
//!   URL is minted; the player should not reconnect to the upstream URL
//! - lifecycle: `Registered -> AwaitingRequest -> Streaming -> terminal`
//!
//! Error semantics:
//! - invalid prepared inputs fail before session registration with
//!   `RelayContractError`
//! - invalid or unsatisfiable request ranges fail before the response body with
//!   `RelayRangeError` (future runtime maps these to HTTP 416)
//! - terminal sessions should reject later requests via
//!   `RelaySessionState::terminal_http_status()`

use std::{net::SocketAddr, path::PathBuf};

use super::PreparedMedia;

pub const RELAY_SESSION_PATH_TEMPLATE: &str = "/relay/sessions/{session_id}/stream";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayTransportContract {
    pub url_path_template: &'static str,
    pub range_policy: RelayRangePolicy,
    pub reconnect_owner: RelayReconnectOwner,
}

impl Default for RelayTransportContract {
    fn default() -> Self {
        Self {
            url_path_template: RELAY_SESSION_PATH_TEMPLATE,
            range_policy: RelayRangePolicy::SingleRangeOrFull,
            reconnect_owner: RelayReconnectOwner::RelaySession,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayRangePolicy {
    SingleRangeOrFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayReconnectOwner {
    RelaySession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelaySessionState {
    Registered,
    AwaitingRequest,
    Streaming,
    Completed,
    Cancelled,
    Failed,
    Expired,
}

impl RelaySessionState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed | Self::Expired)
    }

    pub fn terminal_http_status(self) -> Option<u16> {
        match self {
            Self::Completed | Self::Cancelled | Self::Expired => Some(410),
            Self::Failed => Some(502),
            Self::Registered | Self::AwaitingRequest | Self::Streaming => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayContractError {
    EmptyTrackId,
    PreparedMediaMissingStagedPrefix,
    EmptyUpstreamUrl,
    EmptyPrefixArtifact,
    PrefixLongerThanContent,
    InvalidSessionId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayRangeError {
    InvalidRangeHeader,
    MultipleRangesUnsupported,
    Unsatisfiable,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RelaySessionId(String);

impl RelaySessionId {
    pub fn new(value: impl Into<String>) -> Result<Self, RelayContractError> {
        let value = value.into();

        if value.is_empty()
            || !value.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        {
            return Err(RelayContractError::InvalidSessionId);
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayPlayerEndpoint {
    listen_addr: SocketAddr,
    session_id: RelaySessionId,
}

impl RelayPlayerEndpoint {
    pub fn new(listen_addr: SocketAddr, session_id: RelaySessionId) -> Self {
        Self { listen_addr, session_id }
    }

    pub fn path(&self) -> String {
        format!("/relay/sessions/{}/stream", self.session_id.as_str())
    }

    pub fn url(&self) -> String {
        format!("http://{}{}", self.listen_addr, self.path())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayByteRange {
    pub start: u64,
    pub end: u64,
}

impl RelayByteRange {
    pub fn new(start: u64, end: u64) -> Result<Self, RelayRangeError> {
        if start >= end {
            return Err(RelayRangeError::InvalidRangeHeader);
        }

        Ok(Self { start, end })
    }

    pub fn len(self) -> u64 {
        self.end - self.start
    }

    pub fn is_full_object(self, content_length: u64) -> bool {
        self.start == 0 && self.end == content_length
    }

    pub fn to_content_range_value(self, content_length: u64) -> String {
        format!("bytes {}-{}/{}", self.start, self.end - 1, content_length)
    }

    fn intersection(self, other: Self) -> Option<Self> {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        (start < end).then_some(Self { start, end })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayStagedArtifact {
    pub path: PathBuf,
    pub available: RelayByteRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayUpstreamStream {
    pub url: String,
    pub content_length: u64,
}

/// When set, the relay opens the upstream from byte 0 and writes the first
/// `size` bytes to `path` as a side-effect (tee / multiplexer mode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayTeePrefix {
    pub path: PathBuf,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaySessionSpec {
    pub track_id: String,
    pub staged: RelayStagedArtifact,
    pub upstream: RelayUpstreamStream,
    pub contract: RelayTransportContract,
    pub state: RelaySessionState,
    /// If set, the relay streams from byte 0 and saves the first N bytes to
    /// disk.  The `staged` artifact is empty in this mode — everything comes
    /// from upstream.
    pub tee_prefix: Option<RelayTeePrefix>,
}

impl RelaySessionSpec {
    pub fn try_from_prepared(
        track_id: impl Into<String>,
        prepared: &PreparedMedia,
    ) -> Result<Self, RelayContractError> {
        let track_id = track_id.into();
        if track_id.is_empty() {
            return Err(RelayContractError::EmptyTrackId);
        }

        match prepared {
            PreparedMedia::StagedPrefix { path, bytes, url, content_length } => {
                if url.is_empty() {
                    return Err(RelayContractError::EmptyUpstreamUrl);
                }
                if *bytes == 0 || *content_length == 0 {
                    return Err(RelayContractError::EmptyPrefixArtifact);
                }
                if *bytes > *content_length {
                    return Err(RelayContractError::PrefixLongerThanContent);
                }
                Ok(Self {
                    track_id,
                    staged: RelayStagedArtifact {
                        path: path.clone(),
                        available: RelayByteRange { start: 0, end: *bytes },
                    },
                    upstream: RelayUpstreamStream {
                        url: url.clone(),
                        content_length: *content_length,
                    },
                    contract: RelayTransportContract::default(),
                    state: RelaySessionState::Registered,
                    tee_prefix: None,
                })
            }
            PreparedMedia::StreamAndCache { url, content_length, prefix_path, prefix_size } => {
                if url.is_empty() {
                    return Err(RelayContractError::EmptyUpstreamUrl);
                }
                if *content_length == 0 {
                    return Err(RelayContractError::EmptyPrefixArtifact);
                }
                Ok(Self {
                    track_id,
                    staged: RelayStagedArtifact {
                        path: prefix_path.clone(),
                        available: RelayByteRange { start: 0, end: 0 }, // no staged data yet
                    },
                    upstream: RelayUpstreamStream {
                        url: url.clone(),
                        content_length: *content_length,
                    },
                    contract: RelayTransportContract::default(),
                    state: RelaySessionState::Registered,
                    tee_prefix: Some(RelayTeePrefix {
                        path: prefix_path.clone(),
                        size: (*prefix_size).min(*content_length),
                    }),
                })
            }
            _ => Err(RelayContractError::PreparedMediaMissingStagedPrefix),
        }
    }

    pub fn player_endpoint(
        &self,
        listen_addr: SocketAddr,
        session_id: RelaySessionId,
    ) -> RelayPlayerEndpoint {
        RelayPlayerEndpoint::new(listen_addr, session_id)
    }

    pub fn plan_response(
        &self,
        range_header: Option<&str>,
    ) -> Result<RelayResponsePlan, RelayRangeError> {
        let response =
            RequestedRange::parse(range_header)?.resolve(self.upstream.content_length)?;
        let staged = self.staged.available.intersection(response);
        let upstream_start = response.start.max(self.staged.available.end);
        let upstream = (upstream_start < response.end)
            .then_some(RelayByteRange { start: upstream_start, end: response.end });

        Ok(RelayResponsePlan {
            response,
            staged,
            upstream,
            content_length: self.upstream.content_length,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayResponsePlan {
    pub response: RelayByteRange,
    pub staged: Option<RelayByteRange>,
    pub upstream: Option<RelayByteRange>,
    pub content_length: u64,
}

impl RelayResponsePlan {
    pub fn is_partial_content(&self) -> bool {
        !self.response.is_full_object(self.content_length)
    }

    pub fn status_code(&self) -> u16 {
        if self.is_partial_content() { 206 } else { 200 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestedRange {
    Full,
    From(u64),
    Inclusive { start: u64, end: u64 },
    Suffix(u64),
}

impl RequestedRange {
    fn parse(header: Option<&str>) -> Result<Self, RelayRangeError> {
        let Some(header) = header else {
            return Ok(Self::Full);
        };

        let header = header.trim();
        let spec = header.strip_prefix("bytes=").ok_or(RelayRangeError::InvalidRangeHeader)?;

        if spec.contains(',') {
            return Err(RelayRangeError::MultipleRangesUnsupported);
        }

        let (start, end) = spec.split_once('-').ok_or(RelayRangeError::InvalidRangeHeader)?;

        if start.is_empty() {
            let suffix = end.parse::<u64>().map_err(|_| RelayRangeError::InvalidRangeHeader)?;
            if suffix == 0 {
                return Err(RelayRangeError::InvalidRangeHeader);
            }
            return Ok(Self::Suffix(suffix));
        }

        let start = start.parse::<u64>().map_err(|_| RelayRangeError::InvalidRangeHeader)?;
        if end.is_empty() {
            return Ok(Self::From(start));
        }

        let end = end.parse::<u64>().map_err(|_| RelayRangeError::InvalidRangeHeader)?;
        if end < start {
            return Err(RelayRangeError::InvalidRangeHeader);
        }

        Ok(Self::Inclusive { start, end })
    }

    fn resolve(self, content_length: u64) -> Result<RelayByteRange, RelayRangeError> {
        match self {
            Self::Full => RelayByteRange::new(0, content_length),
            Self::From(start) => {
                if start >= content_length {
                    return Err(RelayRangeError::Unsatisfiable);
                }
                RelayByteRange::new(start, content_length)
            }
            Self::Inclusive { start, end } => {
                if start >= content_length {
                    return Err(RelayRangeError::Unsatisfiable);
                }

                let end = end.saturating_add(1).min(content_length);
                RelayByteRange::new(start, end)
            }
            Self::Suffix(length) => {
                if length == 0 {
                    return Err(RelayRangeError::InvalidRangeHeader);
                }

                let start = content_length.saturating_sub(length.min(content_length));
                RelayByteRange::new(start, content_length)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        net::{IpAddr, Ipv4Addr, SocketAddr},
        path::PathBuf,
    };

    use super::{
        RelayByteRange, RelayContractError, RelayPlayerEndpoint, RelayRangeError, RelaySessionId,
        RelaySessionSpec, RelaySessionState, RelayTransportContract,
    };
    use crate::backends::youtube::media::PreparedMedia;

    #[test]
    fn relay_contract_maps_staged_prefix_into_localhost_session() {
        let prepared = PreparedMedia::StagedPrefix {
            path: PathBuf::from("/tmp/prefix.webm"),
            bytes: 2048,
            url: "https://example.com/upstream".to_string(),
            content_length: 4096,
        };

        let session = RelaySessionSpec::try_from_prepared("track-123", &prepared).unwrap();

        assert_eq!(session.track_id, "track-123");
        assert_eq!(session.staged.path, PathBuf::from("/tmp/prefix.webm"));
        assert_eq!(session.staged.available, RelayByteRange::new(0, 2048).unwrap());
        assert_eq!(session.upstream.url, "https://example.com/upstream");
        assert_eq!(session.upstream.content_length, 4096);
        assert_eq!(session.contract, RelayTransportContract::default());
        assert_eq!(session.state, RelaySessionState::Registered);
    }

    #[test]
    fn relay_contract_maps_stream_and_cache_into_tee_session() {
        let prepared = PreparedMedia::StreamAndCache {
            url: "https://example.com/upstream".to_string(),
            content_length: 4096,
            prefix_path: PathBuf::from("/tmp/prefix.webm"),
            prefix_size: 2048,
        };

        let session = RelaySessionSpec::try_from_prepared("track-123", &prepared).unwrap();

        assert_eq!(session.track_id, "track-123");
        assert_eq!(session.staged.path, PathBuf::from("/tmp/prefix.webm"));
        assert_eq!(session.staged.available, RelayByteRange { start: 0, end: 0 });
        assert_eq!(session.upstream.url, "https://example.com/upstream");
        assert_eq!(session.upstream.content_length, 4096);
        assert_eq!(session.contract, RelayTransportContract::default());
        assert_eq!(session.state, RelaySessionState::Registered);
        assert_eq!(
            session.tee_prefix,
            Some(super::RelayTeePrefix { path: PathBuf::from("/tmp/prefix.webm"), size: 2048 })
        );
    }

    #[test]
    fn relay_contract_defaults_to_single_range_and_relay_owned_reconnect() {
        let contract = RelayTransportContract::default();

        assert_eq!(contract.url_path_template, super::RELAY_SESSION_PATH_TEMPLATE);
        assert_eq!(contract.range_policy, super::RelayRangePolicy::SingleRangeOrFull);
        assert_eq!(contract.reconnect_owner, super::RelayReconnectOwner::RelaySession);
    }

    #[test]
    fn relay_contract_rejects_direct_prepared_media() {
        let prepared = PreparedMedia::Direct { url: "https://example.com/direct".to_string() };

        let err = RelaySessionSpec::try_from_prepared("track-123", &prepared).unwrap_err();

        assert_eq!(err, RelayContractError::PreparedMediaMissingStagedPrefix);
    }

    #[test]
    fn relay_contract_rejects_prefix_longer_than_content() {
        let prepared = PreparedMedia::StagedPrefix {
            path: PathBuf::from("/tmp/prefix.webm"),
            bytes: 4097,
            url: "https://example.com/upstream".to_string(),
            content_length: 4096,
        };

        let err = RelaySessionSpec::try_from_prepared("track-123", &prepared).unwrap_err();

        assert_eq!(err, RelayContractError::PrefixLongerThanContent);
    }

    #[test]
    fn relay_player_endpoint_uses_single_localhost_url_shape() {
        let endpoint = RelayPlayerEndpoint::new(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43111),
            RelaySessionId::new("session-42").unwrap(),
        );

        assert_eq!(endpoint.path(), "/relay/sessions/session-42/stream");
        assert_eq!(endpoint.url(), "http://127.0.0.1:43111/relay/sessions/session-42/stream");
    }

    #[test]
    fn relay_range_contract_splits_prefix_and_upstream_segments() {
        let session = RelaySessionSpec::try_from_prepared(
            "track-123",
            &PreparedMedia::StagedPrefix {
                path: PathBuf::from("/tmp/prefix.webm"),
                bytes: 2048,
                url: "https://example.com/upstream".to_string(),
                content_length: 4096,
            },
        )
        .unwrap();

        let plan = session.plan_response(Some("bytes=1024-3071")).unwrap();

        assert_eq!(plan.response, RelayByteRange::new(1024, 3072).unwrap());
        assert_eq!(plan.staged, Some(RelayByteRange::new(1024, 2048).unwrap()));
        assert_eq!(plan.upstream, Some(RelayByteRange::new(2048, 3072).unwrap()));
        assert!(plan.is_partial_content());
        assert_eq!(plan.status_code(), 206);
    }

    #[test]
    fn relay_range_contract_treats_open_ended_request_as_upstream_only_after_prefix() {
        let session = RelaySessionSpec::try_from_prepared(
            "track-123",
            &PreparedMedia::StagedPrefix {
                path: PathBuf::from("/tmp/prefix.webm"),
                bytes: 2048,
                url: "https://example.com/upstream".to_string(),
                content_length: 4096,
            },
        )
        .unwrap();

        let plan = session.plan_response(Some("bytes=3000-")).unwrap();

        assert_eq!(plan.response, RelayByteRange::new(3000, 4096).unwrap());
        assert_eq!(plan.staged, None);
        assert_eq!(plan.upstream, Some(RelayByteRange::new(3000, 4096).unwrap()));
    }

    #[test]
    fn relay_range_contract_rejects_multi_range_requests() {
        let session = RelaySessionSpec::try_from_prepared(
            "track-123",
            &PreparedMedia::StagedPrefix {
                path: PathBuf::from("/tmp/prefix.webm"),
                bytes: 2048,
                url: "https://example.com/upstream".to_string(),
                content_length: 4096,
            },
        )
        .unwrap();

        let err = session.plan_response(Some("bytes=0-99,200-299")).unwrap_err();

        assert_eq!(err, RelayRangeError::MultipleRangesUnsupported);
    }

    #[test]
    fn relay_terminal_state_maps_to_expected_http_status() {
        assert_eq!(RelaySessionState::Completed.terminal_http_status(), Some(410));
        assert_eq!(RelaySessionState::Cancelled.terminal_http_status(), Some(410));
        assert_eq!(RelaySessionState::Expired.terminal_http_status(), Some(410));
        assert_eq!(RelaySessionState::Failed.terminal_http_status(), Some(502));
        assert_eq!(RelaySessionState::Streaming.terminal_http_status(), None);
    }
}
