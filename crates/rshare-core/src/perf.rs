use hdrhistogram::Histogram;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ClockDomainId(pub u64);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct MonotonicStamp {
    pub domain: ClockDomainId,
    pub value_us: u64,
}

#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
pub enum MonotonicTimeError {
    #[error("cannot compare clock domains {earlier:?} and {later:?}")]
    ClockDomainMismatch {
        earlier: ClockDomainId,
        later: ClockDomainId,
    },
    #[error("monotonic clock regressed from {earlier_us}us to {later_us}us")]
    ClockRegression { earlier_us: u64, later_us: u64 },
}

impl MonotonicStamp {
    pub const fn new(domain: ClockDomainId, value_us: u64) -> Self {
        Self { domain, value_us }
    }

    pub fn checked_duration_since(self, earlier: Self) -> Result<u64, MonotonicTimeError> {
        if self.domain != earlier.domain {
            return Err(MonotonicTimeError::ClockDomainMismatch {
                earlier: earlier.domain,
                later: self.domain,
            });
        }
        if self.value_us < earlier.value_us {
            return Err(MonotonicTimeError::ClockRegression {
                earlier_us: earlier.value_us,
                later_us: self.value_us,
            });
        }
        Ok(self.value_us - earlier.value_us)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SenderStageStamps {
    pub captured: MonotonicStamp,
    pub ingress_enqueued: MonotonicStamp,
    pub router_dequeued: MonotonicStamp,
    pub transport_enqueued: Option<MonotonicStamp>,
}

impl SenderStageStamps {
    #[cfg(test)]
    fn fixture(
        domain: ClockDomainId,
        captured_us: u64,
        ingress_enqueued_us: u64,
        router_dequeued_us: u64,
        transport_enqueued_us: u64,
    ) -> Self {
        Self {
            captured: MonotonicStamp::new(domain, captured_us),
            ingress_enqueued: MonotonicStamp::new(domain, ingress_enqueued_us),
            router_dequeued: MonotonicStamp::new(domain, router_dequeued_us),
            transport_enqueued: Some(MonotonicStamp::new(domain, transport_enqueued_us)),
        }
    }

    pub fn capture_to_route_us(&self) -> Option<u64> {
        self.router_dequeued
            .checked_duration_since(self.captured)
            .ok()
    }

    pub fn capture_to_transport_us(&self) -> Option<u64> {
        self.transport_enqueued?
            .checked_duration_since(self.captured)
            .ok()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReceiverStageStamps {
    pub received: MonotonicStamp,
    pub injection_started: Option<MonotonicStamp>,
    pub injection_completed: Option<MonotonicStamp>,
}

impl ReceiverStageStamps {
    #[cfg(test)]
    fn fixture(
        domain: ClockDomainId,
        received_us: u64,
        injection_started_us: u64,
        injection_completed_us: u64,
    ) -> Self {
        Self {
            received: MonotonicStamp::new(domain, received_us),
            injection_started: Some(MonotonicStamp::new(domain, injection_started_us)),
            injection_completed: Some(MonotonicStamp::new(domain, injection_completed_us)),
        }
    }

    pub fn receive_to_inject_us(&self) -> Option<u64> {
        self.injection_completed?
            .checked_duration_since(self.received)
            .ok()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct LatencyReport {
    pub samples: u64,
    pub p50_us: Option<u64>,
    pub p95_us: Option<u64>,
    pub p99_us: Option<u64>,
    pub max_us: Option<u64>,
    pub overflow: u64,
}

impl LatencyReport {
    pub const fn empty() -> Self {
        Self {
            samples: 0,
            p50_us: None,
            p95_us: None,
            p99_us: None,
            max_us: None,
            overflow: 0,
        }
    }
}

pub struct RollingLatencyHistogram {
    histogram: Histogram<u64>,
    max_us: u64,
    overflow: u64,
}

impl RollingLatencyHistogram {
    pub fn new(max_us: u64) -> Result<Self, hdrhistogram::CreationError> {
        Ok(Self {
            histogram: Histogram::new_with_max(max_us, 3)?,
            max_us,
            overflow: 0,
        })
    }

    pub fn record(&mut self, value_us: u64) {
        if value_us > self.max_us {
            self.overflow = self
                .overflow
                .checked_add(1)
                .expect("latency histogram overflow counter exhausted");
            return;
        }

        self.histogram
            .record(value_us)
            .expect("configured latency bound must be recordable");
    }

    pub fn report(&self) -> LatencyReport {
        if self.histogram.is_empty() {
            return LatencyReport {
                overflow: self.overflow,
                ..LatencyReport::empty()
            };
        }

        LatencyReport {
            samples: self.histogram.len(),
            p50_us: Some(self.histogram.value_at_quantile(0.50)),
            p95_us: Some(self.histogram.value_at_quantile(0.95)),
            p99_us: Some(self.histogram.value_at_quantile(0.99)),
            max_us: Some(self.histogram.max()),
            overflow: self.overflow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_clock_domain_duration_is_rejected() {
        let sender = MonotonicStamp::new(ClockDomainId(1), 100);
        let receiver = MonotonicStamp::new(ClockDomainId(2), 180);
        assert_eq!(
            receiver.checked_duration_since(sender),
            Err(MonotonicTimeError::ClockDomainMismatch {
                earlier: ClockDomainId(1),
                later: ClockDomainId(2),
            })
        );
    }

    #[test]
    fn same_domain_clock_regression_is_rejected() {
        let earlier = MonotonicStamp::new(ClockDomainId(1), 180);
        let later = MonotonicStamp::new(ClockDomainId(1), 100);
        assert!(matches!(
            later.checked_duration_since(earlier),
            Err(MonotonicTimeError::ClockRegression {
                earlier_us: 180,
                later_us: 100
            })
        ));
    }

    #[test]
    fn sender_and_receiver_stages_report_only_local_durations() {
        let sender = SenderStageStamps::fixture(ClockDomainId(1), 100, 120, 150, 180);
        let receiver = ReceiverStageStamps::fixture(ClockDomainId(2), 20, 30, 45);
        assert_eq!(sender.capture_to_route_us(), Some(50));
        assert_eq!(sender.capture_to_transport_us(), Some(80));
        assert_eq!(receiver.receive_to_inject_us(), Some(25));
    }

    #[test]
    fn histogram_overflow_is_counted_not_silently_dropped() {
        let mut histogram = RollingLatencyHistogram::new(100).unwrap();
        histogram.record(101);
        assert_eq!(histogram.report().overflow, 1);
    }

    #[test]
    fn unobserved_stage_is_unavailable_not_zero() {
        let report = LatencyReport::empty();
        assert_eq!(report.p50_us, None);
        assert_eq!(report.samples, 0);
    }
}
