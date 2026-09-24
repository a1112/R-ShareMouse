//! Same-user local IPC. TCP loopback is not an authentication boundary.
use std::io;
use tokio::io::{AsyncRead, AsyncWrite};

pub trait LocalIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> LocalIo for T {}
pub type LocalStream = Box<dyn LocalIo>;
pub use platform::{connect, LocalListener};

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};

    fn socket_path() -> io::Result<PathBuf> {
        let uid = unsafe { libc::geteuid() };
        // Keep below sockaddr_un's limit, including macOS's long per-user TMPDIR.
        let directory = PathBuf::from("/tmp").join(format!("rshare-ipc-{uid}"));
        match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let metadata = std::fs::symlink_metadata(&directory)?;
        if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe local IPC directory",
            ));
        }
        Ok(directory.join(format!("daemon-{}.sock", crate::default_ipc_addr().port())))
    }

    pub async fn connect() -> io::Result<LocalStream> {
        let stream = UnixStream::connect(socket_path()?).await?;
        if stream.peer_cred()?.uid() != unsafe { libc::geteuid() } {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(Box::new(stream))
    }

    pub struct LocalListener {
        listener: UnixListener,
        path: PathBuf,
        // Keep the advisory lock until after Drop has removed our socket.
        _lock: std::fs::File,
    }
    impl LocalListener {
        pub fn bind() -> io::Result<Self> {
            Self::bind_path(socket_path()?)
        }
        fn bind_path(path: PathBuf) -> io::Result<Self> {
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path.with_extension("lock"))?;
            let metadata = lock.metadata()?;
            if !metadata.is_file()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o077 != 0
            {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "daemon IPC is already owned",
                ));
            }
            // The lock survives socket cleanup and is never unlinked, so two
            // daemons cannot acquire different lock inodes during a restart.
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    if !metadata.file_type().is_socket()
                        || metadata.uid() != unsafe { libc::geteuid() }
                    {
                        return Err(io::ErrorKind::PermissionDenied.into());
                    }
                    std::fs::remove_file(&path)?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            Ok(Self {
                listener: UnixListener::bind(&path)?,
                path,
                _lock: lock,
            })
        }
        pub async fn accept(&mut self) -> io::Result<LocalStream> {
            loop {
                let (stream, _) = self.listener.accept().await?;
                if stream.peer_cred()?.uid() == unsafe { libc::geteuid() } {
                    return Ok(Box::new(stream));
                }
            }
        }
    }
    impl Drop for LocalListener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[tokio::test]
        async fn stale_socket_recovery_preserves_live_listener_and_rejects_symlink() {
            let directory =
                PathBuf::from("/tmp").join(format!("rshare-ipc-test-{}", uuid::Uuid::new_v4()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .unwrap();
            let path = directory.join("daemon.sock");
            // A process crash closes the descriptor but leaves the socket name.
            drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
            let live = LocalListener::bind_path(path.clone()).unwrap();
            assert!(LocalListener::bind_path(path.clone()).is_err());
            assert!(path.exists());
            drop(live);
            assert!(!path.exists());
            let target = directory.join("unrelated");
            std::fs::write(&target, b"keep").unwrap();
            std::os::unix::fs::symlink(&target, &path).unwrap();
            assert!(LocalListener::bind_path(path.clone()).is_err());
            assert_eq!(std::fs::read(&target).unwrap(), b"keep");
            std::fs::remove_file(&path).unwrap();
            std::fs::remove_file(&target).unwrap();
            std::fs::remove_file(path.with_extension("lock")).unwrap();
            std::fs::remove_dir(&directory).unwrap();
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use std::{ffi::c_void, ptr};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};

    #[repr(C)]
    struct SecurityAttributes {
        length: u32,
        descriptor: *mut c_void,
        inherit: i32,
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(process: isize, access: u32, token: *mut isize) -> i32;
        fn GetTokenInformation(
            token: isize,
            class: u32,
            data: *mut c_void,
            size: u32,
            needed: *mut u32,
        ) -> i32;
        fn ConvertSidToStringSidW(sid: *const c_void, output: *mut *mut u16) -> i32;
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl: *const u16,
            version: u32,
            output: *mut *mut c_void,
            size: *mut u32,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetCurrentProcessId() -> u32;
        fn ProcessIdToSessionId(process: u32, session: *mut u32) -> i32;
        fn OpenProcess(access: u32, inherit: i32, process: u32) -> isize;
        fn GetNamedPipeServerProcessId(pipe: isize, process: *mut u32) -> i32;
        fn GetNamedPipeClientProcessId(pipe: isize, process: *mut u32) -> i32;
        fn CloseHandle(handle: isize) -> i32;
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    fn user_sid() -> io::Result<String> {
        process_sid(unsafe { GetCurrentProcess() })
    }
    fn process_sid(process: isize) -> io::Result<String> {
        unsafe {
            let mut token = 0;
            if OpenProcessToken(process, 8, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let result = (|| {
                let mut needed = 0;
                GetTokenInformation(token, 1, ptr::null_mut(), 0, &mut needed);
                if needed == 0 || needed > 65536 {
                    return Err(io::Error::last_os_error());
                }
                // TOKEN_USER starts with a pointer; use pointer-aligned backing storage.
                let mut storage =
                    vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
                if GetTokenInformation(token, 1, storage.as_mut_ptr().cast(), needed, &mut needed)
                    == 0
                {
                    return Err(io::Error::last_os_error());
                }
                let mut text = ptr::null_mut();
                if ConvertSidToStringSidW(storage[0] as *const c_void, &mut text) == 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut len = 0;
                while *text.add(len) != 0 {
                    len += 1;
                }
                let value = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
                LocalFree(text.cast());
                Ok(value)
            })();
            CloseHandle(token);
            result
        }
    }
    fn session_id(process: u32) -> io::Result<u32> {
        let mut session = 0;
        if unsafe { ProcessIdToSessionId(process, &mut session) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(session)
    }
    fn verify_peer(pipe: isize, server: bool) -> io::Result<()> {
        let mut pid = 0;
        let found = unsafe {
            if server {
                GetNamedPipeServerProcessId(pipe, &mut pid)
            } else {
                GetNamedPipeClientProcessId(pipe, &mut pid)
            }
        };
        if found == 0 {
            return Err(io::Error::last_os_error());
        }
        if session_id(pid)? != session_id(unsafe { GetCurrentProcessId() })? {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let process = unsafe { OpenProcess(0x1000, 0, pid) };
        if process == 0 {
            return Err(io::Error::last_os_error());
        }
        let sid = process_sid(process);
        unsafe {
            CloseHandle(process);
        }
        if sid? != user_sid()? {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(())
    }
    fn pipe_name() -> io::Result<String> {
        Ok(format!(
            r"\\.\pipe\RShareMouse-{}-{}-{}",
            user_sid()?,
            session_id(unsafe { GetCurrentProcessId() })?,
            crate::default_ipc_addr().port()
        ))
    }
    pub async fn connect() -> io::Result<LocalStream> {
        let name = pipe_name()?;
        for _ in 0..100 {
            match ClientOptions::new().open(&name) {
                Ok(stream) => {
                    verify_peer(stream.as_raw_handle() as isize, true)?;
                    return Ok(Box::new(stream));
                }
                Err(e) if e.raw_os_error() == Some(231) => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await
                }
                Err(e) => return Err(e),
            }
        }
        Err(io::ErrorKind::TimedOut.into())
    }
    fn create(name: &str, first: bool) -> io::Result<NamedPipeServer> {
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;SY)(A;;GA;;;{})", user_sid()?)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        unsafe {
            let mut descriptor = ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let mut attributes = SecurityAttributes {
                length: std::mem::size_of::<SecurityAttributes>() as u32,
                descriptor,
                inherit: 0,
            };
            let result = ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(
                    name,
                    (&mut attributes as *mut SecurityAttributes).cast(),
                );
            LocalFree(descriptor);
            result
        }
    }
    pub struct LocalListener {
        name: String,
        next: NamedPipeServer,
    }
    impl LocalListener {
        pub fn bind() -> io::Result<Self> {
            let name = pipe_name()?;
            Ok(Self {
                next: create(&name, true)?,
                name,
            })
        }
        pub async fn accept(&mut self) -> io::Result<LocalStream> {
            loop {
                self.next.connect().await?;
                let next = create(&self.name, false)?;
                let stream = std::mem::replace(&mut self.next, next);
                if verify_peer(stream.as_raw_handle() as isize, false).is_ok() {
                    return Ok(Box::new(stream));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn local_transport_round_trip_and_single_listener() {
        let mut listener = LocalListener::bind().unwrap();
        assert!(LocalListener::bind().is_err());
        let server = tokio::spawn(async move {
            let mut client = listener.accept().await.unwrap();
            let value = client.read_u32().await.unwrap();
            client.write_u32(value + 1).await.unwrap();
            // Keep the pipe alive until the client consumes the response.
            assert_eq!(client.read_u8().await.unwrap(), 1);
        });
        let mut client = connect().await.unwrap();
        client.write_u32(41).await.unwrap();
        assert_eq!(client.read_u32().await.unwrap(), 42);
        client.write_u8(1).await.unwrap();
        server.await.unwrap();
    }
}
