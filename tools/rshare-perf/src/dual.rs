use crate::report::{Availability, VerdictStatus};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DualMachineAvailability {
    pub availability: Availability,
    pub verdict: VerdictStatus,
}

pub fn require_physical_runner(configured: bool) -> DualMachineAvailability {
    if configured {
        DualMachineAvailability {
            availability: Availability::Available,
            verdict: VerdictStatus::NotRun,
        }
    } else {
        DualMachineAvailability {
            availability: Availability::Unsupported {
                reason: "physical dual-machine runner is not configured".into(),
            },
            verdict: VerdictStatus::Unsupported,
        }
    }
}
