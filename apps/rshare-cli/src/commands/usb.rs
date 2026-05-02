//! Experimental USB forwarding commands.

use anyhow::Result;
use clap::Subcommand;
use uuid::Uuid;

use crate::output::{header, kv, table_header, table_row, warning};

#[derive(Subcommand)]
pub enum UsbCommands {
    /// List local and remotely advertised USB devices
    List,

    /// Run a remote USB device-descriptor control transfer probe
    Probe {
        /// Target R-ShareMouse device id
        device_id: Uuid,

        /// Remote USB bus id/path printed by `rshare usb list`
        bus_id: String,
    },
}

pub async fn execute(command: UsbCommands) -> Result<()> {
    match command {
        UsbCommands::List => list_usb_devices().await,
        UsbCommands::Probe { device_id, bus_id } => {
            probe_remote_descriptor(device_id, bus_id).await
        }
    }
}

async fn list_usb_devices() -> Result<()> {
    let snapshot = rshare_core::daemon_client::request_local_controls().await?;

    header("Local USB Devices");
    if snapshot.usb_devices.is_empty() {
        warning("No local WinUSB-compatible USB device interfaces reported");
    } else {
        table_header(&["VID:PID", "CFG", "EP", "BUS ID"]);
        for device in &snapshot.usb_devices {
            table_row(&[
                &format_usb_vid_pid(device),
                &device.configurations.len().to_string(),
                &device.endpoints.len().to_string(),
                &device.bus_id,
            ]);
        }
    }

    println!();
    header("Remote USB Devices");
    if snapshot.remote_usb_devices.is_empty() {
        warning("No remote USB devices advertised by connected peers");
    } else {
        table_header(&["DEVICE", "STATUS", "VID:PID", "CFG", "EP", "BUS ID"]);
        for remote in &snapshot.remote_usb_devices {
            let name = remote
                .device_name
                .clone()
                .unwrap_or_else(|| remote.device_id.to_string());
            let status = if remote.connected {
                "connected"
            } else {
                "offline"
            };
            table_row(&[
                &name,
                status,
                &format_usb_vid_pid(&remote.device),
                &remote.device.configurations.len().to_string(),
                &remote.device.endpoints.len().to_string(),
                &remote.device.bus_id,
            ]);
        }
    }

    Ok(())
}

async fn probe_remote_descriptor(device_id: Uuid, bus_id: String) -> Result<()> {
    header("USB Descriptor Probe");
    let result =
        rshare_core::daemon_client::request_remote_usb_descriptor_probe(device_id, bus_id).await?;

    kv("Status", &format!("{:?}", result.status));
    kv("Message", &result.message);
    kv("Device ID", &result.device_id.to_string());
    kv("Bus ID", &result.bus_id);
    kv("Request ID", &result.request_id.to_string());
    kv("Transfer ID", &result.transfer_id.to_string());
    if let Some(session_id) = result.session_id {
        kv("Session ID", &session_id.to_string());
    }
    if let Some(elapsed_ms) = result.elapsed_ms {
        kv("Elapsed", &format!("{elapsed_ms} ms"));
    }
    if let Some(actual_length) = result.actual_length {
        kv("Actual Length", &actual_length.to_string());
    }
    if let Some(descriptor) = result.descriptor {
        kv("VID:PID", &format_usb_vid_pid(&descriptor));
        kv("USB BCD", &format!("0x{:04x}", descriptor.usb_version_bcd));
        kv(
            "Device BCD",
            &format!("0x{:04x}", descriptor.device_version_bcd),
        );
        kv("Class", &format!("0x{:02x}", descriptor.class_code));
        kv("Subclass", &format!("0x{:02x}", descriptor.subclass_code));
        kv("Protocol", &format!("0x{:02x}", descriptor.protocol_code));
    }
    if !result.descriptor_bytes.is_empty() {
        kv("Raw", &hex_bytes(&result.descriptor_bytes));
    }

    Ok(())
}

fn format_usb_vid_pid(device: &rshare_core::UsbDeviceDescriptor) -> String {
    format!("{:04x}:{:04x}", device.vendor_id, device.product_id)
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}
