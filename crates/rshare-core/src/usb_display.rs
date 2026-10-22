//! USB display wire format v1: u32 big-endian body length, u8 kind, body.
//! H.264 body: u64 sequence, u16 SPS length, u16 PPS length, SPS, PPS, AVCC.
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const PORT: u16 = 27438;
pub const MAX_PACKET: usize = 8 * 1024 * 1024;
pub const HELLO: u8 = 1;
pub const VIDEO: u8 = 2;
pub const ACK: u8 = 3;
pub const PING: u8 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbDisplayDevice {
    pub udid: String,
    pub connection: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbDisplayStart {
    pub udid: String,
    pub code: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}
impl UsbDisplayStart {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.udid.is_empty() || self.udid.len() > 128 {
            return Err("Select a USB iPhone/iPad");
        }
        if self.code.len() != 6 || !self.code.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Enter the six-digit code shown on the phone");
        }
        if ![(1280, 720), (1920, 1080)].contains(&(self.width, self.height))
            || ![30, 60].contains(&self.fps)
        {
            return Err("USB display supports 1280x720 or 1920x1080 at 30/60 fps");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbDisplayStatus {
    pub phase: String,
    pub udid: Option<String>,
    pub display_id: Option<String>,
    pub transport: Option<String>,
    pub frames_sent: u64,
    pub frames_decoded: u64,
    pub bytes_sent: u64,
    pub message: Option<String>,
}

/// A fresh phone-generated TLS key travels only in the user-copied fragment.
/// Do not log this request or persist its endpoint.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanDisplayStart {
    pub endpoint: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}
impl std::fmt::Debug for LanDisplayStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanDisplayStart")
            .field("endpoint", &"[redacted]")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("fps", &self.fps)
            .finish()
    }
}
impl LanDisplayStart {
    pub fn parse(&self) -> Result<(std::net::Ipv4Addr, [u8; 32]), &'static str> {
        UsbDisplayStart {
            udid: "lan".into(),
            code: "000000".into(),
            width: self.width,
            height: self.height,
            fps: self.fps,
        }
        .validate()?;
        let value = self
            .endpoint
            .trim()
            .strip_prefix("rshare-display://")
            .ok_or("Paste the phone's LAN pairing link")?;
        let (address, secret) = value.split_once('#').ok_or("Missing LAN pairing key")?;
        let host: std::net::Ipv4Addr = address
            .strip_suffix(":27439")
            .ok_or("Invalid LAN port")?
            .parse()
            .map_err(|_| "Use the phone's private IPv4 LAN address")?;
        if !host.is_private() {
            return Err("Only private LAN addresses are allowed");
        }
        if secret.len() != 64 || !secret.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("Invalid LAN pairing key");
        }
        let mut key = [0; 32];
        for (index, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&secret[index * 2..index * 2 + 2], 16)
                .map_err(|_| "Invalid LAN pairing key")?;
        }
        Ok((host, key))
    }
}

pub fn write_packet(mut writer: impl Write, kind: u8, body: &[u8]) -> io::Result<()> {
    if body.len() > MAX_PACKET {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "USB packet too large",
        ));
    }
    let mut packet = Vec::with_capacity(body.len() + 5);
    packet.extend_from_slice(&(body.len() as u32).to_be_bytes());
    packet.push(kind);
    packet.extend_from_slice(body);
    writer.write_all(&packet)
}
pub fn read_packet(mut reader: impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut header = [0; 5];
    reader.read_exact(&mut header)?;
    let size = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
    if size > MAX_PACKET || ![HELLO, VIDEO, ACK, PING].contains(&header[4]) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid USB display frame",
        ));
    }
    let mut body = vec![0; size];
    reader.read_exact(&mut body)?;
    Ok((header[4], body))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lan_links_require_private_literal_address_strong_key_and_fixed_port() {
        let mut request = LanDisplayStart {
            endpoint: format!("rshare-display://192.168.2.10:27439#{}", "ab".repeat(32)),
            width: 1280,
            height: 720,
            fps: 30,
        };
        assert_eq!(request.parse().unwrap().1, [0xab; 32]);
        assert!(!format!("{request:?}").contains(&"ab".repeat(32)));
        for invalid in [
            "rshare-display://8.8.8.8:27439",
            "rshare-display://127.0.0.1:27439",
            "rshare-display://phone.local:27439",
            "rshare-display://192.168.2.10:443",
        ] {
            request.endpoint = format!("{invalid}#{}", "ab".repeat(32));
            assert!(request.parse().is_err());
        }
        request.endpoint = "rshare-display://192.168.2.10:27439#123456".into();
        assert!(request.parse().is_err());
    }
    #[test]
    fn framing_preserves_binary_and_coalesced_frames() {
        let mut bytes = vec![];
        write_packet(&mut bytes, VIDEO, &[0, 255, 0, 1]).unwrap();
        write_packet(&mut bytes, PING, &[]).unwrap();
        let mut input = bytes.as_slice();
        assert_eq!(
            read_packet(&mut input).unwrap(),
            (VIDEO, vec![0, 255, 0, 1])
        );
        assert_eq!(read_packet(input).unwrap(), (PING, vec![]));
    }
    #[test]
    fn rejects_oversize_unknown_and_truncated_frames() {
        for bytes in [
            vec![0, 128, 0, 1, VIDEO],
            vec![0, 0, 0, 0, 255],
            vec![0, 0, 0, 2, VIDEO, 1],
        ] {
            assert!(read_packet(bytes.as_slice()).is_err());
        }
    }
    #[test]
    fn refuses_invalid_pairing_and_capture_dimensions() {
        let mut request = UsbDisplayStart {
            udid: "phone".into(),
            code: "123456".into(),
            width: 1920,
            height: 1080,
            fps: 60,
        };
        assert!(request.validate().is_ok());
        request.code = "12x456".into();
        assert!(request.validate().is_err());
        request.code = "123456".into();
        request.width = u32::MAX;
        assert!(request.validate().is_err());
    }
}
