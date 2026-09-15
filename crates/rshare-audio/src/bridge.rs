//! Process-shared SPSC ABI. Each mapping has one producer and one consumer;
//! native adapters must obey the ownership contract in rshare_audio_bridge.h.
use rshare_core::network_audio::{AudioError, Format};
use std::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
};
pub const CAPACITY: usize = 4096;
pub const MAGIC: u32 = 0x52534142;
pub const ABI: u32 = 1;
#[repr(C)]
pub struct Ring {
    magic: u32,
    version: u32,
    channels: u32,
    sample_rate: u32,
    write_frame: AtomicU64,
    read_frame: AtomicU64,
    generation: AtomicU64,
    clients: AtomicU32,
    online: AtomicU32,
    underruns: AtomicU64,
    overruns: AtomicU64,
    reserved: [u8; 64],
    samples: UnsafeCell<[f32; CAPACITY * 8]>,
}
impl Ring {
    pub fn new(format: Format) -> Result<Self, AudioError> {
        format.validate()?;
        Ok(Self {
            magic: MAGIC,
            version: ABI,
            channels: format.channels as u32,
            sample_rate: format.sample_rate,
            write_frame: AtomicU64::new(0),
            read_frame: AtomicU64::new(0),
            generation: AtomicU64::new(1),
            clients: AtomicU32::new(0),
            online: AtomicU32::new(0),
            underruns: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            reserved: [0; 64],
            samples: UnsafeCell::new([0.0; CAPACITY * 8]),
        })
    }
    pub fn clients(&self) -> u32 {
        self.clients.load(Ordering::Acquire)
    }
    pub fn online(&self, online: bool) {
        self.online.store(online as u32, Ordering::Release)
    }
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
    pub fn depth(&self) -> usize {
        self.write_frame
            .load(Ordering::Acquire)
            .wrapping_sub(self.read_frame.load(Ordering::Acquire))
            .min(CAPACITY as u64) as usize
    }
    /// # Safety
    /// Only the designated producer may call this, and the counterpart must
    /// not access samples without acquiring/publishing the ring cursors.
    pub unsafe fn write(&self, input: &[f32]) -> usize {
        let channels = self.channels as usize;
        if channels == 0
            || channels > 8
            || input.len() % channels != 0
            || self.online.load(Ordering::Acquire) == 0
        {
            return 0;
        }
        let frames = input.len() / channels;
        let w = self.write_frame.load(Ordering::Relaxed);
        let q = self.read_frame.load(Ordering::Acquire);
        let depth = w.wrapping_sub(q);
        if depth > CAPACITY as u64 || frames > CAPACITY - depth as usize {
            self.overruns.fetch_add(1, Ordering::Relaxed);
            return 0;
        }
        let ptr = self.samples.get().cast::<f32>();
        for frame in 0..frames {
            let offset = ((w.wrapping_add(frame as u64)) as usize % CAPACITY) * channels;
            std::ptr::copy_nonoverlapping(
                input.as_ptr().add(frame * channels),
                ptr.add(offset),
                channels,
            );
        }
        self.write_frame
            .store(w.wrapping_add(frames as u64), Ordering::Release);
        frames
    }
    /// # Safety
    /// Only the designated consumer may call this. See `write` for the SPSC
    /// ownership contract; the native peer must use the same versioned ABI.
    pub unsafe fn read(&self, output: &mut [f32]) -> usize {
        output.fill(0.0);
        let channels = self.channels as usize;
        if channels == 0 || channels > 8 || output.len() % channels != 0 {
            return 0;
        }
        let q = self.read_frame.load(Ordering::Relaxed);
        let w = self.write_frame.load(Ordering::Acquire);
        let depth = w.wrapping_sub(q);
        if depth > CAPACITY as u64 {
            return 0;
        }
        let frames = (output.len() / channels).min(depth as usize);
        if self.online.load(Ordering::Acquire) != 0 {
            let ptr = self.samples.get().cast::<f32>();
            for frame in 0..frames {
                let offset = (q.wrapping_add(frame as u64) as usize % CAPACITY) * channels;
                std::ptr::copy_nonoverlapping(
                    ptr.add(offset),
                    output.as_mut_ptr().add(frame * channels),
                    channels,
                );
            }
            if frames < output.len() / channels {
                self.underruns.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.read_frame
            .store(q.wrapping_add(frames as u64), Ordering::Release);
        frames
    }
}
const _: () = assert!(std::mem::size_of::<Ring>() == 131200);
#[cfg(unix)]
pub struct Mapping {
    ptr: std::ptr::NonNull<Ring>,
    _file: std::fs::File,
    path: std::path::PathBuf,
}
#[cfg(unix)]
impl Mapping {
    /// Create in a private daemon-owned directory. Never truncate existing
    /// mappings: the OS audio server may still hold a previous file open.
    pub fn create(path: &std::path::Path, format: Format) -> std::io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let ring = Ring::new(format)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let result = (|| {
            file.set_len(std::mem::size_of::<Ring>() as u64)?;
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    std::mem::size_of::<Ring>(),
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    std::os::fd::AsRawFd::as_raw_fd(&file),
                    0,
                )
            };
            if ptr == libc::MAP_FAILED {
                return Err(std::io::Error::last_os_error());
            }
            let ptr = std::ptr::NonNull::new(ptr.cast::<Ring>()).unwrap();
            unsafe { ptr.as_ptr().write(ring) };
            Ok(Self {
                ptr,
                _file: file,
                path: path.to_path_buf(),
            })
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        }
        result
    }
    pub fn ring(&self) -> &Ring {
        unsafe { self.ptr.as_ref() }
    }
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}
#[cfg(unix)]
impl Drop for Mapping {
    fn drop(&mut self) {
        self.ring().online(false);
        unsafe {
            libc::munmap(self.ptr.as_ptr().cast(), std::mem::size_of::<Ring>());
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use rshare_core::network_audio::Encoding;
    #[test]
    fn shared_abi_full_empty_and_offline_are_bounded() {
        let ring = Ring::new(Format {
            sample_rate: 48000,
            channels: 2,
            encoding: Encoding::Float32,
        })
        .unwrap();
        assert_eq!(std::mem::offset_of!(Ring, samples), 128);
        ring.online(true);
        let data = vec![0.25; CAPACITY * 2];
        unsafe {
            assert_eq!(ring.write(&data), CAPACITY);
            assert_eq!(ring.write(&[0.1; 2]), 0);
            let mut out = vec![0.0; CAPACITY * 2];
            assert_eq!(ring.read(&mut out), CAPACITY);
            assert_eq!(out, data);
            assert_eq!(ring.read(&mut out), 0);
            assert!(out.iter().all(|v| *v == 0.0));
            assert_eq!(ring.write(&data), CAPACITY);
            ring.online(false);
            ring.read(&mut out);
            assert!(out.iter().all(|v| *v == 0.0));
        }
    }
}

/// macOS uses a launchd/XPC memory broker, not paths inside user directories:
/// coreaudiod is sandboxed and runs under a different uid.
#[cfg(target_os = "macos")]
pub struct MacMapping {
    handle: std::ptr::NonNull<std::ffi::c_void>,
    ring: std::ptr::NonNull<Ring>,
    token: std::ffi::CString,
}
#[cfg(target_os = "macos")]
extern "C" {
    fn rsa_macos_bridge_create(
        token: *const std::ffi::c_char,
        rate: u32,
        channels: u32,
    ) -> *mut std::ffi::c_void;
    fn rsa_macos_bridge_ring(handle: *mut std::ffi::c_void) -> *mut Ring;
    fn rsa_macos_bridge_release(handle: *mut std::ffi::c_void, token: *const std::ffi::c_char);
}
#[cfg(target_os = "macos")]
impl MacMapping {
    pub fn create(format: Format) -> Result<Self, String> {
        format.validate().map_err(|e| e.to_string())?;
        let token = std::ffi::CString::new(uuid::Uuid::new_v4().to_string()).unwrap();
        let handle = std::ptr::NonNull::new(unsafe {
            rsa_macos_bridge_create(token.as_ptr(), format.sample_rate, format.channels as u32)
        })
        .ok_or("Audio memory broker is not installed or reachable")?;
        let ring = std::ptr::NonNull::new(unsafe { rsa_macos_bridge_ring(handle.as_ptr()) })
            .ok_or("Audio broker returned no mapping")?;
        Ok(Self {
            handle,
            ring,
            token,
        })
    }
    pub fn ring(&self) -> &Ring {
        unsafe { self.ring.as_ref() }
    }
    pub fn token(&self) -> &str {
        self.token.to_str().unwrap()
    }
}
#[cfg(target_os = "macos")]
impl Drop for MacMapping {
    fn drop(&mut self) {
        unsafe { rsa_macos_bridge_release(self.handle.as_ptr(), self.token.as_ptr()) }
    }
}
