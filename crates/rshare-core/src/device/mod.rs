//! Device management

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

use crate::protocol::{DeviceId, ScreenInfo, Direction};

/// Device status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceStatus {
    Online,
    Offline,
    Unknown,
}

impl DeviceStatus {
    pub fn is_online(&self) -> bool {
        matches!(self, DeviceStatus::Online)
    }
}

/// Device information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: DeviceId,
    pub name: String,
    pub hostname: String,
    pub screen: ScreenInfo,
    pub status: DeviceStatus,
    pub last_seen: Option<u64>, // timestamp
}

impl Device {
    pub fn new(id: DeviceId, name: String, hostname: String, screen: ScreenInfo) -> Self {
        Self {
            id,
            name,
            hostname,
            screen,
            status: DeviceStatus::Unknown,
            last_seen: None,
        }
    }

    pub fn with_status(mut self, status: DeviceStatus) -> Self {
        self.status = status;
        self
    }

    /// Update the last seen timestamp
    pub fn update_seen(&mut self) {
        self.last_seen = Some(super::protocol::timestamp_ms());
        self.status = DeviceStatus::Online;
    }

    /// Mark device as offline
    pub fn mark_offline(&mut self) {
        self.status = DeviceStatus::Offline;
    }

    /// Check if device is considered stale (not seen for a while)
    pub fn is_stale(&self, timeout: Duration) -> bool {
        if let Some(seen) = self.last_seen {
            let elapsed = Duration::from_millis(
                super::protocol::timestamp_ms().saturating_sub(seen)
            );
            elapsed > timeout
        } else {
            true
        }
    }

    /// Get display name
    pub fn display_name(&self) -> String {
        if self.name.is_empty() {
            format!("{} ({})", self.hostname, self.id)
        } else {
            format!("{} ({})", self.name, self.hostname)
        }
    }
}

/// Device position in the layout
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevicePosition {
    pub device_id: DeviceId,
    pub x: i32,
    pub y: i32,
    pub screen: ScreenInfo,
}

impl DevicePosition {
    pub fn new(device_id: DeviceId, x: i32, y: i32, screen: ScreenInfo) -> Self {
        Self {
            device_id,
            x,
            y,
            screen,
        }
    }

    /// Get the bounding rect of this device's screen
    pub fn rect(&self) -> (i32, i32, i32, i32) {
        (
            self.x,
            self.y,
            self.x + self.screen.width as i32,
            self.y + self.screen.height as i32,
        )
    }

    /// Check if a point is within this screen
    pub fn contains(&self, x: i32, y: i32) -> bool {
        let (left, top, right, bottom) = self.rect();
        x >= left && x < right && y >= top && y < bottom
    }
}

/// Screen layout configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenLayout {
    pub devices: Vec<DevicePosition>,
}

impl Default for ScreenLayout {
    fn default() -> Self {
        Self {
            devices: Vec::new(),
        }
    }
}

impl ScreenLayout {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a device to the layout
    pub fn add_device(&mut self, position: DevicePosition) {
        self.devices.push(position);
    }

    /// Remove a device from the layout
    pub fn remove_device(&mut self, device_id: &DeviceId) {
        self.devices.retain(|d| &d.device_id != device_id);
    }

    /// Find which device contains a point
    pub fn find_device_at(&self, x: i32, y: i32) -> Option<&DevicePosition> {
        self.devices.iter().find(|d| d.contains(x, y))
    }

    /// Find adjacent device in a direction
    pub fn find_adjacent(&self, from_device: &DeviceId, direction: Direction) -> Option<DeviceId> {
        let from_pos = self.devices.iter().find(|d| &d.device_id == from_device)?;

        match direction {
            Direction::Left => self.devices.iter()
                .find(|d| {
                    d.x + d.screen.width as i32 == from_pos.x
                        && d.y < from_pos.y + from_pos.screen.height as i32
                        && d.y + d.screen.height as i32 > from_pos.y
                })
                .map(|d| d.device_id),
            Direction::Right => self.devices.iter()
                .find(|d| {
                    d.x == from_pos.x + from_pos.screen.width as i32
                        && d.y < from_pos.y + from_pos.screen.height as i32
                        && d.y + d.screen.height as i32 > from_pos.y
                })
                .map(|d| d.device_id),
            Direction::Top => self.devices.iter()
                .find(|d| {
                    d.y + d.screen.height as i32 == from_pos.y
                        && d.x < from_pos.x + from_pos.screen.width as i32
                        && d.x + d.screen.width as i32 > from_pos.x
                })
                .map(|d| d.device_id),
            Direction::Bottom => self.devices.iter()
                .find(|d| {
                    d.y == from_pos.y + from_pos.screen.height as i32
                        && d.x < from_pos.x + from_pos.screen.width as i32
                        && d.x + d.screen.width as i32 > from_pos.x
                })
                .map(|d| d.device_id),
        }
    }

    /// Auto-arrange devices in a row
    pub fn auto_arrange_horizontal(&mut self) {
        let mut x = 0;
        for device in &mut self.devices {
            device.x = x;
            device.y = 0;
            x += device.screen.width as i32;
        }
    }

    /// Auto-arrange devices in a column
    pub fn auto_arrange_vertical(&mut self) {
        let mut y = 0;
        for device in &mut self.devices {
            device.x = 0;
            device.y = y;
            y += device.screen.height as i32;
        }
    }
}

/// Device registry managing all known devices
#[derive(Debug, Clone)]
pub struct DeviceRegistry {
    devices: HashMap<DeviceId, Device>,
    layout: ScreenLayout,
    local_device_id: DeviceId,
}

impl DeviceRegistry {
    pub fn new(local_device_id: DeviceId) -> Self {
        Self {
            devices: HashMap::new(),
            layout: ScreenLayout::new(),
            local_device_id,
        }
    }

    /// Get the local device ID
    pub fn local_id(&self) -> DeviceId {
        self.local_device_id
    }

    /// Add or update a device
    pub fn upsert_device(&mut self, device: Device) {
        let id = device.id;
        self.devices.insert(id, device);
    }

    /// Get a device by ID
    pub fn get(&self, id: &DeviceId) -> Option<&Device> {
        self.devices.get(id)
    }

    /// Get a mutable device by ID
    pub fn get_mut(&mut self, id: &DeviceId) -> Option<&mut Device> {
        self.devices.get_mut(id)
    }

    /// Remove a device
    pub fn remove(&mut self, id: &DeviceId) -> Option<Device> {
        self.devices.remove(id)
    }

    /// Get all online devices
    pub fn online_devices(&self) -> Vec<&Device> {
        self.devices
            .values()
            .filter(|d| d.status.is_online())
            .collect()
    }

    /// Get all devices except local
    pub fn remote_devices(&self) -> Vec<&Device> {
        self.devices
            .values()
            .filter(|d| d.id != self.local_device_id)
            .collect()
    }

    /// Get the screen layout
    pub fn layout(&self) -> &ScreenLayout {
        &self.layout
    }

    /// Update the screen layout
    pub fn set_layout(&mut self, layout: ScreenLayout) {
        self.layout = layout;
    }

    /// Clean up stale devices
    pub fn cleanup_stale(&mut self, timeout: Duration) -> Vec<DeviceId> {
        let stale: Vec<DeviceId> = self.devices
            .iter()
            .filter(|(_, d)| d.id != self.local_device_id && d.is_stale(timeout))
            .map(|(id, _)| *id)
            .collect();

        for id in &stale {
            if let Some(device) = self.devices.get_mut(id) {
                device.mark_offline();
            }
        }

        stale
    }

    /// Get device count
    pub fn count(&self) -> usize {
        self.devices.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_device_status() {
        assert!(DeviceStatus::Online.is_online());
        assert!(!DeviceStatus::Offline.is_online());
        assert!(!DeviceStatus::Unknown.is_online());
    }

    #[test]
    fn test_device_display_name() {
        let device = Device::new(
            Uuid::new_v4(),
            "My PC".to_string(),
            "my-pc".to_string(),
            ScreenInfo::primary(),
        );
        assert!(device.display_name().contains("My PC"));
        assert!(device.display_name().contains("my-pc"));
    }

    #[test]
    fn test_screen_layout_adjacent() {
        let mut layout = ScreenLayout::new();

        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();

        layout.add_device(DevicePosition::new(
            id1,
            0,
            0,
            ScreenInfo::new(0, 0, 1920, 1080),
        ));
        layout.add_device(DevicePosition::new(
            id2,
            1920,
            0,
            ScreenInfo::new(0, 0, 1920, 1080),
        ));

        assert_eq!(layout.find_adjacent(&id1, Direction::Right), Some(id2));
        assert_eq!(layout.find_adjacent(&id2, Direction::Left), Some(id1));
    }

    #[test]
    fn test_device_registry() {
        let local_id = Uuid::new_v4();
        let mut registry = DeviceRegistry::new(local_id);

        let device = Device::new(
            Uuid::new_v4(),
            "Remote".to_string(),
            "remote".to_string(),
            ScreenInfo::primary(),
        );

        registry.upsert_device(device.clone());
        assert_eq!(registry.count(), 1);
        assert_eq!(registry.local_id(), local_id);
    }
}
