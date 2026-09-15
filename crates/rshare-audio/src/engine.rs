//! Preallocated playout and clock recovery. Feed this on an audio worker through
//! an SPSC bridge; neither render nor insert allocates or locks.
use rshare_core::network_audio::*;
pub fn decode_samples(
    bytes: &[u8],
    encoding: Encoding,
    output: &mut [f32],
) -> Result<(), AudioError> {
    if bytes.len() != output.len() * encoding.bytes() {
        return Err(AudioError::InvalidPacket);
    }
    for (chunk, out) in bytes.chunks_exact(encoding.bytes()).zip(output) {
        *out = match encoding {
            Encoding::Pcm24 => {
                let n = i32::from_le_bytes([0, chunk[0], chunk[1], chunk[2]]) >> 8;
                n as f32 / 8388608.0
            }
            Encoding::Float32 => {
                let x = f32::from_le_bytes(chunk.try_into().unwrap());
                if x.is_finite() {
                    x
                } else {
                    0.0
                }
            }
        };
    }
    Ok(())
}
pub fn encode_samples(
    input: &[f32],
    encoding: Encoding,
    output: &mut [u8],
) -> Result<(), AudioError> {
    if output.len() != input.len() * encoding.bytes() {
        return Err(AudioError::InvalidPacket);
    }
    for (&sample, chunk) in input.iter().zip(output.chunks_exact_mut(encoding.bytes())) {
        let sample = if sample.is_finite() { sample } else { 0.0 };
        match encoding {
            Encoding::Float32 => chunk.copy_from_slice(&sample.to_le_bytes()),
            Encoding::Pcm24 => {
                let v = (sample as f64 * 8388608.0)
                    .round()
                    .clamp(-8388608.0, 8388607.0) as i32;
                chunk.copy_from_slice(&v.to_le_bytes()[..3]);
            }
        }
    }
    Ok(())
}
pub struct DriftController {
    target: f64,
    integral: f64,
    correction: f64,
}
impl DriftController {
    pub fn new(target_frames: f64) -> Self {
        Self {
            target: target_frames,
            integral: 0.0,
            correction: 0.0,
        }
    }
    /// Update once per millisecond, irrespective of native callback block size.
    pub fn update(&mut self, depth_frames: f64) -> f64 {
        let error = (depth_frames - self.target).clamp(-self.target, self.target);
        self.integral = (self.integral + error * 0.00000002).clamp(-0.001, 0.001);
        self.correction = (error * 0.00001 + self.integral).clamp(-0.001, 0.001);
        1.0 + self.correction
    }
    pub fn ppm(&self) -> f64 {
        self.correction * 1_000_000.0
    }
}
const TAPS: usize = 32;
const PHASES: usize = 256;
pub struct Playout {
    format: Format,
    target: usize,
    capacity: usize,
    samples: Vec<f32>,
    tags: Vec<u64>,
    filters: Vec<f32>,
    start: Option<u64>,
    end: u64,
    position: f64,
    running: bool,
    gain: f32,
    drift: DriftController,
    ratio: f64,
    until_update: usize,
    pub diagnostics: Diagnostics,
}
impl Playout {
    pub fn new(format: Format, buffer_ms: u16) -> Result<Self, AudioError> {
        format.validate()?;
        if !(2..=20).contains(&buffer_ms) {
            return Err(AudioError::InvalidConfiguration);
        }
        let target = format.sample_rate as usize * buffer_ms as usize / 1000;
        let capacity = format.sample_rate as usize * 40 / 1000;
        let mut filters = vec![0.0; TAPS * PHASES];
        for phase in 0..PHASES {
            let mut sum = 0.0;
            for tap in 0..TAPS {
                let x = tap as f64 - (TAPS / 2 - 1) as f64 - phase as f64 / PHASES as f64;
                let sinc = if x.abs() < 1e-12 {
                    0.94
                } else {
                    (std::f64::consts::PI * x * 0.94).sin() / (std::f64::consts::PI * x)
                };
                let window = 0.5 + 0.5 * (std::f64::consts::PI * x / (TAPS as f64 / 2.0)).cos();
                let value = (sinc * window) as f32;
                filters[phase * TAPS + tap] = value;
                sum += value;
            }
            for value in &mut filters[phase * TAPS..(phase + 1) * TAPS] {
                *value /= sum;
            }
        }
        Ok(Self {
            format,
            target,
            capacity,
            samples: vec![0.0; capacity * format.channels as usize],
            tags: vec![u64::MAX; capacity],
            filters,
            start: None,
            end: 0,
            position: 0.0,
            running: false,
            gain: 0.0,
            drift: DriftController::new(target as f64),
            ratio: 1.0,
            until_update: 0,
            diagnostics: Diagnostics::default(),
        })
    }
    pub fn insert(&mut self, position: u64, samples: &[f32]) -> bool {
        let channels = self.format.channels as usize;
        if samples.len() != self.format.frames_per_packet() * channels
            || position
                .checked_add(self.format.frames_per_packet() as u64)
                .is_none()
        {
            return false;
        }
        let end = position + self.format.frames_per_packet() as u64;
        if self.running && position < (self.position as u64) {
            self.diagnostics.late += 1;
            return false;
        }
        if self.running && end > self.position as u64 + self.capacity as u64 {
            self.diagnostics.overruns += 1;
            return false;
        }
        if !self.running
            && self
                .start
                .is_some_and(|start| end.saturating_sub(start.min(position)) > self.capacity as u64)
        {
            self.diagnostics.overruns += 1;
            return false;
        }
        if (position..end).any(|p| self.tags[p as usize % self.capacity] == p) {
            self.diagnostics.duplicates += 1;
            return false;
        }
        for (frame, input) in samples.chunks_exact(channels).enumerate() {
            let p = position + frame as u64;
            let slot = p as usize % self.capacity;
            self.tags[slot] = p;
            for (out, value) in self.samples[slot * channels..(slot + 1) * channels]
                .iter_mut()
                .zip(input)
            {
                *out = if value.is_finite() { *value } else { 0.0 };
            }
        }
        self.start = Some(self.start.map_or(position, |s| s.min(position)));
        self.end = self.end.max(end);
        self.diagnostics.received += 1;
        true
    }
    pub fn reset(&mut self) {
        self.tags.fill(u64::MAX);
        self.start = None;
        self.end = 0;
        self.position = 0.0;
        self.running = false;
        self.gain = 0.0;
        self.ratio = 1.0;
        self.until_update = 0;
        self.drift = DriftController::new(self.target as f64);
    }
    pub fn render(&mut self, output: &mut [f32]) {
        output.fill(0.0);
        let channels = self.format.channels as usize;
        if output.len() % channels != 0 {
            return;
        }
        if !self.running {
            let Some(start) = self.start else { return };
            if self.end.saturating_sub(start) < self.target as u64 {
                return;
            }
            self.position = start as f64;
            self.running = true;
        }
        let mut underrun = false;
        for frame in output.chunks_exact_mut(channels) {
            if self.until_update == 0 {
                self.ratio = self
                    .drift
                    .update((self.end as f64 - self.position).max(0.0));
                self.until_update = self.format.frames_per_packet();
            }
            self.until_update -= 1;
            let center = self.position as u64;
            let present = center < self.end && self.tags[center as usize % self.capacity] == center;
            if !present {
                underrun = true;
                self.gain = 0.0;
            } else {
                self.gain = (self.gain + 1.0 / (self.format.sample_rate as f32 * 0.001)).min(1.0);
                let phase = ((self.position.fract() * PHASES as f64) as usize).min(PHASES - 1);
                for (channel, out) in frame.iter_mut().enumerate() {
                    let mut sum = 0.0;
                    for tap in 0..TAPS {
                        let pos = center as i128 + tap as i128 - (TAPS / 2 - 1) as i128;
                        if pos < 0 || pos >= self.end as i128 {
                            continue;
                        }
                        let pos = pos as u64;
                        let slot = pos as usize % self.capacity;
                        if self.tags[slot] == pos {
                            sum += self.samples[slot * channels + channel]
                                * self.filters[phase * TAPS + tap];
                        }
                    }
                    *out = sum * self.gain;
                }
            }
            self.position += self.ratio;
        }
        if underrun {
            self.diagnostics.underruns += 1;
        }
        self.diagnostics.buffer_depth_ms =
            (self.end as f64 - self.position).max(0.0) * 1000.0 / self.format.sample_rate as f64;
        self.diagnostics.drift_ppm = self.drift.ppm();
    }
}
