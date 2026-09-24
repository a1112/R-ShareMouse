//! Bounded legacy host worker. Native I/O never executes on a Tokio worker.
//! This is a migration boundary, not an overlapped per-device actor implementation.
use rshare_core::{usb_authority::UsbOwner, Message};
use rshare_platform::ExperimentalUsbHostRuntime;

pub fn is_host_request(message: &Message) -> bool {
    matches!(
        message,
        Message::UsbDeviceClaimRequest { .. }
            | Message::UsbTransfer { .. }
            | Message::UsbDeviceRelease { .. }
            | Message::UsbDeviceReset { .. }
            | Message::UsbTransferCancel { .. }
    )
}
pub fn is_usb_message(message: &Message) -> bool {
    is_host_request(message)
        || matches!(
            message,
            Message::UsbDeviceAttached { .. }
                | Message::UsbDeviceDetached { .. }
                | Message::UsbDeviceClaimResponse { .. }
                | Message::UsbTransferComplete { .. }
                | Message::UsbForwardingError { .. }
                | Message::UsbFlowControl { .. }
        )
}
pub fn bounded(message: &Message) -> bool {
    match message {
        Message::UsbTransfer { transfer } => {
            transfer.data.len() <= 1024 * 1024
                && transfer.expected_length.is_none_or(|n| n <= 1024 * 1024)
                && transfer.bus_id.len() <= 128
                && transfer.flags.len() <= 8
                && transfer.iso_packets.is_empty()
        }
        Message::UsbDeviceClaimRequest { request } => {
            request.bus_id.len() <= 128 && request.interface_numbers.len() <= 8
        }
        Message::UsbDeviceRelease { bus_id, reason, .. }
        | Message::UsbTransferCancel { bus_id, reason, .. } => {
            bus_id.len() <= 128 && reason.len() <= 1024
        }
        Message::UsbDeviceReset { bus_id, .. } => bus_id.len() <= 128,
        _ => false,
    }
}
pub fn execute(
    runtime: &mut ExperimentalUsbHostRuntime,
    owner: UsbOwner,
    message: Message,
) -> Option<Message> {
    let result = match message {
        Message::UsbDeviceClaimRequest { request } => {
            return Some(Message::UsbDeviceClaimResponse {
                response: runtime.claim_device(owner, request),
            })
        }
        Message::UsbTransfer { transfer } => runtime
            .submit_transfer(owner, &transfer)
            .map(|c| {
                Some(Message::UsbTransferComplete {
                    transfer_id: c.transfer_id,
                    bus_id: c.bus_id,
                    status: c.status,
                    transfer_status: c.transfer_status,
                    endpoint_address: c.endpoint_address,
                    transfer_kind: c.transfer_kind,
                    actual_length: c.actual_length,
                    data: c.data,
                    iso_packets: vec![],
                })
            })
            .map_err(|e| (Some(transfer.bus_id), e)),
        Message::UsbDeviceRelease {
            session_id, bus_id, ..
        } => runtime
            .release_device(owner, session_id, &bus_id)
            .map(|_| None)
            .map_err(|e| (Some(bus_id), e)),
        Message::UsbDeviceReset {
            session_id,
            bus_id,
            reset_kind,
        } => runtime
            .reset_device(owner, session_id, &bus_id, reset_kind)
            .map(|_| None)
            .map_err(|e| (Some(bus_id), e)),
        Message::UsbTransferCancel {
            transfer_id,
            bus_id,
            ..
        } => runtime
            .cancel_transfer(owner, transfer_id, &bus_id)
            .map(|_| None)
            .map_err(|e| (Some(bus_id), e)),
        _ => return None,
    };
    result.unwrap_or_else(|(bus_id, error)| {
        Some(Message::UsbForwardingError {
            bus_id,
            message: error.to_string(),
        })
    })
}
