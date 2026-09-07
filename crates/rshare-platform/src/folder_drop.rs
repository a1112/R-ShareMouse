//! Native file-manager seams for simulated cross-screen copy gestures.
//! These functions run outside the capture/injection actors.
#[cfg(not(target_os = "macos"))]
use anyhow::bail;
use anyhow::Result;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
mod macos;

#[derive(Debug, Clone, Default)]
pub struct DragOrigin {
    #[cfg(target_os = "macos")]
    revision: i64,
    #[cfg(windows)]
    point: (i32, i32),
}

pub async fn begin_drag(x: i32, y: i32) -> Result<DragOrigin> {
    #[cfg(target_os = "macos")]
    {
        let _ = (x, y);
        tokio::task::spawn_blocking(|| {
            macos::drag_pasteboard(false).map(|(revision, _)| DragOrigin { revision })
        })
        .await?
    }
    #[cfg(windows)]
    {
        Ok(DragOrigin { point: (x, y) })
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (x, y);
        bail!("本平台尚不支持文件管理器跨屏拖拽")
    }
}

pub async fn dragged_files(origin: DragOrigin) -> Result<Vec<String>> {
    #[cfg(target_os = "macos")]
    {
        tokio::task::spawn_blocking(move || {
            let (revision, files) = macos::drag_pasteboard(true)?;
            Ok(if revision != origin.revision {
                files
            } else {
                vec![]
            })
        })
        .await?
    }
    #[cfg(windows)]
    {
        explorer_query("source", origin.point.0, origin.point.1).await
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = origin;
        Ok(vec![])
    }
}

pub async fn destination_folder(x: i32, y: i32) -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        use macos::Destination;
        match tokio::task::spawn_blocking(move || macos::destination_folder(x, y)).await?? {
            Destination::Path(path) => Ok(path),
            Destination::Window(bounds) => finder_window_folder(bounds).await,
        }
    }
    #[cfg(windows)]
    {
        let folders = explorer_query("target", x, y).await?;
        if folders.len() != 1 {
            bail!("请松手到资源管理器中的文件夹或文件列表空白处");
        }
        Ok(PathBuf::from(&folders[0]))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (x, y);
        bail!("本平台尚不支持文件管理器跨屏拖拽")
    }
}

#[cfg(target_os = "macos")]
async fn finder_window_folder(bounds: [i32; 4]) -> Result<PathBuf> {
    use anyhow::{bail, Context};
    use tokio::{io::AsyncReadExt, process::Command};
    // Finder does not expose AXDocument on all macOS versions. Read its
    // scriptable folder target only after AX has verified this exact window.
    let mut child = Command::new("/usr/bin/osascript")
        .args(["-e", include_str!("folder_drop/finder.applescript"), "--"])
        .args(bounds.map(|n| n.to_string()))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("无法读取 Finder 文件夹位置")?;
    let mut stdout = child
        .stdout
        .take()
        .context("Finder 输出不可用")?
        .take(64 * 1024 + 1);
    let mut stderr = child
        .stderr
        .take()
        .context("Finder 错误输出不可用")?
        .take(8192);
    let mut bytes = Vec::new();
    let mut errors = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        let (output, error_output) = tokio::join!(stdout.read_to_end(&mut bytes), stderr.read_to_end(&mut errors));
        output?; error_output?;
        if bytes.len() > 64 * 1024 { bail!("Finder 文件夹地址过长"); }
        if !child.wait().await?.success() {
            let error: String = String::from_utf8_lossy(&errors).chars().take(500).collect();
            bail!("无法读取 Finder 目标文件夹；请检查自动化中的 Finder 权限，并使用普通文件夹窗口：{}", error.trim());
        }
        let path = String::from_utf8(bytes).context("Finder 返回了无效地址")?;
        // osascript appends exactly one newline; preserve spaces in filenames.
        let path = path.strip_suffix('\n').unwrap_or(&path);
        if path.is_empty() { bail!("Finder 没有返回目标文件夹"); }
        Ok(PathBuf::from(path))
    }).await.context("Finder 文件夹识别超时，请检查自动化权限")?
}

/// End only the native drag that has already been captured for remote copy.
pub async fn cancel_native_drag() -> Result<()> {
    tokio::task::spawn_blocking(|| {
        #[cfg(target_os = "macos")]
        {
            let mut input = crate::macos::MacosInputEmulator::new();
            input.activate()?;
            input.send_hardware_key(53, true)?;
            input.send_hardware_key(53, false)?;
        }
        #[cfg(windows)]
        {
            let mut input = crate::windows::WindowsInputEmulator::new();
            input.activate()?;
            input.send_key(0x1B, true)?;
            input.send_key(0x1B, false)?;
        }
        Ok(())
    })
    .await?
}

#[cfg(windows)]
async fn explorer_query(mode: &str, x: i32, y: i32) -> Result<Vec<String>> {
    use anyhow::Context;
    use tokio::{io::AsyncReadExt, process::Command};
    // The script is fixed code. Coordinates and mode are separate data arguments.
    // Drain with a bound; never trust unbounded COM/PowerShell output.
    let mut child = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-Command",
        ])
        .arg(format!(
            "& {{\n{}\n}}",
            include_str!("folder_drop/explorer.ps1")
        ))
        .args([mode, &x.to_string(), &y.to_string()])
        .creation_flags(0x08000000)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("无法启动资源管理器文件夹识别")?;
    let mut stdout = child
        .stdout
        .take()
        .context("无法读取资源管理器识别结果")?
        .take(1024 * 1024 + 1);
    let mut bytes = Vec::new();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        stdout.read_to_end(&mut bytes).await?;
        if bytes.len() > 1024 * 1024 {
            bail!("资源管理器识别结果过大");
        }
        if !child.wait().await?.success() {
            bail!("无法识别此位置的文件夹；请使用普通资源管理器文件夹窗口");
        }
        let files: Vec<String> = serde_json::from_slice(&bytes)?;
        if files.len() > rshare_core::file_transfer::MAX_FILE_ENTRIES {
            bail!("拖拽项目过多");
        }
        Ok(files)
    })
    .await
    .context("资源管理器文件夹识别超时")?;
    result
}
