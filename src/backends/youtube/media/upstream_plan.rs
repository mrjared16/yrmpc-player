#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamReadPlan {
    QueryRange { start: u64, end: u64 },
    AnchoredQueryRange { start: u64, end: u64 },
    ChunkedQueryRange { start: u64, end: u64 },
}

impl UpstreamReadPlan {
    pub fn range(self) -> (u64, u64) {
        match self {
            Self::QueryRange { start, end }
            | Self::AnchoredQueryRange { start, end }
            | Self::ChunkedQueryRange { start, end } => (start, end),
        }
    }
}

pub fn default_upstream_read_plans(start: u64, end: u64) -> [UpstreamReadPlan; 3] {
    [
        UpstreamReadPlan::QueryRange { start, end },
        UpstreamReadPlan::AnchoredQueryRange { start, end },
        UpstreamReadPlan::ChunkedQueryRange { start, end },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_upstream_read_plans_preserve_order() {
        assert_eq!(
            default_upstream_read_plans(10, 20),
            [
                UpstreamReadPlan::QueryRange { start: 10, end: 20 },
                UpstreamReadPlan::AnchoredQueryRange { start: 10, end: 20 },
                UpstreamReadPlan::ChunkedQueryRange { start: 10, end: 20 },
            ]
        );
    }
}
