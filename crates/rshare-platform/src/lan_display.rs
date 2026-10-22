//! Network.framework TLS-PSK stream; only explicit private LAN endpoints.
use std::{
    ffi::{c_char, c_void, CStr, CString},
    io::{self, Read, Write},
    sync::Arc,
};
unsafe extern "C" {
    fn rshare_lan_connect(
        host: *const c_char,
        key: *const u8,
        error: *mut c_char,
        size: usize,
    ) -> *mut c_void;
    fn rshare_lan_read(handle: *mut c_void, bytes: *mut u8, size: usize) -> isize;
    fn rshare_lan_write(handle: *mut c_void, bytes: *const u8, size: usize) -> bool;
    fn rshare_lan_cancel(handle: *mut c_void);
    fn rshare_lan_release(handle: *mut c_void);
}
struct Native(*mut c_void);
// Network.framework permits concurrent cancellation. Read/write are used only
// by the display worker; the second Arc is retained solely for stop().
unsafe impl Send for Native {}
unsafe impl Sync for Native {}
impl Drop for Native {
    fn drop(&mut self) {
        unsafe { rshare_lan_release(self.0) }
    }
}
#[derive(Clone)]
pub struct LanStream(Arc<Native>);
impl LanStream {
    pub fn connect(host: std::net::Ipv4Addr, key: &[u8; 32]) -> anyhow::Result<Self> {
        let host = CString::new(host.to_string())?;
        let mut error = [0 as c_char; 512];
        let handle = unsafe {
            rshare_lan_connect(host.as_ptr(), key.as_ptr(), error.as_mut_ptr(), error.len())
        };
        anyhow::ensure!(
            !handle.is_null(),
            "{}",
            unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy()
        );
        Ok(Self(Arc::new(Native(handle))))
    }
    pub fn cancel(&self) {
        unsafe { rshare_lan_cancel(self.0 .0) }
    }
}
impl Read for LanStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let count = unsafe { rshare_lan_read(self.0 .0, bytes.as_mut_ptr(), bytes.len()) };
        if count < 0 {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "LAN TLS receive failed or timed out",
            ));
        }
        Ok(count as usize)
    }
}
impl Write for LanStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if !unsafe { rshare_lan_write(self.0 .0, bytes.as_ptr(), bytes.len()) } {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "LAN TLS send failed or timed out",
            ));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
