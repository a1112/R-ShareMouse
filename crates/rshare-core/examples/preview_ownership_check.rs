//! Native release validation: fail before touching a foreign listener or missing daemon.
#[cfg(windows)]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use rshare_core::desktop_runtime;
    let mode = std::env::args().nth(1).unwrap_or_default();
    anyhow::ensure!(
        std::env::var_os("RSHARE_USER_ROOT").is_some(),
        "Pass a fresh explicit test profile"
    );
    desktop_runtime::initialize()?;
    desktop_runtime::write_runtime_report()?;
    let listener = if mode == "occupied" {
        Some(std::net::TcpListener::bind(rshare_core::default_ipc_addr())?)
    } else {
        anyhow::ensure!(mode == "missing", "Use occupied or missing");
        let sibling = std::env::current_exe()?
            .parent()
            .unwrap()
            .join("rshare-daemon.exe");
        anyhow::ensure!(
            !sibling.exists(),
            "Missing-resource case requires an executable without a companion daemon"
        );
        None
    };
    let started = std::time::Instant::now();
    // Even with a live listener, client actions must fail before any connection
    // when this desktop has no bundled Child. In particular they cannot stop it.
    let refused = rshare_core::daemon_client::request_shutdown()
        .await
        .expect_err("Unowned request must fail");
    anyhow::ensure!(format!("{refused:#}").contains("Bundled daemon is not running"));
    let failure = desktop_runtime::spawn_owned(None, None)
        .await
        .expect_err("Must reject this startup");
    let message = format!("{failure:#}");
    anyhow::ensure!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "Preflight must fail promptly"
    );
    anyhow::ensure!(
        message.contains(if mode == "occupied" {
            "IPC port is already occupied"
        } else {
            "daemon.exe is missing"
        }),
        "Unexpected failure: {message}"
    );
    if let Some(listener) = listener {
        // Its socket is still ours after rejected startup; no shutdown request was sent.
        anyhow::ensure!(listener.local_addr()? == rshare_core::default_ipc_addr());
        listener.set_nonblocking(true)?;
        anyhow::ensure!(
            listener.accept().unwrap_err().kind() == std::io::ErrorKind::WouldBlock,
            "Must not connect to an unowned listener"
        );
        println!("Foreign test listener retained");
    }
    desktop_runtime::shutdown_owned().await?;
    println!("Passed {mode}: {message}");
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows native ownership validation only");
    std::process::exit(1);
}
