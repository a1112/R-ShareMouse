//! Same-subnet Wake-on-LAN packet delivery.

use anyhow::{bail, Context, Result};
use if_addrs::{get_if_addrs, IfAddr};
use rshare_core::wake::normalize_mac;
use std::net::Ipv4Addr;
use tokio::net::UdpSocket;
use tokio::time::{sleep, Duration};

const WOL_PORT: u16 = 9;

pub fn magic_packet(mac: &str) -> Result<[u8; 102]> {
    let normalized = normalize_mac(mac).map_err(anyhow::Error::msg)?;
    let compact = normalized.replace(':', "");
    let mut bytes = [0u8; 6];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&compact[index * 2..index * 2 + 2], 16)?;
    }
    let mut packet = [0xFF; 102];
    for chunk in packet[6..].chunks_exact_mut(6) {
        chunk.copy_from_slice(&bytes);
    }
    Ok(packet)
}

pub fn broadcast_for(
    local: Ipv4Addr,
    mask: Ipv4Addr,
    target: Option<Ipv4Addr>,
) -> Option<Ipv4Addr> {
    let host_mask = !u32::from(mask);
    if host_mask.count_ones() < 2 || host_mask == u32::MAX || host_mask & (host_mask + 1) != 0 {
        return None;
    }
    let network = u32::from(local) & u32::from(mask);
    let broadcast_bits = network | host_mask;
    if target.is_some_and(|target| {
        let target = u32::from(target);
        target & u32::from(mask) != network || target == network || target == broadcast_bits
    }) {
        return None;
    }
    let broadcast = Ipv4Addr::from(broadcast_bits);
    (broadcast != local).then_some(broadcast)
}

/// Send three packets on each eligible local IPv4 interface. A successful
/// UDP send confirms delivery to the OS only, not that the target powered on.
pub async fn send_magic_packet(mac: &str, target_ip: Option<Ipv4Addr>) -> Result<usize> {
    let packet = magic_packet(mac)?;
    let interfaces = get_if_addrs().context("Failed to enumerate LAN interfaces")?;
    let mut destinations = Vec::new();
    for interface in interfaces {
        if !crate::discovery::is_candidate_interface(&interface) {
            continue;
        }
        let IfAddr::V4(address) = interface.addr else {
            continue;
        };
        let Some(broadcast) = broadcast_for(address.ip, address.netmask, target_ip) else {
            continue;
        };
        destinations.push((interface.name, address.ip, broadcast));
    }
    send_to_broadcasts(
        packet,
        destinations,
        |local, broadcast, packet| async move {
            let socket = UdpSocket::bind((local, 0)).await?;
            socket.set_broadcast(true)?;
            for copy in 0..3 {
                socket.send_to(&packet, (broadcast, WOL_PORT)).await?;
                if copy < 2 {
                    sleep(Duration::from_millis(100)).await;
                }
            }
            Ok::<_, std::io::Error>(())
        },
    )
    .await
}

async fn send_to_broadcasts<F, Fut>(
    packet: [u8; 102],
    destinations: Vec<(String, Ipv4Addr, Ipv4Addr)>,
    mut send: F,
) -> Result<usize>
where
    F: FnMut(Ipv4Addr, Ipv4Addr, [u8; 102]) -> Fut,
    Fut: std::future::Future<Output = std::io::Result<()>>,
{
    let mut sent_interfaces = 0;
    let mut last_error = None;
    for (name, local, broadcast) in destinations {
        let result = send(local, broadcast, packet).await;
        match result {
            Ok(()) => sent_interfaces += 1,
            Err(error) => {
                tracing::warn!(interface = %name, %error, "Wake packet send failed");
                last_error = Some(error);
            }
        }
    }
    if sent_interfaces == 0 {
        if let Some(error) = last_error {
            bail!("Could not send Wake-on-LAN packet: {error}");
        }
        bail!("No eligible IPv4 interface in the target subnet");
    }
    Ok(sent_interfaces)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[tokio::test]
    async fn no_eligible_interface_is_reported() {
        let packet = magic_packet("02:11:22:33:44:55").unwrap();
        let result = send_to_broadcasts(packet, Vec::new(), |_, _, _| async { Ok(()) }).await;
        assert!(result.unwrap_err().to_string().contains("No eligible IPv4"));
    }

    #[tokio::test]
    async fn packet_send_failure_is_reported() {
        let packet = magic_packet("02:11:22:33:44:55").unwrap();
        let destinations = vec![(
            "test0".to_string(),
            Ipv4Addr::new(192, 168, 1, 2),
            Ipv4Addr::new(192, 168, 1, 255),
        )];
        let result = send_to_broadcasts(packet, destinations, |_, _, _| async {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "blocked"))
        })
        .await;
        assert!(result.unwrap_err().to_string().contains("blocked"));
    }
}
