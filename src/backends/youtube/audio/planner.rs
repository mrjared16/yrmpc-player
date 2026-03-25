use crate::backends::youtube::config::AudioDeliveryMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioTransportTarget {
    DirectUrl,
    PreparedInput,
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
pub struct AudioDeliveryPlan {
    pub mode: AudioDeliveryMode,
    pub transport: AudioTransportTarget,
    pub prepare_action: PrepareAction,
    pub prefetch_policy: PrefetchPolicy,
    pub enable_mpv_reconnect: bool,
}

impl AudioDeliveryPlan {
    pub fn uses_local_staging(self) -> bool {
        matches!(self.prepare_action, PrepareAction::StagePrefix)
    }

    pub fn streams_immediate_cache_miss_via_relay(self) -> bool {
        matches!(self.transport, AudioTransportTarget::LocalRelay) && self.uses_local_staging()
    }

    pub fn allows_immediate_direct(self) -> bool {
        matches!(self.mode, AudioDeliveryMode::Direct)
    }

    pub fn needs_source_adapter(self) -> bool {
        false
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct AudioDeliveryPlanner;

impl AudioDeliveryPlanner {
    pub fn plan(self, mode: AudioDeliveryMode) -> AudioDeliveryPlan {
        match mode {
            AudioDeliveryMode::Auto => AudioDeliveryPlan {
                mode,
                transport: AudioTransportTarget::LocalRelay,
                prepare_action: PrepareAction::StagePrefix,
                prefetch_policy: PrefetchPolicy::StagePrefix,
                enable_mpv_reconnect: true,
            },
            AudioDeliveryMode::Staged => AudioDeliveryPlan {
                mode,
                transport: AudioTransportTarget::PreparedInput,
                prepare_action: PrepareAction::StagePrefix,
                prefetch_policy: PrefetchPolicy::StagePrefix,
                enable_mpv_reconnect: false,
            },
            AudioDeliveryMode::Direct => AudioDeliveryPlan {
                mode,
                transport: AudioTransportTarget::DirectUrl,
                prepare_action: PrepareAction::ResolveOnly,
                prefetch_policy: PrefetchPolicy::ResolveOnly,
                enable_mpv_reconnect: true,
            },
            AudioDeliveryMode::Relay => AudioDeliveryPlan {
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
    use super::{AudioDeliveryPlanner, AudioTransportTarget, PrepareAction};
    use crate::backends::youtube::config::AudioDeliveryMode;

    #[test]
    fn planner_maps_staged_to_prepared_input_transport() {
        let plan = AudioDeliveryPlanner.plan(AudioDeliveryMode::Staged);
        assert_eq!(plan.transport, AudioTransportTarget::PreparedInput);
        assert_eq!(plan.prepare_action, PrepareAction::StagePrefix);
        assert!(!plan.needs_source_adapter());
        assert!(!plan.enable_mpv_reconnect);
    }

    #[test]
    fn planner_maps_direct_to_resolve_only() {
        let plan = AudioDeliveryPlanner.plan(AudioDeliveryMode::Direct);
        assert_eq!(plan.transport, AudioTransportTarget::DirectUrl);
        assert_eq!(plan.prepare_action, PrepareAction::ResolveOnly);
        assert!(plan.enable_mpv_reconnect);
    }

    #[test]
    fn planner_maps_auto_to_relay_transport_for_non_direct_preparation() {
        let plan = AudioDeliveryPlanner.plan(AudioDeliveryMode::Auto);
        assert_eq!(plan.transport, AudioTransportTarget::LocalRelay);
        assert_eq!(plan.prepare_action, PrepareAction::StagePrefix);
        assert!(plan.streams_immediate_cache_miss_via_relay());
        assert!(!plan.allows_immediate_direct());
        assert!(plan.enable_mpv_reconnect);
    }

    #[test]
    fn planner_maps_relay_to_local_relay_with_shared_staging() {
        let plan = AudioDeliveryPlanner.plan(AudioDeliveryMode::Relay);
        assert_eq!(plan.transport, AudioTransportTarget::LocalRelay);
        assert_eq!(plan.prepare_action, PrepareAction::StagePrefix);
        assert!(plan.streams_immediate_cache_miss_via_relay());
        assert!(plan.enable_mpv_reconnect);
    }

    #[test]
    fn planner_does_not_use_tee_miss_for_non_relay_modes() {
        assert!(!AudioDeliveryPlanner
            .plan(AudioDeliveryMode::Staged)
            .streams_immediate_cache_miss_via_relay());
        assert!(!AudioDeliveryPlanner
            .plan(AudioDeliveryMode::Direct)
            .streams_immediate_cache_miss_via_relay());
    }
}
