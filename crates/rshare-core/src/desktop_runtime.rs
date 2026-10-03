//! The preview desktop owns only its bundled daemon and process tree.
use crate::{daemon_client, preview_profile, Config, ServiceStatusSnapshot};
use anyhow::{Context, Result};
use std::{
    fs,
    net::{TcpListener, UdpSocket},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::OnceLock,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::{JobObjects::*, Threading::GetCurrentProcess},
};

struct ProcessJob(HANDLE);
unsafe impl Send for ProcessJob {}
unsafe impl Sync for ProcessJob {}
impl ProcessJob {
    fn own_desktop() -> Result<Self> {
        unsafe {
            let job = Self(CreateJobObjectW(None, None)?);
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            )?;
            AssignProcessToJobObject(job.0, GetCurrentProcess())?;
            Ok(job)
        }
    }
}
impl Drop for ProcessJob {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
static JOB: OnceLock<ProcessJob> = OnceLock::new();
static PROFILE_LOCK: OnceLock<fs::File> = OnceLock::new();
static CHILD: tokio::sync::Mutex<Option<Child>> = tokio::sync::Mutex::const_new(None);

pub fn enabled() -> bool {
    std::env::var_os("RSHARE_OWNED_DESKTOP").is_some_and(|v| v == "1")
}

pub fn initialize() -> Result<()> {
    let requested = std::env::var_os("RSHARE_USER_ROOT").map(PathBuf::from);
    let native = dirs::config_dir();
    let root = preview_profile::profile_root(requested.as_deref(), native.as_deref())?;
    fs::create_dir_all(&root)?;
    std::env::set_var("RSHARE_USER_ROOT", fs::canonicalize(root)?);
    std::env::set_var("RSHARE_OWNED_DESKTOP", "1");
    std::env::remove_var("RSHARE_DAEMON_BIN");
    let listeners = (0..3)
        .map(|_| TcpListener::bind("127.0.0.1:0"))
        .collect::<std::io::Result<Vec<_>>>()?;
    for (name, listener) in ["RSHARE_IPC_PORT", "RSHARE_WS_PORT", "RSHARE_MOBILE_PORT"]
        .into_iter()
        .zip(&listeners)
    {
        std::env::set_var(name, listener.local_addr()?.port().to_string());
    }
    let config_path = crate::config::default_config_path()?;
    if !config_path.exists() {
        Config::default().save()?;
    }
    if JOB.get().is_none() {
        JOB.set(ProcessJob::own_desktop()?)
            .map_err(|_| anyhow::anyhow!("Desktop process ownership already initialized"))?;
    }
    Ok(())
}

pub fn write_runtime_report() -> Result<()> {
    let root = crate::config::default_config_path()?
        .parent()
        .context("Missing profile root")?
        .to_path_buf();
    use std::os::windows::fs::OpenOptionsExt;
    if PROFILE_LOCK.get().is_none() {
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(root.join("rbox-desktop.lock"))
            .context("Preview user profile is already owned by another desktop")?;
        PROFILE_LOCK
            .set(lock)
            .map_err(|_| anyhow::anyhow!("Profile ownership already initialized"))?;
    }
    let body = serde_json::json!({"desktopPid":std::process::id(),"ipc":crate::default_ipc_addr().to_string(),"ws":crate::default_local_controls_ws_addr().to_string(),"profile":root,"ownership":"bundled Windows job"});
    fs::write(
        root.join("rbox-runtime.json"),
        serde_json::to_vec_pretty(&body)?,
    )?;
    Ok(())
}

fn companion() -> Result<PathBuf> {
    let desktop = fs::canonicalize(std::env::current_exe()?)?;
    let root = desktop
        .parent()
        .context("Desktop has no package directory")?;
    let executable = fs::canonicalize(root.join("rshare-daemon.exe"))
        .context("Bundled rshare-daemon.exe is missing")?;
    if executable.parent() != Some(root) {
        anyhow::bail!("Bundled daemon escapes package directory");
    }
    Ok(executable)
}

pub async fn spawn_owned(port: Option<u16>, bind: Option<&str>) -> Result<ServiceStatusSnapshot> {
    anyhow::ensure!(
        PROFILE_LOCK.get().is_some(),
        "Preview profile is not owned by this desktop"
    );
    let mut owned = CHILD.lock().await;
    if let Some(child) = owned.as_mut() {
        if child.try_wait()?.is_some() {
            *owned = None;
        }
    }
    if owned.is_none() {
        // Never connect to an occupied port and treat a foreign process as ours.
        drop(
            TcpListener::bind(crate::default_ipc_addr())
                .context("Preview IPC port is already occupied")?,
        );
        let executable = companion()?;
        let root = crate::config::default_config_path()?
            .parent()
            .unwrap()
            .to_path_buf();
        let mut config = Config::load()?;
        if let Some(port) = port {
            config.network.port = port;
        }
        if let Some(bind) = bind {
            config.network.bind_address = bind.to_owned();
        }
        drop(UdpSocket::bind(config.bind_address()?).context("R-ShareMouse QUIC port is already occupied; close the other instance before starting this preview")?);
        // The profile is held exclusively and no child or IPC listener is alive.
        // A leftover PID is bookkeeping only; never terminate its current process.
        match fs::remove_file(root.join("rshare.pid")) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let error_log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("daemon-startup.stderr.log"))?;
        let mut command = Command::new(&executable);
        command
            .current_dir(executable.parent().unwrap())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(error_log));
        if let Some(port) = port {
            command.env("RSHARE_PORT", port.to_string());
        }
        if let Some(bind) = bind {
            command.env("RSHARE_BIND", bind);
        }
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
        *owned = Some(command.spawn()?);
    }
    let child = owned.as_mut().unwrap();
    let expected = child.id();
    let ready = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(status) = daemon_client::request_status_raw().await {
                preview_profile::require_owned_pid(expected, status.pid)?;
                break Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("Bundled daemon readiness timed out"))
    .and_then(|ready| ready)
    .and_then(|status| {
        preview_profile::require_owned_pid(expected, status.pid)?;
        Ok(status)
    });
    if ready.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        *owned = None;
    }
    ready
}

pub async fn shutdown_owned() -> Result<()> {
    let mut owned = CHILD.lock().await;
    if let Some(child) = owned.as_mut() {
        if child.try_wait()?.is_none() {
            if let Ok(Ok(status)) =
                tokio::time::timeout(Duration::from_secs(2), daemon_client::request_status_raw())
                    .await
            {
                if preview_profile::require_owned_pid(child.id(), status.pid).is_ok() {
                    let _ = tokio::time::timeout(
                        Duration::from_secs(2),
                        daemon_client::request_shutdown_raw(),
                    )
                    .await;
                }
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while child.try_wait()?.is_none() && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if child.try_wait()?.is_none() {
                child.kill()?;
            }
            child.wait()?;
        }
        *owned = None;
    }
    Ok(())
}

pub async fn check_owner() -> Result<()> {
    let mut owned = CHILD.lock().await;
    let child = owned.as_mut().context("Bundled daemon is not running")?;
    anyhow::ensure!(child.try_wait()?.is_none(), "Bundled daemon has exited");
    let status = tokio::time::timeout(Duration::from_secs(2), daemon_client::request_status_raw())
        .await
        .context("Bundled daemon identity check timed out")??;
    preview_profile::require_owned_pid(child.id(), status.pid)?;
    Ok(())
}
