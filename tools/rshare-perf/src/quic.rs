use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LoadKind {
    Diagnostics,
    Status,
    Audio,
    Bulk,
}

impl std::str::FromStr for LoadKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "diagnostics" => Ok(Self::Diagnostics),
            "status" => Ok(Self::Status),
            "audio" => Ok(Self::Audio),
            "bulk" => Ok(Self::Bulk),
            other => Err(format!("unknown load kind {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuicScenario {
    Rate {
        rate_hz: u32,
        duration_secs: u64,
        load: Vec<LoadKind>,
    },
    SlowFastPeerIsolation,
    StallRecovery {
        stall_ms: u64,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ScenarioError {
    #[error("unsupported QUIC rate/duration/load combination")]
    UnsupportedCombination,
    #[error("stall recovery must use exactly 100 ms")]
    StallMustBeExactly100Ms,
}

impl QuicScenario {
    pub fn validate(&self) -> Result<(), ScenarioError> {
        match self {
            Self::Rate {
                rate_hz,
                duration_secs,
                load,
            } => {
                let base =
                    matches!((*rate_hz, *duration_secs), (125, 10) | (500, 10)) && load.is_empty();
                let thousand = (*rate_hz, *duration_secs) == (1000, 60)
                    && (load.is_empty()
                        || *load
                            == [
                                LoadKind::Diagnostics,
                                LoadKind::Status,
                                LoadKind::Audio,
                                LoadKind::Bulk,
                            ]);
                if base || thousand {
                    Ok(())
                } else {
                    Err(ScenarioError::UnsupportedCombination)
                }
            }
            Self::SlowFastPeerIsolation => Ok(()),
            Self::StallRecovery { stall_ms: 100 } => Ok(()),
            Self::StallRecovery { .. } => Err(ScenarioError::StallMustBeExactly100Ms),
        }
    }
}

pub fn scenario_matrix() -> Vec<QuicScenario> {
    vec![
        QuicScenario::Rate {
            rate_hz: 125,
            duration_secs: 10,
            load: vec![],
        },
        QuicScenario::Rate {
            rate_hz: 500,
            duration_secs: 10,
            load: vec![],
        },
        QuicScenario::Rate {
            rate_hz: 1000,
            duration_secs: 60,
            load: vec![],
        },
        QuicScenario::Rate {
            rate_hz: 1000,
            duration_secs: 60,
            load: vec![
                LoadKind::Diagnostics,
                LoadKind::Status,
                LoadKind::Audio,
                LoadKind::Bulk,
            ],
        },
        QuicScenario::SlowFastPeerIsolation,
        QuicScenario::StallRecovery { stall_ms: 100 },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quic_matrix_contains_every_predeclared_scenario() {
        assert_eq!(
            scenario_matrix(),
            vec![
                QuicScenario::Rate {
                    rate_hz: 125,
                    duration_secs: 10,
                    load: vec![],
                },
                QuicScenario::Rate {
                    rate_hz: 500,
                    duration_secs: 10,
                    load: vec![],
                },
                QuicScenario::Rate {
                    rate_hz: 1000,
                    duration_secs: 60,
                    load: vec![],
                },
                QuicScenario::Rate {
                    rate_hz: 1000,
                    duration_secs: 60,
                    load: vec![
                        LoadKind::Diagnostics,
                        LoadKind::Status,
                        LoadKind::Audio,
                        LoadKind::Bulk,
                    ],
                },
                QuicScenario::SlowFastPeerIsolation,
                QuicScenario::StallRecovery { stall_ms: 100 },
            ]
        );
    }

    #[test]
    fn stall_recovery_rejects_non_exact_duration() {
        assert!(matches!(
            QuicScenario::StallRecovery { stall_ms: 99 }.validate(),
            Err(ScenarioError::StallMustBeExactly100Ms)
        ));
    }
}
