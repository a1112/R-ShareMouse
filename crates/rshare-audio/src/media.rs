//! Bounded binary QUIC DATAGRAM codec. All allocation occurs off realtime threads.
use rshare_core::{network_audio::*, DeviceId};
use std::collections::{BTreeMap, BTreeSet};
pub const HEADER_BYTES: usize = 72;
const MAX_PACKET_BYTES: usize = 9216;
const REASSEMBLY_MS: u64 = 20;
const MAX_FRAGMENTS: usize = 32;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub sequence: u64,
    pub sample_position: u64,
    pub data: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Header {
    stream: DeviceId,
    generation: u64,
    sequence: u64,
    position: u64,
    format: Format,
    index: usize,
    count: usize,
    offset: usize,
    total: usize,
}
pub fn encode_frame(
    stream: DeviceId,
    generation: u64,
    sequence: u64,
    position: u64,
    format: Format,
    data: &[u8],
    mtu: usize,
) -> Result<Vec<Vec<u8>>, AudioError> {
    format.validate()?;
    if generation == 0 || data.len() != format.packet_bytes() || mtu <= HEADER_BYTES || mtu > 65535
    {
        return Err(AudioError::InvalidPacket);
    }
    let chunk_bytes = mtu - HEADER_BYTES;
    let count = data.len().div_ceil(chunk_bytes);
    if count > MAX_FRAGMENTS {
        return Err(AudioError::InvalidPacket);
    }
    let mut result = Vec::with_capacity(count);
    for (index, chunk) in data.chunks(chunk_bytes).enumerate() {
        let mut p = Vec::with_capacity(HEADER_BYTES + chunk.len());
        p.extend_from_slice(b"RSAU");
        p.extend_from_slice(&MEDIA_VERSION.to_be_bytes());
        p.extend_from_slice(&(HEADER_BYTES as u16).to_be_bytes());
        p.extend_from_slice(stream.as_bytes());
        for value in [generation, sequence, position] {
            p.extend_from_slice(&value.to_be_bytes());
        }
        p.extend_from_slice(&format.sample_rate.to_be_bytes());
        p.push(format.channels);
        p.push(match format.encoding {
            Encoding::Pcm24 => 1,
            Encoding::Float32 => 2,
        });
        p.extend_from_slice(&(format.frames_per_packet() as u16).to_be_bytes());
        p.extend_from_slice(&(index as u16).to_be_bytes());
        p.extend_from_slice(&(count as u16).to_be_bytes());
        p.extend_from_slice(&((index * chunk_bytes) as u32).to_be_bytes());
        p.extend_from_slice(&(data.len() as u32).to_be_bytes());
        p.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
        p.extend_from_slice(&0u16.to_be_bytes());
        p.extend_from_slice(chunk);
        result.push(p);
    }
    Ok(result)
}
fn u16_at(p: &[u8], i: usize) -> u16 {
    u16::from_be_bytes(p[i..i + 2].try_into().unwrap())
}
fn u32_at(p: &[u8], i: usize) -> u32 {
    u32::from_be_bytes(p[i..i + 4].try_into().unwrap())
}
fn u64_at(p: &[u8], i: usize) -> u64 {
    u64::from_be_bytes(p[i..i + 8].try_into().unwrap())
}
fn decode(p: &[u8]) -> Result<Header, AudioError> {
    if p.len() < HEADER_BYTES
        || p.len() > MAX_PACKET_BYTES
        || &p[..4] != b"RSAU"
        || u16_at(p, 4) != MEDIA_VERSION
        || u16_at(p, 6) as usize != HEADER_BYTES
        || u16_at(p, 70) != 0
    {
        return Err(AudioError::InvalidPacket);
    }
    let encoding = match p[53] {
        1 => Encoding::Pcm24,
        2 => Encoding::Float32,
        _ => return Err(AudioError::InvalidPacket),
    };
    let format = Format {
        sample_rate: u32_at(p, 48),
        channels: p[52],
        encoding,
    };
    format.validate()?;
    let h = Header {
        stream: DeviceId::from_slice(&p[8..24]).map_err(|_| AudioError::InvalidPacket)?,
        generation: u64_at(p, 24),
        sequence: u64_at(p, 32),
        position: u64_at(p, 40),
        format,
        index: u16_at(p, 56) as usize,
        count: u16_at(p, 58) as usize,
        offset: u32_at(p, 60) as usize,
        total: u32_at(p, 64) as usize,
    };
    let length = p.len() - HEADER_BYTES;
    if length == 0
        || length != u16_at(p, 68) as usize
        || h.total != format.packet_bytes()
        || h.count == 0
        || h.count > MAX_FRAGMENTS
        || h.index >= h.count
        || h.offset.checked_add(length).is_none_or(|n| n > h.total)
        || u16_at(p, 54) as usize != format.frames_per_packet()
        || h.position
            .checked_add(format.frames_per_packet() as u64)
            .is_none()
    {
        return Err(AudioError::InvalidPacket);
    }
    Ok(h)
}
struct Partial {
    header: Header,
    since: u64,
    data: Vec<u8>,
    covered: Vec<bool>,
    fragments: BTreeSet<usize>,
}
pub struct Reassembler {
    stream: DeviceId,
    generation: u64,
    format: Format,
    partials: BTreeMap<u64, Partial>,
    completed: BTreeSet<u64>,
    newest: Option<u64>,
    pub rejected: u64,
    pub duplicates: u64,
    pub expired: u64,
}
impl Reassembler {
    pub fn new(stream: DeviceId, generation: u64, format: Format) -> Result<Self, AudioError> {
        format.validate()?;
        if generation == 0 {
            return Err(AudioError::StaleGeneration);
        }
        Ok(Self {
            stream,
            generation,
            format,
            partials: BTreeMap::new(),
            completed: BTreeSet::new(),
            newest: None,
            rejected: 0,
            duplicates: 0,
            expired: 0,
        })
    }
    pub fn pending(&self) -> usize {
        self.partials.len()
    }
    pub fn expire(&mut self, now_ms: u64) {
        let before = self.partials.len();
        self.partials
            .retain(|_, p| now_ms.saturating_sub(p.since) < REASSEMBLY_MS);
        self.expired += (before - self.partials.len()) as u64;
    }
    pub fn push(&mut self, p: &[u8], now_ms: u64) -> Result<Option<Frame>, AudioError> {
        self.expire(now_ms);
        let h = decode(p)?;
        if h.stream != self.stream || h.generation != self.generation {
            self.rejected += 1;
            return Err(AudioError::StaleGeneration);
        }
        if h.format != self.format {
            return Err(AudioError::UnsupportedFormat);
        }
        if self.completed.contains(&h.sequence) {
            self.duplicates += 1;
            return Ok(None);
        }
        if self
            .newest
            .is_some_and(|n| h.sequence < n.saturating_sub(128))
        {
            self.rejected += 1;
            return Ok(None);
        }
        if !self.partials.contains_key(&h.sequence) && self.partials.len() >= 20 {
            self.partials.pop_first();
            self.expired += 1;
        }
        let part = self.partials.entry(h.sequence).or_insert_with(|| Partial {
            header: h.clone(),
            since: now_ms,
            data: vec![0; h.total],
            covered: vec![false; h.total],
            fragments: BTreeSet::new(),
        });
        if part.header.position != h.position || part.header.count != h.count {
            return Err(AudioError::InvalidPacket);
        }
        if part.fragments.contains(&h.index) {
            self.duplicates += 1;
            return Ok(None);
        }
        let end = h.offset + p.len() - HEADER_BYTES;
        if part.covered[h.offset..end].iter().any(|v| *v) {
            return Err(AudioError::InvalidPacket);
        }
        part.data[h.offset..end].copy_from_slice(&p[HEADER_BYTES..]);
        part.covered[h.offset..end].fill(true);
        part.fragments.insert(h.index);
        if part.fragments.len() == h.count {
            let part = self.partials.remove(&h.sequence).unwrap();
            if part.covered.iter().any(|v| !*v) {
                return Err(AudioError::InvalidPacket);
            }
            self.completed.insert(h.sequence);
            self.newest = Some(self.newest.unwrap_or(0).max(h.sequence));
            while self.completed.len() > 128 {
                self.completed.pop_first();
            }
            return Ok(Some(Frame {
                sequence: h.sequence,
                sample_position: h.position,
                data: part.data,
            }));
        }
        Ok(None)
    }
}
