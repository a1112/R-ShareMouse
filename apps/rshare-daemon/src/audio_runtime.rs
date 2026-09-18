use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rshare_core::{
    AudioFormat, AudioFramePayload, AudioSampleFormat, DeviceId, LocalAudioCaptureSource,
};
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc as std_mpsc, Arc, Mutex,
};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

const DEFAULT_FRAME_MS: u16 = 20;
const MAX_RENDER_BUFFER_MS: u32 = 300;
const CAPTURE_QUEUE_FRAMES: usize = 5;

#[derive(Debug)]
pub struct CapturedAudioFrame {
    pub captured_at: std::time::Instant,
    pub frame: AudioFramePayload,
    pub level_peak: u8,
    pub level_rms: u8,
}

impl CapturedAudioFrame {
    pub fn is_stale(&self) -> bool {
        self.captured_at.elapsed()
            > std::time::Duration::from_millis(
                CAPTURE_QUEUE_FRAMES as u64 * u64::from(DEFAULT_FRAME_MS),
            )
    }
}

#[derive(Clone)]
pub struct AudioRuntimeHandle {
    tx: std_mpsc::Sender<AudioRuntimeCommand>,
}

pub struct StartedAudioCapture {
    pub format: AudioFormat,
    pub rx: mpsc::Receiver<CapturedAudioFrame>,
}

enum AudioRuntimeCommand {
    StartCapture {
        source: LocalAudioCaptureSource,
        endpoint_name: Option<String>,
        stream_id: DeviceId,
        response: std_mpsc::Sender<std::result::Result<StartedAudioCapture, String>>,
    },
    StopCapture,
    StartRender {
        stream_id: DeviceId,
        format: AudioFormat,
        response: std_mpsc::Sender<std::result::Result<AudioRenderStats, String>>,
    },
    PushFrame {
        frame: AudioFramePayload,
        response: std_mpsc::Sender<std::result::Result<AudioRenderStats, String>>,
    },
    StopRender,
    Shutdown,
}

impl AudioRuntimeHandle {
    pub fn start() -> Result<Self> {
        let (tx, rx) = std_mpsc::channel();
        std::thread::Builder::new()
            .name("rshare-audio-runtime".to_string())
            .spawn(move || run_audio_runtime(rx))
            .context("Failed to start audio runtime thread")?;
        Ok(Self { tx })
    }

    pub fn start_capture(
        &self,
        source: LocalAudioCaptureSource,
        endpoint_name: Option<&str>,
        stream_id: DeviceId,
    ) -> Result<StartedAudioCapture> {
        self.request(|response| AudioRuntimeCommand::StartCapture {
            source,
            endpoint_name: endpoint_name.map(str::to_string),
            stream_id,
            response,
        })
    }

    pub fn stop_capture(&self) {
        let _ = self.tx.send(AudioRuntimeCommand::StopCapture);
    }

    pub fn start_render(
        &self,
        stream_id: DeviceId,
        format: AudioFormat,
    ) -> Result<AudioRenderStats> {
        self.request(|response| AudioRuntimeCommand::StartRender {
            stream_id,
            format,
            response,
        })
    }

    pub fn push_frame(&self, frame: &AudioFramePayload) -> Result<AudioRenderStats> {
        self.request(|response| AudioRuntimeCommand::PushFrame {
            frame: frame.clone(),
            response,
        })
    }

    pub fn stop_render(&self) {
        let _ = self.tx.send(AudioRuntimeCommand::StopRender);
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(AudioRuntimeCommand::Shutdown);
    }

    fn request<T>(
        &self,
        command: impl FnOnce(std_mpsc::Sender<std::result::Result<T, String>>) -> AudioRuntimeCommand,
    ) -> Result<T> {
        let (response_tx, response_rx) = std_mpsc::channel();
        self.tx
            .send(command(response_tx))
            .map_err(|error| anyhow!("Audio runtime thread is unavailable: {error}"))?;
        match response_rx.recv() {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(anyhow!(error)),
            Err(error) => Err(anyhow!("Audio runtime response channel closed: {error}")),
        }
    }
}

fn run_audio_runtime(rx: std_mpsc::Receiver<AudioRuntimeCommand>) {
    let mut _capture: Option<AudioCaptureSession> = None;
    let mut render = AudioRenderRuntime::new();

    while let Ok(command) = rx.recv() {
        match command {
            AudioRuntimeCommand::StartCapture {
                source,
                endpoint_name,
                stream_id,
                response,
            } => {
                _capture = None;
                let result =
                    match AudioCaptureSession::start(source, endpoint_name.as_deref(), stream_id) {
                        Ok((session, rx)) => {
                            let started = StartedAudioCapture {
                                format: session.format.clone(),
                                rx,
                            };
                            _capture = Some(session);
                            Ok(started)
                        }
                        Err(error) => Err(error.to_string()),
                    };
                let _ = response.send(result);
            }
            AudioRuntimeCommand::StopCapture => {
                _capture = None;
            }
            AudioRuntimeCommand::StartRender {
                stream_id,
                format,
                response,
            } => {
                let result = render
                    .start(stream_id, format)
                    .map_err(|error| error.to_string());
                let _ = response.send(result);
            }
            AudioRuntimeCommand::PushFrame { frame, response } => {
                let result = render.push_frame(&frame).map_err(|error| error.to_string());
                let _ = response.send(result);
            }
            AudioRuntimeCommand::StopRender => render.stop(),
            AudioRuntimeCommand::Shutdown => break,
        }
    }
}

struct AudioCaptureSession {
    _stream: cpal::Stream,
    pub format: AudioFormat,
}

impl AudioCaptureSession {
    pub fn start(
        source: LocalAudioCaptureSource,
        endpoint_name: Option<&str>,
        stream_id: DeviceId,
    ) -> Result<(Self, mpsc::Receiver<CapturedAudioFrame>)> {
        if source == LocalAudioCaptureSource::Loopback && !cfg!(windows) {
            anyhow::bail!("System audio loopback capture is currently supported only on Windows");
        }

        let host = cpal::default_host();
        let device = select_capture_device(&host, source, endpoint_name)?;
        // CPAL 0.15 WASAPI enables AUDCLNT_STREAMFLAGS_LOOPBACK when an
        // output endpoint is used to build an input stream.
        let supported_config = if source == LocalAudioCaptureSource::Loopback {
            device.default_output_config()
        } else {
            device.default_input_config()
        }
        .context("Default audio capture config is unavailable")?;
        let format = AudioFormat {
            sample_rate: supported_config.sample_rate().0,
            channels: u8::try_from(supported_config.channels())
                .context("Too many capture channels")?,
            sample_format: AudioSampleFormat::PcmI16Le,
            frame_ms: DEFAULT_FRAME_MS,
        };
        validate_audio_format(&format)?;
        let config: cpal::StreamConfig = supported_config.clone().into();
        let (tx, rx) = mpsc::channel(CAPTURE_QUEUE_FRAMES);
        let pending_samples = Arc::new(Mutex::new(Vec::<i16>::new()));
        let sequence = Arc::new(AtomicU64::new(0));

        let stream = match supported_config.sample_format() {
            cpal::SampleFormat::I16 => build_input_stream::<i16>(
                &device,
                &config,
                stream_id,
                format.clone(),
                pending_samples,
                sequence,
                tx,
            )?,
            cpal::SampleFormat::U16 => build_input_stream::<u16>(
                &device,
                &config,
                stream_id,
                format.clone(),
                pending_samples,
                sequence,
                tx,
            )?,
            cpal::SampleFormat::F32 => build_input_stream::<f32>(
                &device,
                &config,
                stream_id,
                format.clone(),
                pending_samples,
                sequence,
                tx,
            )?,
            sample_format => anyhow::bail!("Unsupported input sample format: {sample_format:?}"),
        };
        stream
            .play()
            .context("Failed to start audio input stream")?;

        Ok((
            Self {
                _stream: stream,
                format,
            },
            rx,
        ))
    }
}

pub struct AudioRenderRuntime {
    active: Option<AudioRenderSession>,
}

impl AudioRenderRuntime {
    pub fn new() -> Self {
        Self { active: None }
    }

    pub fn start(&mut self, stream_id: DeviceId, format: AudioFormat) -> Result<AudioRenderStats> {
        self.stop();
        let session = AudioRenderSession::start(stream_id, format)?;
        let stats = session.stats();
        self.active = Some(session);
        Ok(stats)
    }

    pub fn push_frame(&mut self, frame: &AudioFramePayload) -> Result<AudioRenderStats> {
        validate_audio_frame(frame, &frame.format)?;
        if self
            .active
            .as_ref()
            .map(|session| session.stream_id != frame.stream_id)
            .unwrap_or(true)
        {
            self.start(frame.stream_id, frame.format.clone())?;
        }

        let session = self
            .active
            .as_mut()
            .ok_or_else(|| anyhow!("Audio render session is not active"))?;
        session.push_frame(frame)?;
        Ok(session.stats())
    }

    pub fn stop(&mut self) {
        self.active = None;
    }
}

struct AudioRenderSession {
    stream_id: DeviceId,
    format: AudioFormat,
    buffer: Arc<Mutex<VecDeque<i16>>>,
    underruns: Arc<AtomicU64>,
    overruns: Arc<AtomicU64>,
    frames_received: Arc<AtomicU64>,
    _stream: cpal::Stream,
}

impl AudioRenderSession {
    fn start(stream_id: DeviceId, format: AudioFormat) -> Result<Self> {
        validate_audio_format(&format)?;
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("No default audio output device is available"))?;
        let supported_config = output_config_for_format(&device, &format)?;
        let config: cpal::StreamConfig = supported_config.clone().into();
        let buffer = Arc::new(Mutex::new(VecDeque::<i16>::new()));
        let underruns = Arc::new(AtomicU64::new(0));
        let overruns = Arc::new(AtomicU64::new(0));
        let frames_received = Arc::new(AtomicU64::new(0));
        let render_buffer = buffer.clone();
        let render_underruns = underruns.clone();
        let render_channels = config.channels as usize;

        let err_fn = |err| tracing::warn!("Audio output stream error: {err}");
        let stream = match supported_config.sample_format() {
            cpal::SampleFormat::I16 => device.build_output_stream(
                &config,
                move |data: &mut [i16], _| {
                    fill_output_buffer(data, render_channels, &render_buffer, &render_underruns)
                },
                err_fn,
                None,
            )?,
            cpal::SampleFormat::U16 => device.build_output_stream(
                &config,
                move |data: &mut [u16], _| {
                    fill_output_buffer(data, render_channels, &render_buffer, &render_underruns)
                },
                err_fn,
                None,
            )?,
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    fill_output_buffer(data, render_channels, &render_buffer, &render_underruns)
                },
                err_fn,
                None,
            )?,
            sample_format => anyhow::bail!("Unsupported output sample format: {sample_format:?}"),
        };
        stream
            .play()
            .context("Failed to start audio output stream")?;

        Ok(Self {
            stream_id,
            format,
            buffer,
            underruns,
            overruns,
            frames_received,
            _stream: stream,
        })
    }

    fn push_frame(&mut self, frame: &AudioFramePayload) -> Result<()> {
        validate_audio_frame(frame, &self.format)?;

        let mut samples = Vec::with_capacity(frame.data.len() / 2);
        for chunk in frame.data.chunks_exact(2) {
            samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
        }

        let mut buffer = self
            .buffer
            .lock()
            .map_err(|_| anyhow!("Audio render buffer lock is poisoned"))?;
        let max_samples = samples_per_ms(&self.format) * MAX_RENDER_BUFFER_MS as usize;
        if buffer.len().saturating_add(samples.len()) > max_samples {
            let drop_count = buffer
                .len()
                .saturating_add(samples.len())
                .saturating_sub(max_samples);
            for _ in 0..drop_count {
                let _ = buffer.pop_front();
            }
            self.overruns.fetch_add(1, Ordering::Relaxed);
        }
        buffer.extend(samples);
        self.frames_received.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn stats(&self) -> AudioRenderStats {
        let buffer_samples = self.buffer.lock().map(|buffer| buffer.len()).unwrap_or(0);
        let samples_per_ms = samples_per_ms(&self.format).max(1);
        AudioRenderStats {
            frames_received: self.frames_received.load(Ordering::Relaxed),
            underruns: self.underruns.load(Ordering::Relaxed),
            overruns: self.overruns.load(Ordering::Relaxed),
            buffer_depth_ms: (buffer_samples / samples_per_ms) as u32,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AudioRenderStats {
    pub frames_received: u64,
    pub underruns: u64,
    pub overruns: u64,
    pub buffer_depth_ms: u32,
}

fn build_input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    stream_id: DeviceId,
    format: AudioFormat,
    pending_samples: Arc<Mutex<Vec<i16>>>,
    sequence: Arc<AtomicU64>,
    tx: mpsc::Sender<CapturedAudioFrame>,
) -> Result<cpal::Stream>
where
    T: cpal::Sample + cpal::SizedSample,
    i16: FromSampleValue<T>,
{
    let frame_samples = samples_per_frame(&format);
    let err_fn = |err| tracing::warn!("Audio input stream error: {err}");
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _| {
            let mut converted: Vec<i16> =
                data.iter().copied().map(i16::from_sample_value).collect();
            if converted.is_empty() {
                return;
            }

            let mut pending = match pending_samples.lock() {
                Ok(pending) => pending,
                Err(_) => return,
            };
            pending.append(&mut converted);

            while pending.len() >= frame_samples {
                let frame_samples_i16: Vec<i16> = pending.drain(..frame_samples).collect();
                let (level_peak, level_rms) = audio_levels(&frame_samples_i16);
                let mut data = Vec::with_capacity(frame_samples_i16.len() * 2);
                for sample in frame_samples_i16 {
                    data.extend_from_slice(&sample.to_le_bytes());
                }
                let frame = AudioFramePayload {
                    stream_id,
                    sequence: sequence.fetch_add(1, Ordering::Relaxed) + 1,
                    timestamp_ms: timestamp_ms_now(),
                    format: format.clone(),
                    data,
                };
                if !try_queue_capture(
                    &tx,
                    CapturedAudioFrame {
                        captured_at: std::time::Instant::now(),
                        frame,
                        level_peak,
                        level_rms,
                    },
                ) {
                    break;
                }
            }
        },
        err_fn,
        None,
    )?)
}

// Full queues drop the new frame; capture callbacks must never wait for the network.
fn try_queue_capture(tx: &mpsc::Sender<CapturedAudioFrame>, frame: CapturedAudioFrame) -> bool {
    !matches!(
        tx.try_send(frame),
        Err(mpsc::error::TrySendError::Closed(_))
    )
}

fn capture_endpoint_name(source: LocalAudioCaptureSource, name: &str) -> &str {
    let name = name.trim();
    if source == LocalAudioCaptureSource::Loopback {
        name.strip_prefix("系统声音 (")
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or(name)
    } else {
        name
    }
}

fn capture_device_index(names: &[&str], wanted: &str) -> Result<usize> {
    let wanted = wanted.trim().to_lowercase();
    let mut matches = names
        .iter()
        .enumerate()
        .filter(|(_, name)| name.trim().to_lowercase() == wanted);
    let (index, _) = matches
        .next()
        .context("Requested audio capture endpoint is unavailable")?;
    if matches.next().is_some() {
        anyhow::bail!("Requested audio capture endpoint name is ambiguous");
    }
    Ok(index)
}

fn select_capture_device(
    host: &cpal::Host,
    source: LocalAudioCaptureSource,
    endpoint_name: Option<&str>,
) -> Result<cpal::Device> {
    let loopback = source == LocalAudioCaptureSource::Loopback;
    if let Some(name) = endpoint_name {
        let devices: Vec<_> = if loopback {
            host.output_devices()?
        } else {
            host.input_devices()?
        }
        .collect();
        let names: Vec<_> = devices
            .iter()
            .map(|d| d.name().unwrap_or_default())
            .collect();
        let names: Vec<_> = names.iter().map(String::as_str).collect();
        let index = capture_device_index(&names, capture_endpoint_name(source, name))?;
        return devices
            .into_iter()
            .nth(index)
            .context("Audio capture endpoint disappeared");
    }
    if loopback {
        host.default_output_device()
    } else {
        host.default_input_device()
    }
    .context("No default audio capture endpoint is available")
}

fn validate_audio_format(format: &AudioFormat) -> Result<()> {
    if !(8000..=192000).contains(&format.sample_rate)
        || !(1..=8).contains(&format.channels)
        || !(1..=100).contains(&format.frame_ms)
        || (u64::from(format.sample_rate) * u64::from(format.frame_ms)) % 1000 != 0
    {
        anyhow::bail!("Unsupported audio format: {format:?}");
    }
    Ok(())
}

fn validate_audio_frame(frame: &AudioFramePayload, expected: &AudioFormat) -> Result<()> {
    validate_audio_format(&frame.format)?;
    if frame.format != *expected || frame.data.len() != samples_per_frame(expected) * 2 {
        anyhow::bail!("Audio frame format or payload length does not match the stream");
    }
    Ok(())
}
fn output_config_for_format(
    device: &cpal::Device,
    format: &AudioFormat,
) -> Result<cpal::SupportedStreamConfig> {
    let wanted_rate = cpal::SampleRate(format.sample_rate);
    let wanted_channels = format.channels as u16;
    for config in device
        .supported_output_configs()
        .context("Output device does not expose supported configs")?
    {
        if config.channels() == wanted_channels
            && config.min_sample_rate() <= wanted_rate
            && config.max_sample_rate() >= wanted_rate
            && matches!(
                config.sample_format(),
                cpal::SampleFormat::I16 | cpal::SampleFormat::U16 | cpal::SampleFormat::F32
            )
        {
            return Ok(config.with_sample_rate(wanted_rate));
        }
    }
    anyhow::bail!(
        "Output device does not support {} Hz / {} channels; audio resampling is required",
        format.sample_rate,
        format.channels
    )
}

fn fill_output_buffer<T>(
    data: &mut [T],
    _channels: usize,
    buffer: &Arc<Mutex<VecDeque<i16>>>,
    underruns: &Arc<AtomicU64>,
) where
    T: FromSampleValue<i16> + Copy,
{
    let mut did_underrun = false;
    if let Ok(mut queued) = buffer.lock() {
        for sample in data.iter_mut() {
            let value = queued.pop_front().unwrap_or_else(|| {
                did_underrun = true;
                0
            });
            *sample = T::from_sample_value(value);
        }
    } else {
        did_underrun = true;
        for sample in data.iter_mut() {
            *sample = T::from_sample_value(0);
        }
    }

    if did_underrun {
        underruns.fetch_add(1, Ordering::Relaxed);
    }
}

fn samples_per_frame(format: &AudioFormat) -> usize {
    ((format.sample_rate as usize * format.channels as usize * format.frame_ms as usize) / 1000)
        .max(format.channels as usize)
}

fn samples_per_ms(format: &AudioFormat) -> usize {
    ((format.sample_rate as usize * format.channels as usize) / 1000).max(1)
}

fn audio_levels(samples: &[i16]) -> (u8, u8) {
    if samples.is_empty() {
        return (0, 0);
    }

    let mut peak = 0i32;
    let mut sum_squares = 0f64;
    for sample in samples {
        let value = i32::from(*sample).abs();
        peak = peak.max(value);
        sum_squares += f64::from(*sample) * f64::from(*sample);
    }
    let rms = (sum_squares / samples.len() as f64).sqrt();
    (
        ((peak as f64 / i16::MAX as f64) * 100.0).min(100.0) as u8,
        ((rms / i16::MAX as f64) * 100.0).min(100.0) as u8,
    )
}

fn timestamp_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

trait FromSampleValue<T> {
    fn from_sample_value(value: T) -> Self;
}

impl FromSampleValue<i16> for i16 {
    fn from_sample_value(value: i16) -> Self {
        value
    }
}

impl FromSampleValue<u16> for i16 {
    fn from_sample_value(value: u16) -> Self {
        (i32::from(value) - 32768).clamp(i16::MIN as i32, i16::MAX as i32) as i16
    }
}

impl FromSampleValue<f32> for i16 {
    fn from_sample_value(value: f32) -> Self {
        (value.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
    }
}

impl FromSampleValue<i16> for u16 {
    fn from_sample_value(value: i16) -> Self {
        (i32::from(value) + 32768).clamp(0, u16::MAX as i32) as u16
    }
}

impl FromSampleValue<i16> for f32 {
    fn from_sample_value(value: i16) -> Self {
        value as f32 / i16::MAX as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_level_detects_peak_and_rms() {
        let (peak, rms) = audio_levels(&[0, i16::MAX, -i16::MAX, 0]);
        assert_eq!(peak, 100);
        assert!(rms > 60);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires an active Windows audio render endpoint"]
    fn windows_loopback_capture_opens_default_render_endpoint() {
        let (capture, _rx) =
            AudioCaptureSession::start(LocalAudioCaptureSource::Loopback, None, DeviceId::new_v4())
                .unwrap();
        validate_audio_format(&capture.format).unwrap();
    }

    #[test]
    fn frame_sample_count_uses_format_duration() {
        let format = AudioFormat::pcm_i16_48k_stereo_20ms();
        assert_eq!(samples_per_frame(&format), 1_920);
    }

    #[test]
    fn sample_conversion_centers_unsigned_audio() {
        assert_eq!(i16::from_sample_value(32768u16), 0);
        assert_eq!(u16::from_sample_value(0i16), 32768);
    }

    #[test]
    fn loopback_selects_render_endpoint_and_unwraps_display_label() {
        assert_eq!(
            capture_endpoint_name(
                LocalAudioCaptureSource::Loopback,
                "系统声音 (Speakers (USB Audio))"
            ),
            "Speakers (USB Audio)"
        );
        assert_eq!(
            capture_endpoint_name(LocalAudioCaptureSource::Microphone, "系统声音 (Mic)"),
            "系统声音 (Mic)"
        );
    }

    #[test]
    fn named_capture_requires_one_exact_device() {
        let names = ["USB Mic", "USB Mic Pro"];
        assert_eq!(capture_device_index(&names, " usb mic ").unwrap(), 0);
        assert!(capture_device_index(&names, "USB").is_err());
        assert!(capture_device_index(&["Mic", "Mic"], "Mic").is_err());
    }

    #[test]
    fn stalled_capture_expires_using_monotonic_time() {
        let mut captured = CapturedAudioFrame {
            captured_at: std::time::Instant::now() - std::time::Duration::from_secs(1),
            frame: AudioFramePayload {
                stream_id: DeviceId::nil(),
                sequence: 1,
                timestamp_ms: u64::MAX,
                format: AudioFormat::pcm_i16_48k_stereo_20ms(),
                data: vec![],
            },
            level_peak: 0,
            level_rms: 0,
        };
        assert!(captured.is_stale());
        captured.captured_at = std::time::Instant::now();
        assert!(!captured.is_stale());
    }

    #[test]
    fn malformed_audio_is_rejected_before_rendering() {
        let format = AudioFormat::pcm_i16_48k_stereo_20ms();
        let mut frame = AudioFramePayload {
            stream_id: DeviceId::new_v4(),
            sequence: 1,
            timestamp_ms: 0,
            format: format.clone(),
            data: vec![0; 3840],
        };
        assert!(validate_audio_frame(&frame, &format).is_ok());
        frame.data.pop();
        assert!(validate_audio_frame(&frame, &format).is_err());
        frame.data.push(0);
        frame.format.sample_rate = 44100;
        assert!(validate_audio_frame(&frame, &format).is_err());
        frame.format.channels = 0;
        assert!(validate_audio_format(&frame.format).is_err());
    }

    #[test]
    fn capture_queue_drops_without_blocking_when_consumer_stalls() {
        let (tx, mut rx) = mpsc::channel(2);
        for sequence in 1..=3 {
            assert_eq!(
                try_queue_capture(
                    &tx,
                    CapturedAudioFrame {
                        captured_at: std::time::Instant::now(),
                        frame: AudioFramePayload {
                            stream_id: DeviceId::nil(),
                            sequence,
                            timestamp_ms: 0,
                            format: AudioFormat::pcm_i16_48k_stereo_20ms(),
                            data: vec![]
                        },
                        level_peak: 0,
                        level_rms: 0,
                    }
                ),
                true
            );
        }
        assert_eq!(rx.try_recv().unwrap().frame.sequence, 1);
        assert_eq!(rx.try_recv().unwrap().frame.sequence, 2);
        assert!(rx.try_recv().is_err());
        drop(rx);
        assert!(!try_queue_capture(
            &tx,
            CapturedAudioFrame {
                captured_at: std::time::Instant::now(),
                frame: AudioFramePayload {
                    stream_id: DeviceId::nil(),
                    sequence: 4,
                    timestamp_ms: 0,
                    format: AudioFormat::pcm_i16_48k_stereo_20ms(),
                    data: vec![]
                },
                level_peak: 0,
                level_rms: 0,
            }
        ));
    }
}
