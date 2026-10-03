//! Validate the server PID of the actual connected loopback socket before bytes are sent.
use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
};
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
};

fn matches(row: &MIB_TCPROW_OWNER_PID, local: SocketAddr, peer: SocketAddr) -> bool {
    row.dwState == 5
        && Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes()) == peer.ip()
        && u16::from_be(row.dwLocalPort as u16) == peer.port()
        && Ipv4Addr::from(row.dwRemoteAddr.to_ne_bytes()) == local.ip()
        && u16::from_be(row.dwRemotePort as u16) == local.port()
}

pub fn require_connection_owner(
    local: SocketAddr,
    peer: SocketAddr,
    expected: u32,
) -> io::Result<()> {
    if expected == 0
        || !local.is_ipv4()
        || !peer.is_ipv4()
        || !local.ip().is_loopback()
        || !peer.ip().is_loopback()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Owned daemon IPC requires an IPv4 loopback connection",
        ));
    }
    for _ in 0..3 {
        let mut size = 0;
        let result =
            unsafe { GetExtendedTcpTable(None, &mut size, false, 2, TCP_TABLE_OWNER_PID_ALL, 0) };
        if result != 122 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        if !(4..=16 * 1024 * 1024).contains(&size) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "TCP owner table size is invalid",
            ));
        }
        // DWORD storage provides the table's required alignment.
        let mut words = vec![0u32; (size as usize).div_ceil(4)];
        let result = unsafe {
            GetExtendedTcpTable(
                Some(words.as_mut_ptr().cast()),
                &mut size,
                false,
                2,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        };
        if result == 122 {
            continue;
        }
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        let count = words[0] as usize;
        let offset = std::mem::offset_of!(MIB_TCPTABLE_OWNER_PID, table);
        let bytes = count
            .checked_mul(std::mem::size_of::<MIB_TCPROW_OWNER_PID>())
            .and_then(|bytes| bytes.checked_add(offset))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "TCP owner table overflow")
            })?;
        if bytes > size as usize || bytes > words.len() * 4 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Truncated TCP owner table",
            ));
        }
        for index in 0..count {
            let row = unsafe {
                std::ptr::read_unaligned(
                    words
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset + index * std::mem::size_of::<MIB_TCPROW_OWNER_PID>())
                        .cast::<MIB_TCPROW_OWNER_PID>(),
                )
            };
            if matches(&row, local, peer) {
                return crate::preview_profile::require_owned_pid(expected, row.dwOwningPid);
            }
        }
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "Connected IPC socket has no verifiable server process",
        ));
    }
    Err(io::Error::new(
        io::ErrorKind::Other,
        "TCP owner table kept changing",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_loopback_connection_belongs_to_its_server_process() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_server, _) = listener.accept().unwrap();
        require_connection_owner(
            client.local_addr().unwrap(),
            client.peer_addr().unwrap(),
            std::process::id(),
        )
        .unwrap();
    }
    #[test]
    fn wrong_owner_is_rejected_on_the_connected_socket() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_server, _) = listener.accept().unwrap();
        assert!(require_connection_owner(
            client.local_addr().unwrap(),
            client.peer_addr().unwrap(),
            std::process::id() + 1
        )
        .is_err());
    }
    #[test]
    fn matching_requires_both_connected_endpoints_in_network_byte_order() {
        let local: SocketAddr = "127.0.0.1:48123".parse().unwrap();
        let peer: SocketAddr = "127.0.0.1:53111".parse().unwrap();
        let row = MIB_TCPROW_OWNER_PID {
            dwState: 5,
            dwLocalAddr: u32::from_ne_bytes([127, 0, 0, 1]),
            dwLocalPort: (peer.port().to_be()) as u32,
            dwRemoteAddr: u32::from_ne_bytes([127, 0, 0, 1]),
            dwRemotePort: (local.port().to_be()) as u32,
            dwOwningPid: 42,
        };
        assert!(matches(&row, local, peer));
        assert!(!matches(&row, peer, local));
        assert!(!matches(&row, "127.0.0.1:48124".parse().unwrap(), peer));
    }
}
