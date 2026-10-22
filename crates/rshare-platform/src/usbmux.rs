//! Native usbmuxd protocol; selects only physical USB attachments, never Wi-Fi.
use anyhow::{bail, ensure, Context, Result};
use plist::{Dictionary, Value};
use rshare_core::usb_display::{UsbDisplayDevice, PORT};
use std::{
    io::{Cursor, Read, Write},
    os::unix::net::UnixStream,
    time::Duration,
};

const MAX_RESPONSE: usize = 1024 * 1024;
fn open() -> Result<UnixStream> {
    let stream =
        UnixStream::connect("/var/run/usbmuxd").context("Apple USB service is unavailable")?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    Ok(stream)
}
fn message(kind: &str) -> Dictionary {
    let mut fields = Dictionary::new();
    fields.insert("MessageType".into(), kind.into());
    fields.insert("ClientVersionString".into(), "RShareMouse-USB-1".into());
    fields.insert("ProgName".into(), "R-ShareMouse".into());
    fields.insert("kLibUSBMuxVersion".into(), 3u64.into());
    fields
}
fn read_response(mut input: impl Read) -> Result<Value> {
    let mut header = [0u8; 16];
    input.read_exact(&mut header)?;
    let word = |i| u32::from_le_bytes(header[i..i + 4].try_into().unwrap());
    let length = word(0) as usize;
    ensure!(
        (16..=MAX_RESPONSE).contains(&length) && word(4) == 1 && word(8) == 8 && word(12) == 1,
        "Invalid usbmuxd response header"
    );
    let mut bytes = vec![0; length - 16];
    input.read_exact(&mut bytes)?;
    Ok(Value::from_reader(Cursor::new(bytes))?)
}
fn exchange(stream: &mut UnixStream, fields: Dictionary) -> Result<Value> {
    let mut bytes = Vec::new();
    Value::Dictionary(fields).to_writer_xml(&mut bytes)?;
    for word in [(bytes.len() + 16) as u32, 1, 8, 1] {
        stream.write_all(&word.to_le_bytes())?;
    }
    stream.write_all(&bytes)?;
    read_response(stream)
}
fn usb_devices(value: &Value) -> Result<Vec<(u32, UsbDisplayDevice)>> {
    let devices = value
        .as_dictionary()
        .and_then(|d| d.get("DeviceList"))
        .and_then(Value::as_array)
        .context("usbmuxd returned no device list")?;
    let mut result = vec![];
    for entry in devices {
        let Some(entry) = entry.as_dictionary() else {
            continue;
        };
        let Some(props) = entry.get("Properties").and_then(Value::as_dictionary) else {
            continue;
        };
        if props.get("ConnectionType").and_then(Value::as_string) != Some("USB") {
            continue;
        }
        let id = entry
            .get("DeviceID")
            .and_then(Value::as_unsigned_integer)
            .and_then(|id| u32::try_from(id).ok());
        let serial = props.get("SerialNumber").and_then(Value::as_string);
        if let (Some(id), Some(serial)) = (id, serial) {
            result.push((
                id,
                UsbDisplayDevice {
                    udid: serial.into(),
                    connection: "USB".into(),
                },
            ));
        }
    }
    Ok(result)
}
pub fn list() -> Result<Vec<UsbDisplayDevice>> {
    let value = exchange(&mut open()?, message("ListDevices"))?;
    Ok(usb_devices(&value)?
        .into_iter()
        .map(|(_, device)| device)
        .collect())
}
pub fn connect(udid: &str) -> Result<UnixStream> {
    let value = exchange(&mut open()?, message("ListDevices"))?;
    let Some((id, _)) = usb_devices(&value)?
        .into_iter()
        .find(|(_, device)| device.udid == udid)
    else {
        bail!("Selected iPhone/iPad is not attached by USB");
    };
    let mut stream = open()?;
    let mut fields = message("Connect");
    fields.insert("DeviceID".into(), (id as u64).into());
    fields.insert("PortNumber".into(), (PORT.to_be() as u64).into());
    let response = exchange(&mut stream, fields)?;
    let code = response
        .as_dictionary()
        .and_then(|d| d.get("Number"))
        .and_then(Value::as_unsigned_integer);
    ensure!(
        code == Some(0),
        "Phone USB receiver is not listening (usbmuxd {code:?}); open USB display on the phone"
    );
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excludes_wireless_and_missing_connection_types() {
        let value = Value::from_reader_xml(br#"<plist><dict><key>DeviceList</key><array>
        <dict><key>DeviceID</key><integer>1</integer><key>Properties</key><dict><key>ConnectionType</key><string>USB</string><key>SerialNumber</key><string>wired</string></dict></dict>
        <dict><key>DeviceID</key><integer>2</integer><key>Properties</key><dict><key>ConnectionType</key><string>Network</string><key>SerialNumber</key><string>wifi</string></dict></dict>
        </array></dict></plist>"#.as_slice()).unwrap();
        let devices = usb_devices(&value).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].1.udid, "wired");
    }
    #[test]
    fn rejects_unbounded_response_before_allocation() {
        let mut bytes = vec![];
        for word in [u32::MAX, 1, 8, 1] {
            bytes.extend(word.to_le_bytes());
        }
        assert!(read_response(bytes.as_slice()).is_err());
    }
}
