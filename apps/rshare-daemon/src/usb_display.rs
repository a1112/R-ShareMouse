//! Daemon-owned native USB/LAN display lifecycle. Transport is explicitly selected.
use anyhow::{bail, Result};
use rshare_core::usb_display::{UsbDisplayDevice, UsbDisplayStart, UsbDisplayStatus};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[derive(Default)]
struct Shared {
    status: Mutex<UsbDisplayStatus>,
    cancelled: AtomicBool,
    #[cfg(target_os = "macos")]
    socket: Mutex<Option<DisplayStream>>,
    #[cfg(target_os = "macos")]
    cleanup_error: Mutex<Option<String>>,
}
#[derive(Default)]
pub struct Manager {
    shared: Arc<Shared>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

#[cfg(target_os = "macos")]
enum DisplayStream {
    Usb(std::os::unix::net::UnixStream),
    Lan(rshare_platform::lan_display::LanStream),
}
#[cfg(target_os = "macos")]
impl DisplayStream {
    fn stop(&self) {
        match self {
            Self::Usb(s) => {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
            Self::Lan(s) => s.cancel(),
        }
    }
    fn cancellation_handle(&self) -> Result<Self> {
        Ok(match self {
            Self::Usb(s) => Self::Usb(s.try_clone()?),
            Self::Lan(s) => Self::Lan(s.clone()),
        })
    }
}
#[cfg(target_os = "macos")]
impl std::io::Read for DisplayStream {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Usb(s) => s.read(b),
            Self::Lan(s) => s.read(b),
        }
    }
}
#[cfg(target_os = "macos")]
impl std::io::Write for DisplayStream {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Usb(s) => s.write(b),
            Self::Lan(s) => s.write(b),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn validate_ack(body: &[u8], sequence: u64, previous_decoded: u64) -> Result<u64> {
    use anyhow::{ensure, Context};
    ensure!(body.len() <= 4096, "Display acknowledgment too large");
    let ack: serde_json::Value = serde_json::from_slice(body)?;
    let decoded = ack["frames_decoded"]
        .as_u64()
        .context("Missing decoder counter")?;
    ensure!(
        decoded <= sequence && ack["sequence"].as_u64() == Some(sequence),
        "Invalid display frame sequence"
    );
    ensure!(
        decoded >= previous_decoded,
        "Display decoder counter regressed"
    );
    Ok(decoded)
}
impl Manager {
    pub fn devices(&self) -> Result<Vec<UsbDisplayDevice>> {
        #[cfg(target_os = "macos")]
        {
            rshare_platform::usbmux::list()
        }
        #[cfg(not(target_os = "macos"))]
        {
            bail!("Native USB display currently requires a macOS host")
        }
    }
    pub fn status(&self) -> UsbDisplayStatus {
        let mut result = self
            .shared
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if result.phase.is_empty() {
            result.phase = "Stopped".into();
        }
        result
    }
    pub fn start(&self, request: UsbDisplayStart) -> Result<UsbDisplayStatus> {
        request.validate().map_err(anyhow::Error::msg)?;
        self.start_internal(request, None)
    }
    pub fn start_lan(
        &self,
        request: rshare_core::usb_display::LanDisplayStart,
    ) -> Result<UsbDisplayStatus> {
        let (host, key) = request.parse().map_err(anyhow::Error::msg)?;
        self.start_internal(
            UsbDisplayStart {
                udid: host.to_string(),
                code: request
                    .endpoint
                    .trim()
                    .split_once('#')
                    .unwrap()
                    .1
                    .to_ascii_lowercase(),
                width: request.width,
                height: request.height,
                fps: request.fps,
            },
            Some((host, key)),
        )
    }
    fn start_internal(
        &self,
        request: UsbDisplayStart,
        lan: Option<(std::net::Ipv4Addr, [u8; 32])>,
    ) -> Result<UsbDisplayStatus> {
        #[cfg(not(target_os = "macos"))]
        {
            bail!("Native USB display currently requires a macOS host")
        }
        #[cfg(target_os = "macos")]
        {
            let mut worker = self.worker.lock().unwrap_or_else(|e| e.into_inner());
            if worker.as_ref().is_some_and(|job| !job.is_finished()) {
                bail!("A Native display session is already active");
            }
            if let Some(job) = worker.take() {
                let _ = job.join();
            }
            if self
                .shared
                .cleanup_error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some()
            {
                bail!("The previous virtual display could not be removed; restart the daemon before retrying");
            }
            self.shared.cancelled.store(false, Ordering::Release);
            *self.shared.status.lock().unwrap_or_else(|e| e.into_inner()) = UsbDisplayStatus {
                phase: "Starting".into(),
                udid: Some(request.udid.clone()),
                transport: Some(
                    if lan.is_some() {
                        "lan-tls"
                    } else {
                        "usbmuxd-usb"
                    }
                    .into(),
                ),
                ..Default::default()
            };
            let shared = self.shared.clone();
            let job = std::thread::Builder::new()
                .name("usb-display".into())
                .spawn(move || {
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run(&shared, request, lan)
                    }));
                    shared
                        .socket
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .take();
                    let cleanup_error = shared
                        .cleanup_error
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone();
                    let mut status = shared.status.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(error) = cleanup_error {
                        status.phase = "Failed".into();
                        status.message = Some(match &outcome {
                            Ok(Err(cause)) => format!("{cause:#}; {error}"),
                            _ => error,
                        });
                        return;
                    }
                    status.display_id = None;
                    if shared.cancelled.load(Ordering::Acquire) {
                        status.phase = "Stopped".into();
                        status.message = None;
                    } else {
                        status.phase = "Failed".into();
                        status.message = Some(match outcome {
                            Ok(Err(error)) => format!("{error:#}"),
                            Ok(Ok(())) => "Native display session ended".into(),
                            Err(_) => "USB display worker stopped unexpectedly".into(),
                        });
                    }
                });
            match job {
                Ok(job) => *worker = Some(job),
                Err(error) => {
                    let mut status = self.shared.status.lock().unwrap_or_else(|e| e.into_inner());
                    status.phase = "Failed".into();
                    status.message = Some(error.to_string());
                    return Err(error.into());
                }
            }
            Ok(self.status())
        }
    }
    pub fn stop(&self) -> UsbDisplayStatus {
        self.shared.cancelled.store(true, Ordering::Release);
        #[cfg(target_os = "macos")]
        if let Some(socket) = self
            .shared
            .socket
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            socket.stop();
        }
        let mut status = self.shared.status.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(
            status.phase.as_str(),
            "Starting" | "WaitingForVideo" | "Streaming"
        ) {
            status.phase = "Stopping".into();
        }
        drop(status);
        self.status()
    }
    pub fn shutdown(&self) {
        self.stop();
        if let Some(worker) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = worker.join();
        }
    }
}
impl Drop for Manager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(target_os = "macos")]
fn run(
    shared: &Shared,
    request: UsbDisplayStart,
    lan: Option<(std::net::Ipv4Addr, [u8; 32])>,
) -> Result<()> {
    use anyhow::{ensure, Context};
    use rshare_core::{
        usb_display::*, VirtualDisplayCreateRequest, VirtualDisplayOperationStatus,
        VirtualDisplayRemoveRequest,
    };
    use rshare_platform::{macos_display::H264Capture, virtual_display};
    use std::time::{Duration, Instant};
    struct DisplayGuard<'a>(String, &'a Shared);
    impl Drop for DisplayGuard<'_> {
        fn drop(&mut self) {
            if let Err(error) =
                virtual_display::remove_virtual_display(&VirtualDisplayRemoveRequest {
                    id: self.0.clone(),
                })
            {
                tracing::error!("Virtual display cleanup failed: {error:#}");
                *self
                    .1
                    .cleanup_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(format!(
                    "Virtual display cleanup failed: {error:#}; restart the daemon"
                ));
            }
        }
    }
    let mut socket = if let Some((host, key)) = lan {
        DisplayStream::Lan(rshare_platform::lan_display::LanStream::connect(
            host, &key,
        )?)
    } else {
        DisplayStream::Usb(rshare_platform::usbmux::connect(&request.udid)?)
    };
    *shared.socket.lock().unwrap_or_else(|e| e.into_inner()) = Some(socket.cancellation_handle()?);
    let hello = serde_json::json!({"version":1,"code":request.code,"width":request.width,"height":request.height,"fps":request.fps});
    write_packet(&mut socket, HELLO, &serde_json::to_vec(&hello)?)?;
    let (kind, body) = read_packet(&mut socket)
        .context("Phone pairing failed; check its current pairing code or LAN link")?;
    let response: serde_json::Value = serde_json::from_slice(&body)?;
    ensure!(
        kind == ACK && response["accepted"] == true && response["version"] == 1,
        "Phone rejected display pairing"
    );
    ensure!(
        !shared.cancelled.load(Ordering::Acquire),
        "USB start cancelled"
    );
    let id = format!("usb-display-{}", rshare_core::DeviceId::new_v4());
    let display = virtual_display::create_virtual_display(&VirtualDisplayCreateRequest {
        id: Some(id.clone()),
        width: request.width,
        height: request.height,
        refresh_rate_millihz: Some(60000),
        name: Some(
            if lan.is_some() {
                "R-ShareMouse LAN iPhone/iPad"
            } else {
                "R-ShareMouse USB iPhone/iPad"
            }
            .into(),
        ),
    })?;
    ensure!(
        display.status == VirtualDisplayOperationStatus::Created,
        "Could not create virtual display: {:?}",
        display.message
    );
    let _display_guard = DisplayGuard(id, shared);
    let display_id = display
        .display
        .and_then(|display| display.display_id)
        .context("Virtual display has no OS id")?;
    {
        let mut status = shared.status.lock().unwrap_or_else(|e| e.into_inner());
        status.display_id = Some(display_id.clone());
        status.phase = "WaitingForVideo".into();
    }
    let capture = H264Capture::start(&display_id, request.width, request.height, request.fps)?;
    let started = Instant::now();
    let mut heartbeat = Instant::now();
    let mut sequence = 0u64;
    while !shared.cancelled.load(Ordering::Acquire) {
        capture.check()?;
        let frame = match capture.frames.recv_timeout(Duration::from_millis(200)) {
            Ok(frame) => Some(frame),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
            Err(error) => bail!("Capture stopped: {error}"),
        };
        if frame.is_none() && heartbeat.elapsed() < Duration::from_millis(750) {
            continue;
        }
        if let Some(frame) = frame {
            sequence += 1;
            let mut body = sequence.to_be_bytes().to_vec();
            body.extend(frame);
            write_packet(&mut socket, VIDEO, &body).context("Video write failed")?;
            let mut status = shared.status.lock().unwrap_or_else(|e| e.into_inner());
            status.frames_sent = sequence;
            status.bytes_sent += (body.len() + 5) as u64;
        } else {
            ensure!(
                sequence > 0 || started.elapsed() < Duration::from_secs(10),
                "No screen frame arrived within ten seconds"
            );
            write_packet(&mut socket, PING, &[])?;
        }
        let (kind, body) = read_packet(&mut socket)
            .context("Receiver disconnected or stopped acknowledging video")?;
        ensure!(
            kind == ACK && body.len() <= 4096,
            "Invalid receiver acknowledgment"
        );
        let mut status = shared.status.lock().unwrap_or_else(|e| e.into_inner());
        let decoded = validate_ack(&body, sequence, status.frames_decoded)?;
        status.frames_decoded = decoded;
        if decoded > 0 {
            status.phase = "Streaming".into();
        }
        heartbeat = Instant::now();
    }
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    #[test]
    fn receiver_progress_must_be_bounded_monotonic_and_for_current_frame() {
        assert_eq!(
            validate_ack(br#"{"sequence":10,"frames_decoded":8}"#, 10, 7).unwrap(),
            8
        );
        for body in [
            br#"{"sequence":9,"frames_decoded":8}"#.as_slice(),
            br#"{"sequence":10,"frames_decoded":11}"#,
            br#"{"sequence":10,"frames_decoded":6}"#,
            br#"{"sequence":10}"#,
        ] {
            assert!(validate_ack(body, 10, 7).is_err());
        }
    }
    #[test]
    fn invalid_start_does_not_launch_worker_or_change_state() {
        let manager = Manager::default();
        assert!(manager
            .start(UsbDisplayStart {
                udid: "x".into(),
                code: "bad".into(),
                width: 1920,
                height: 1080,
                fps: 60
            })
            .is_err());
        assert_eq!(manager.status().phase, "Stopped");
        assert!(manager.worker.lock().unwrap().is_none());
    }
}
