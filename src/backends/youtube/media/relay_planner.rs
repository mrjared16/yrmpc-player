use std::path::PathBuf;

use super::{
    PreparedMedia, RelayByteRange, RelayContractError, RelaySessionSpec, RelayStagedArtifact,
    RelayTeePrefix, UpstreamReadPlan, default_upstream_read_plans,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayPlayStrategy {
    CacheHitRelay {
        track_id: String,
        stream_url: String,
        prefix: RelayStagedArtifact,
    },
    TeeMissRelay {
        track_id: String,
        stream_url: String,
        prefix_target: RelayTeePrefix,
    },
    DirectFallback {
        track_id: String,
        stream_url: String,
    },
}

pub struct RelayPlanner;

impl RelayPlanner {
    pub fn from_prepared(
        track_id: impl Into<String>,
        prepared: &PreparedMedia,
    ) -> Result<RelayPlayStrategy, RelayContractError> {
        let track_id = track_id.into();
        match prepared {
            PreparedMedia::StagedPrefix { path, bytes, url, content_length } => {
                if track_id.is_empty() {
                    return Err(RelayContractError::EmptyTrackId);
                }
                if url.is_empty() {
                    return Err(RelayContractError::EmptyUpstreamUrl);
                }
                if *bytes == 0 || *content_length == 0 {
                    return Err(RelayContractError::EmptyPrefixArtifact);
                }
                if *bytes > *content_length {
                    return Err(RelayContractError::PrefixLongerThanContent);
                }

                Ok(RelayPlayStrategy::CacheHitRelay {
                    track_id,
                    stream_url: url.clone(),
                    prefix: RelayStagedArtifact {
                        path: path.clone(),
                        available: RelayByteRange { start: 0, end: *bytes },
                    },
                })
            }
            PreparedMedia::StreamAndCache { url, content_length, prefix_path, prefix_size } => {
                if track_id.is_empty() {
                    return Err(RelayContractError::EmptyTrackId);
                }
                if url.is_empty() {
                    return Err(RelayContractError::EmptyUpstreamUrl);
                }
                if *content_length == 0 {
                    return Err(RelayContractError::EmptyPrefixArtifact);
                }

                Ok(RelayPlayStrategy::TeeMissRelay {
                    track_id,
                    stream_url: url.clone(),
                    prefix_target: RelayTeePrefix {
                        path: prefix_path.clone(),
                        size: *prefix_size,
                    },
                })
            }
            PreparedMedia::Direct { .. } | PreparedMedia::LocalFile { .. } => {
                Err(RelayContractError::PreparedMediaMissingStagedPrefix)
            }
        }
    }

    pub fn from_session(spec: &RelaySessionSpec) -> RelayPlayStrategy {
        if let Some(prefix_target) = &spec.tee_prefix {
            RelayPlayStrategy::TeeMissRelay {
                track_id: spec.track_id.clone(),
                stream_url: spec.upstream.url.clone(),
                prefix_target: prefix_target.clone(),
            }
        } else {
            RelayPlayStrategy::CacheHitRelay {
                track_id: spec.track_id.clone(),
                stream_url: spec.upstream.url.clone(),
                prefix: spec.staged.clone(),
            }
        }
    }
}

impl RelayPlayStrategy {
    pub fn direct_fallback(track_id: impl Into<String>, stream_url: impl Into<String>) -> Self {
        Self::DirectFallback {
            track_id: track_id.into(),
            stream_url: stream_url.into(),
        }
    }

    pub fn continuation_plans(&self, start: u64, end: u64) -> Vec<UpstreamReadPlan> {
        match self {
            Self::CacheHitRelay { .. } | Self::TeeMissRelay { .. } => {
                default_upstream_read_plans(start, end).into()
            }
            Self::DirectFallback { .. } => Vec::new(),
        }
    }

    pub fn stream_url(&self) -> &str {
        match self {
            Self::CacheHitRelay { stream_url, .. }
            | Self::TeeMissRelay { stream_url, .. }
            | Self::DirectFallback { stream_url, .. } => stream_url,
        }
    }

    pub fn prefix_path(&self) -> Option<&PathBuf> {
        match self {
            Self::CacheHitRelay { prefix, .. } => Some(&prefix.path),
            Self::TeeMissRelay { prefix_target, .. } => Some(&prefix_target.path),
            Self::DirectFallback { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn cache_hit_strategy_builds_continuation_plans() {
        let strategy = RelayPlanner::from_prepared(
            "track-1",
            &PreparedMedia::StagedPrefix {
                path: PathBuf::from("/tmp/prefix.webm"),
                bytes: 2048,
                url: "https://example.com/upstream".to_string(),
                content_length: 4096,
            },
        )
        .unwrap();

        assert!(matches!(strategy, RelayPlayStrategy::CacheHitRelay { .. }));
        assert_eq!(strategy.continuation_plans(5, 9).len(), 3);
    }
}
