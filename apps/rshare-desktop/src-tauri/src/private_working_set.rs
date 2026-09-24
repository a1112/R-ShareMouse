//! Private resident memory, in bytes. Never substitute RSS or private commit.
#[cfg(windows)]
pub fn bytes(pid: u32) -> Option<u64> {
    use std::{ffi::c_void, mem::size_of};
    // PROCESS_MEMORY_COUNTERS_EX2. Local layout also builds with older SDKs.
    #[repr(C)]
    struct Counters {
        cb: u32,
        faults: u32,
        peak_ws: usize,
        ws: usize,
        peak_paged: usize,
        paged: usize,
        peak_nonpaged: usize,
        nonpaged: usize,
        commit: usize,
        peak_commit: usize,
        private_commit: usize,
        private_ws: usize,
        shared_commit: u64,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn K32GetProcessMemoryInfo(handle: *mut c_void, info: *mut Counters, size: u32) -> i32;
    }
    // Query-limited access is sufficient on supported Windows versions.
    let handle = unsafe { OpenProcess(0x1000, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut info: Counters = unsafe { std::mem::zeroed() };
    info.cb = size_of::<Counters>() as u32;
    // Older Windows may accept the buffer while only filling an older prefix.
    info.private_ws = usize::MAX;
    let ok = unsafe { K32GetProcessMemoryInfo(handle, &mut info, size_of::<Counters>() as u32) };
    unsafe {
        CloseHandle(handle);
    }
    (ok != 0 && info.private_ws != usize::MAX).then_some(info.private_ws as u64)
}

#[cfg(target_os = "linux")]
pub fn bytes(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()?;
    let value = |key: &str| -> Option<u64> {
        let line = text.lines().find(|line| line.starts_with(key))?;
        line.split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()?
            .checked_mul(1024)
    };
    value("Private_Clean:")?.checked_add(value("Private_Dirty:")?)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn bytes(_pid: u32) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn invalid_pid_is_unavailable() {
        assert_eq!(super::bytes(u32::MAX), None);
    }
    #[test]
    #[cfg(any(windows, target_os = "linux"))]
    fn private_resident_memory_tracks_touched_allocation() {
        let before = super::bytes(std::process::id()).expect("supported test host");
        let mut allocation = vec![0u8; 16 * 1024 * 1024];
        for byte in allocation.iter_mut().step_by(4096) {
            *byte = 42;
        }
        std::hint::black_box(&allocation);
        let after = super::bytes(std::process::id()).unwrap();
        assert!(after >= before + 8 * 1024 * 1024, "{before} -> {after}");
    }
}
