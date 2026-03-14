use crate::backends::youtube::config::AudioDeliveryMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioTransportTarget {
    DirectUrl,
    Combined,
    LocalRelay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrepareAction {
    ResolveOnly,
    StagePrefix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefetchPolicy {
    ResolveOnly,
    StagePrefix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioSourcePlan {
    pub mode: AudioDeliveryMode,
    pub transport: AudioTransportTarget,
    pub prepare_action: PrepareAction,
    pub prefetch_policy: PrefetchPolicy,
    pub enable_mpv_reconnect: bool,
}

impl AudioSourcePlan {
    pub fn uses_local_staging(self) -> bool {
        matches!(self.prepare_action, PrepareAction::StagePrefix)
    }

    pub fn needs_source_adapter(self) -> bool {
        false
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct AudioSourcePlanner;

impl AudioSourcePlanner {
    pub fn plan(self, mode: AudioDeliveryMode) -> AudioSourcePlan {
        match mode {
            AudioDeliveryMode::Combined => AudioSourcePlan {
                mode,
                transport: AudioTransportTarget::Combined,
                prepare_action: PrepareAction::StagePrefix,
                prefetch_policy: PrefetchPolicy::StagePrefix,
                enable_mpv_reconnect: false,
            },
            AudioDeliveryMode::Direct => AudioSourcePlan {
                mode,
                transport: AudioTransportTarget::DirectUrl,
                prepare_action: PrepareAction::ResolveOnly,
                prefetch_policy: PrefetchPolicy::ResolveOnly,
                enable_mpv_reconnect: true,
            },
            AudioDeliveryMode::Relay => AudioSourcePlan {
                mode,
                transport: AudioTransportTarget::LocalRelay,
                prepare_action: PrepareAction::StagePrefix,
                prefetch_policy: PrefetchPolicy::StagePrefix,
                enable_mpv_reconnect: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioSourcePlanner, AudioTransportTarget, PrepareAction};
    use crate::backends::youtube::config::AudioDeliveryMode;

    #[test]
    fn planner_maps_combined_to_combined_transport() {
        let plan = AudioSourcePlanner.plan(AudioDeliveryMode::Combined);
        assert_eq!(plan.transport, AudioTransportTarget::Combined);
        assert_eq!(plan.prepare_action, PrepareAction::StagePrefix);
        assert!(!plan.needs_source_adapter());
        assert!(!plan.enable_mpv_reconnect);
    }

    #[test]
    fn planner_maps_direct_to_resolve_only() {
        let plan = AudioSourcePlanner.plan(AudioDeliveryMode::Direct);
        assert_eq!(plan.transport, AudioTransportTarget::DirectUrl);
        assert_eq!(plan.prepare_action, PrepareAction::ResolveOnly);
        assert!(plan.enable_mpv_reconnect);
    }

    #[test]
    fn planner_maps_relay_to_local_relay_with_shared_staging() {
        let plan = AudioSourcePlanner.plan(AudioDeliveryMode::Relay);
        assert_eq!(plan.transport, AudioTransportTarget::LocalRelay);
        assert_eq!(plan.prepare_action, PrepareAction::StagePrefix);
        assert!(plan.enable_mpv_reconnect);
    }
}
