//! Hotkey manager
//!
//! This module handles global hotkey registration and callback invocation.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Hotkey combination
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hotkey {
    /// Primary key code
    pub key_code: u32,
    /// Control modifier
    pub ctrl: bool,
    /// Alt modifier
    pub alt: bool,
    /// Shift modifier
    pub shift: bool,
    /// Super/Windows/Command modifier
    pub super_key: bool,
}

impl Hotkey {
    /// Create a new hotkey
    pub fn new(key_code: u32) -> Self {
        Self {
            key_code,
            ctrl: false,
            alt: false,
            shift: false,
            super_key: false,
        }
    }

    /// Set ctrl modifier
    pub fn ctrl(mut self) -> Self {
        self.ctrl = true;
        self
    }

    /// Set alt modifier
    pub fn alt(mut self) -> Self {
        self.alt = true;
        self
    }

    /// Set shift modifier
    pub fn shift(mut self) -> Self {
        self.shift = true;
        self
    }

    /// Set super modifier
    pub fn super_key(mut self) -> Self {
        self.super_key = true;
        self
    }

    /// Create a Ctrl+Key combination
    pub fn ctrl_key(key_code: u32) -> Self {
        Self::new(key_code).ctrl()
    }

    /// Create a Alt+Key combination
    pub fn alt_key(key_code: u32) -> Self {
        Self::new(key_code).alt()
    }

    /// Create a Shift+Key combination
    pub fn shift_key(key_code: u32) -> Self {
        Self::new(key_code).shift()
    }

    /// Create a Ctrl+Shift+Key combination
    pub fn ctrl_shift(key_code: u32) -> Self {
        Self::new(key_code).ctrl().shift()
    }

    /// Create a Ctrl+Alt+Key combination
    pub fn ctrl_alt(key_code: u32) -> Self {
        Self::new(key_code).ctrl().alt()
    }
}

/// Hotkey action callback
pub type HotkeyCallback = Arc<dyn Fn() + Send + Sync>;

/// Hotkey registration
pub struct HotkeyRegistration {
    hotkey: Hotkey,
    callback: HotkeyCallback,
    description: String,
}

impl HotkeyRegistration {
    pub fn new(hotkey: Hotkey, callback: HotkeyCallback) -> Self {
        Self {
            hotkey,
            callback,
            description: String::new(),
        }
    }

    pub fn with_description(mut self, description: String) -> Self {
        self.description = description;
        self
    }
}

/// Modifier key state tracker
#[derive(Debug, Clone, Default)]
pub struct ModifierState {
    pub ctrl_pressed: bool,
    pub alt_pressed: bool,
    pub shift_pressed: bool,
    pub super_pressed: bool,
}

impl ModifierState {
    /// Update modifier state based on key code
    pub fn update(&mut self, key_code: u32, pressed: bool) {
        match key_code {
            0x11 => self.ctrl_pressed = pressed,  // VK_CONTROL
            0x12 => self.alt_pressed = pressed,   // VK_MENU
            0x10 => self.shift_pressed = pressed, // VK_SHIFT
            0x5B | 0x5C => self.super_pressed = pressed, // VK_LWIN/RWIN
            _ => {}
        }
    }

    pub fn matches(&self, hotkey: &Hotkey) -> bool {
        self.ctrl_pressed == hotkey.ctrl
            && self.alt_pressed == hotkey.alt
            && self.shift_pressed == hotkey.shift
            && self.super_pressed == hotkey.super_key
    }
}

/// Hotkey manager
pub struct HotkeyManager {
    hotkeys: HashMap<Hotkey, HotkeyCallback>,
    descriptions: HashMap<Hotkey, String>,
    modifiers: ModifierState,
    enabled: bool,
}

impl HotkeyManager {
    pub fn new() -> Self {
        Self {
            hotkeys: HashMap::new(),
            descriptions: HashMap::new(),
            modifiers: ModifierState::default(),
            enabled: true,
        }
    }

    /// Register a hotkey
    pub fn register(&mut self, registration: HotkeyRegistration) -> Result<()> {
        let hotkey = registration.hotkey.clone();
        let description = registration.description.clone();

        self.hotkeys.insert(hotkey.clone(), registration.callback);
        if !description.is_empty() {
            self.descriptions.insert(hotkey.clone(), description);
        }

        tracing::debug!("Registered hotkey: {:?}", hotkey);

        Ok(())
    }

    /// Unregister a hotkey
    pub fn unregister(&mut self, hotkey: &Hotkey) -> Result<()> {
        self.hotkeys.remove(hotkey);
        self.descriptions.remove(hotkey);

        tracing::debug!("Unregistered hotkey: {:?}", hotkey);

        Ok(())
    }

    /// Process a raw key event
    pub fn process_key_event(&mut self, key_code: u32, pressed: bool) -> Option<Hotkey> {
        if !self.enabled {
            return None;
        }

        self.modifiers.update(key_code, pressed);

        // Only trigger on key press
        if !pressed {
            return None;
        }

        for hotkey in self.hotkeys.keys() {
            if hotkey.key_code == key_code && self.modifiers.matches(hotkey) {
                return Some(hotkey.clone());
            }
        }

        None
    }

    /// Invoke a hotkey callback
    pub fn invoke(&self, hotkey: &Hotkey) {
        if let Some(callback) = self.hotkeys.get(hotkey) {
            callback();
        }
    }

    /// Enable hotkey processing
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// Disable hotkey processing
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// Check if hotkeys are enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get all registered hotkeys
    pub fn hotkeys(&self) -> impl Iterator<Item = &Hotkey> {
        self.hotkeys.keys()
    }

    /// Get description for a hotkey
    pub fn description(&self, hotkey: &Hotkey) -> Option<&String> {
        self.descriptions.get(hotkey)
    }

    /// Clear all hotkeys
    pub fn clear(&mut self) {
        self.hotkeys.clear();
        self.descriptions.clear();
    }

    /// Get the modifier state
    pub fn modifiers(&self) -> &ModifierState {
        &self.modifiers
    }
}

impl Default for HotkeyManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared hotkey manager for async access
pub type SharedHotkeyManager = Arc<RwLock<HotkeyManager>>;

/// Create a shared hotkey manager
pub fn create_shared_hotkey_manager() -> SharedHotkeyManager {
    Arc::new(RwLock::new(HotkeyManager::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn test_hotkey_creation() {
        let hotkey = Hotkey::new(0x41).ctrl().shift(); // 'A' key code

        assert!(hotkey.ctrl);
        assert!(hotkey.shift);
        assert!(!hotkey.alt);
        assert_eq!(hotkey.key_code, 0x41);
    }

    #[test]
    fn test_hotkey_combinations() {
        assert!(Hotkey::ctrl_key(0x53).ctrl); // 'S'
        assert!(Hotkey::alt_key(0x54).alt);   // 'T'
        assert!(Hotkey::shift_key(0x44).shift); // 'D'
        assert!(Hotkey::ctrl_alt(0x44).ctrl);
        assert!(Hotkey::ctrl_alt(0x44).alt);
    }

    #[test]
    fn test_modifier_state() {
        let mut state = ModifierState::default();

        state.update(0x11, true); // VK_CONTROL
        assert!(state.ctrl_pressed);

        state.update(0x10, true); // VK_SHIFT
        assert!(state.shift_pressed);

        state.update(0x11, false);
        assert!(!state.ctrl_pressed);
    }

    #[test]
    fn test_hotkey_manager() {
        let mut manager = HotkeyManager::new();

        let triggered = Arc::new(AtomicBool::new(false));
        let triggered_clone = triggered.clone();

        let hotkey = Hotkey::ctrl_shift(0x51); // Ctrl+Shift+Q

        let callback: HotkeyCallback = Arc::new(move || {
            triggered_clone.store(true, Ordering::SeqCst);
        });

        let registration = HotkeyRegistration::new(hotkey.clone(), callback);
        manager.register(registration).unwrap();

        // Simulate key press
        manager.process_key_event(0x11, true); // Ctrl down
        manager.process_key_event(0x10, true); // Shift down
        let triggered_hotkey = manager.process_key_event(0x51, true); // Q down

        assert!(triggered_hotkey.is_some());
        if let Some(hk) = triggered_hotkey {
            manager.invoke(&hk);
        }

        assert!(triggered.load(Ordering::SeqCst));
    }

    #[test]
    fn test_hotkey_enable_disable() {
        let mut manager = HotkeyManager::new();
        assert!(manager.is_enabled());

        manager.disable();
        assert!(!manager.is_enabled());

        manager.enable();
        assert!(manager.is_enabled());
    }
}
