//! Backend selection and fallback orchestration
//!
//! This module provides the backend selector that chooses the best available
//! backend based on preference order and health status.

use crate::backend::{BackendCapabilities, BackendKind};
use rshare_core::{BackendFailureReason, ResolvedInputMode};
use std::fmt::Debug;

/// A backend candidate for selection.
#[derive(Debug, Clone)]
pub struct BackendCandidate {
    /// The kind of backend.
    pub kind: BackendKind,
    /// Whether this backend is available and healthy.
    pub healthy: bool,
    /// Failure reason if unhealthy.
    pub failure_reason: Option<BackendFailureReason>,
    /// Capabilities of this backend.
    pub capabilities: BackendCapabilities,
}

impl BackendCandidate {
    /// Create a new healthy backend candidate.
    pub fn healthy(kind: BackendKind) -> Self {
        Self {
            kind,
            healthy: true,
            failure_reason: None,
            capabilities: BackendCapabilities::default(),
        }
    }

    /// Create a new unhealthy backend candidate.
    pub fn unhealthy(kind: BackendKind, reason: BackendFailureReason) -> Self {
        Self {
            kind,
            healthy: false,
            failure_reason: Some(reason),
            capabilities: BackendCapabilities::default(),
        }
    }

    /// Create a new backend candidate with custom capabilities.
    pub fn with_capabilities(mut self, capabilities: BackendCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }
}

/// Selection result containing the chosen backend and metadata.
#[derive(Debug, Clone)]
pub struct SelectionResult {
    /// The selected backend kind.
    pub kind: BackendKind,
    /// Whether the system is in degraded mode (running with non-preferred backend).
    pub degraded: bool,
    /// Reason for degradation if applicable.
    pub degradation_reason: Option<String>,
}

impl SelectionResult {
    /// Create a new non-degraded selection result.
    pub fn new(kind: BackendKind) -> Self {
        Self {
            kind,
            degraded: false,
            degradation_reason: None,
        }
    }

    /// Create a degraded selection result with a reason.
    pub fn degraded(kind: BackendKind, reason: String) -> Self {
        Self {
            kind,
            degraded: true,
            degradation_reason: Some(reason),
        }
    }

    /// Convert to ResolvedInputMode.
    pub fn to_input_mode(&self) -> Option<ResolvedInputMode> {
        match self.kind {
            BackendKind::Portable => Some(ResolvedInputMode::Portable),
            #[cfg(target_os = "windows")]
            BackendKind::WindowsNative => Some(ResolvedInputMode::WindowsNative),
            #[cfg(target_os = "windows")]
            BackendKind::VirtualHid => Some(ResolvedInputMode::VirtualHid),
            #[cfg(target_os = "linux")]
            BackendKind::Evdev => Some(ResolvedInputMode::Evdev),
            #[cfg(target_os = "linux")]
            BackendKind::UInput => Some(ResolvedInputMode::UInput),
        }
    }
}

/// Backend selector that chooses based on preference order.
///
/// Preference order:
/// 1. VirtualHid (Windows only, optional)
/// 2. WindowsNative (Windows only)
/// 3. Portable (cross-platform)
#[derive(Debug, Clone, Default)]
pub struct BackendSelector;

impl BackendSelector {
    /// Create a new backend selector.
    pub fn new() -> Self {
        Self
    }

    /// Select the best backend from available candidates.
    pub fn select(&self, candidates: &[BackendCandidate]) -> Option<SelectionResult> {
        if candidates.is_empty() {
            return None;
        }

        // Build preference order based on platform
        let preference_order = self.preference_order();

        // Find the first healthy backend in preference order
        let mut selected_kind: Option<BackendKind> = None;
        let mut first_healthy_index: Option<usize> = None;

        for (index, preferred_kind) in preference_order.iter().enumerate() {
            // Check if any candidate of this kind is healthy
            let has_healthy = candidates
                .iter()
                .any(|c| c.kind == *preferred_kind && c.healthy);

            if has_healthy {
                selected_kind = Some(*preferred_kind);
                first_healthy_index = Some(index);
                break;
            }
        }

        let kind = selected_kind?;

        // Determine if this is degraded mode
        // Degraded if we skipped higher-priority backends that exist in candidates
        let available_kinds: Vec<_> = candidates
            .iter()
            .map(|c| c.kind)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        let has_higher_priority_available = available_kinds.iter().any(|k| {
            preference_order
                .iter()
                .position(|x| x == k)
                .map(|pos| pos < first_healthy_index.unwrap())
                .unwrap_or(false)
        });

        let result = if has_higher_priority_available {
            let first_available = preference_order
                .iter()
                .find(|k| available_kinds.contains(k))
                .unwrap();
            SelectionResult::degraded(
                kind,
                format!(
                    "Preferred backend {:?} unavailable, using {:?}",
                    first_available, kind
                ),
            )
        } else {
            SelectionResult::new(kind)
        };

        Some(result)
    }

    /// Get the preference order for backends on this platform.
    fn preference_order(&self) -> Vec<BackendKind> {
        #[cfg(target_os = "windows")]
        {
            let mut order = vec![BackendKind::Portable];
            order.insert(0, BackendKind::WindowsNative);
            order.insert(0, BackendKind::VirtualHid);
            order
        }

        #[cfg(target_os = "linux")]
        {
            // Prefer Evdev (driver-level) over Portable (user-space fallback)
            let mut order = vec![BackendKind::Portable];
            order.insert(0, BackendKind::Evdev);
            order
        }

        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            vec![BackendKind::Portable]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(kind: BackendKind, healthy: bool) -> BackendCandidate {
        if healthy {
            BackendCandidate::healthy(kind)
        } else {
            BackendCandidate::unhealthy(kind, BackendFailureReason::Unavailable)
        }
    }

    #[cfg(target_os = "windows")]
    fn virtual_hid_candidate(healthy: bool, reason: Option<&str>) -> BackendCandidate {
        if healthy {
            BackendCandidate::healthy(BackendKind::VirtualHid)
        } else {
            BackendCandidate::unhealthy(
                BackendKind::VirtualHid,
                reason
                    .and_then(|r| match r {
                        "protocol mismatch" => Some(BackendFailureReason::VersionMismatch),
                        "unavailable" => Some(BackendFailureReason::Unavailable),
                        _ => None,
                    })
                    .unwrap_or(BackendFailureReason::Unavailable),
            )
        }
    }

    #[cfg(target_os = "windows")]
    fn windows_native_candidate(healthy: bool) -> BackendCandidate {
        candidate(BackendKind::WindowsNative, healthy)
    }

    fn portable_candidate(healthy: bool) -> BackendCandidate {
        candidate(BackendKind::Portable, healthy)
    }

    #[test]
    fn selector_prefers_portable_when_only_option() {
        let selector = BackendSelector::new();
        let candidates = vec![portable_candidate(true)];

        let selected = selector.select(&candidates).unwrap();
        assert_eq!(selected.kind, BackendKind::Portable);
        assert!(!selected.degraded);
    }

    #[test]
    fn selector_skips_unhealthy_backends() {
        let selector = BackendSelector::new();
        let candidates = vec![portable_candidate(false), portable_candidate(true)];

        let selected = selector.select(&candidates).unwrap();
        assert_eq!(selected.kind, BackendKind::Portable);
    }

    #[test]
    fn selector_returns_none_when_all_unhealthy() {
        let selector = BackendSelector::new();
        let candidates = vec![portable_candidate(false), portable_candidate(false)];

        assert!(selector.select(&candidates).is_none());
    }

    #[test]
    fn selector_returns_none_for_empty_candidates() {
        let selector = BackendSelector::new();
        assert!(selector.select(&[]).is_none());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn selector_skips_unhealthy_virtual_hid() {
        let selector = BackendSelector::new();
        let candidates = vec![
            virtual_hid_candidate(false, Some("unavailable")),
            windows_native_candidate(true),
            portable_candidate(true),
        ];

        let selected = selector.select(&candidates).unwrap();
        assert_eq!(selected.kind, BackendKind::WindowsNative);
        assert!(selected.degraded);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn selector_prefers_virtual_hid_when_healthy() {
        let selector = BackendSelector::new();
        let candidates = vec![
            virtual_hid_candidate(true, None),
            windows_native_candidate(true),
            portable_candidate(true),
        ];

        let selected = selector.select(&candidates).unwrap();
        assert_eq!(selected.kind, BackendKind::VirtualHid);
        assert!(!selected.degraded);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn selector_degrades_when_virtual_hid_version_mismatches() {
        let selector = BackendSelector::new();
        let candidates = vec![
            virtual_hid_candidate(false, Some("protocol mismatch")),
            windows_native_candidate(true),
            portable_candidate(true),
        ];

        let selected = selector.select(&candidates).unwrap();
        assert_eq!(selected.kind, BackendKind::WindowsNative);
        assert!(selected.degraded);
        assert!(selected.degradation_reason.unwrap().contains("VirtualHid"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn selector_falls_back_to_portable_when_native_unavailable() {
        let selector = BackendSelector::new();
        let candidates = vec![
            virtual_hid_candidate(false, Some("unavailable")),
            windows_native_candidate(false),
            portable_candidate(true),
        ];

        let selected = selector.select(&candidates).unwrap();
        assert_eq!(selected.kind, BackendKind::Portable);
        assert!(selected.degraded);
    }

    #[cfg(target_os = "linux")]
    fn evdev_candidate(healthy: bool) -> BackendCandidate {
        candidate(BackendKind::Evdev, healthy)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn selector_prefers_evdev_over_portable() {
        let selector = BackendSelector::new();
        let candidates = vec![evdev_candidate(true), portable_candidate(true)];

        let selected = selector.select(&candidates).unwrap();
        assert_eq!(selected.kind, BackendKind::Evdev);
        assert!(!selected.degraded);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn selector_falls_back_to_portable_when_evdev_unavailable() {
        let selector = BackendSelector::new();
        let candidates = vec![evdev_candidate(false), portable_candidate(true)];

        let selected = selector.select(&candidates).unwrap();
        assert_eq!(selected.kind, BackendKind::Portable);
        assert!(selected.degraded);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn selector_degrades_when_evdev_permission_denied() {
        let selector = BackendSelector::new();
        let candidates = vec![
            BackendCandidate::unhealthy(BackendKind::Evdev, BackendFailureReason::PermissionDenied),
            portable_candidate(true),
        ];

        let selected = selector.select(&candidates).unwrap();
        assert_eq!(selected.kind, BackendKind::Portable);
        assert!(selected.degraded);
    }
}
