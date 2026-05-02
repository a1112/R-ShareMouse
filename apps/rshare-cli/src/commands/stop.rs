//! Stop command implementation.

use anyhow::Result;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use crate::output::{info, success, warning};

type BoxFutureResult<'a> = Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

/// Execute the stop command.
pub async fn execute(force: bool) -> Result<()> {
    let manager = Arc::new(rshare_core::service::ServiceManager::new()?);
    execute_with(
        force,
        {
            let manager = Arc::clone(&manager);
            move || manager.is_running()
        },
        {
            let manager = Arc::clone(&manager);
            move || manager.get_pid()
        },
        || Box::pin(async { rshare_core::daemon_client::request_shutdown().await }),
        {
            let manager = Arc::clone(&manager);
            move |pid| manager.is_pid_alive(pid)
        },
        {
            let manager = Arc::clone(&manager);
            move |pid| {
                let manager = Arc::clone(&manager);
                Box::pin(async move { manager.stop_pid(pid) })
            }
        },
        20,
        Duration::from_millis(200),
    )
    .await
}

async fn execute_with<IsRunning, GetPid, RequestShutdown, IsPidAlive, StopPid>(
    force: bool,
    mut is_running: IsRunning,
    mut get_pid: GetPid,
    mut request_shutdown: RequestShutdown,
    mut is_pid_alive: IsPidAlive,
    mut stop_pid: StopPid,
    wait_polls: usize,
    wait_interval: Duration,
) -> Result<()>
where
    IsRunning: FnMut() -> bool,
    GetPid: FnMut() -> Option<u32>,
    RequestShutdown: FnMut() -> BoxFutureResult<'static>,
    IsPidAlive: FnMut(u32) -> bool,
    StopPid: FnMut(u32) -> BoxFutureResult<'static>,
{
    if !is_running() {
        warning("R-ShareMouse service is not running");
        return Ok(());
    }

    let pid = get_pid().ok_or_else(|| anyhow::anyhow!("service reported running without a PID"))?;

    if force {
        info("Force stopping service...");
        stop_pid(pid).await?;
        success("Service stopped (force)");
        return Ok(());
    }

    info("Requesting graceful shutdown...");
    if let Err(err) = request_shutdown().await {
        warning(&format!(
            "Graceful shutdown request failed ({}), falling back to process stop",
            err
        ));
        stop_pid(pid).await?;
        success("Service stopped");
        return Ok(());
    }

    for _ in 0..wait_polls {
        tokio::time::sleep(wait_interval).await;
        if !is_pid_alive(pid) {
            success("Service stopped");
            return Ok(());
        }
    }

    warning("Service did not exit after graceful shutdown, forcing stop");
    stop_pid(pid).await?;
    success("Service stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn graceful_stop_keeps_tracking_original_pid_after_pid_file_disappears() {
        let process_alive = Arc::new(AtomicBool::new(true));
        let graceful_requests = Arc::new(AtomicUsize::new(0));
        let forced_stops = Arc::new(AtomicUsize::new(0));

        let result = execute_with(
            false,
            || true,
            || Some(4242),
            {
                let graceful_requests = Arc::clone(&graceful_requests);
                move || {
                    Box::pin({
                        let graceful_requests = Arc::clone(&graceful_requests);
                        async move {
                            graceful_requests.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        }
                    })
                }
            },
            {
                let process_alive = Arc::clone(&process_alive);
                move |pid| {
                    assert_eq!(pid, 4242);
                    process_alive.load(Ordering::SeqCst)
                }
            },
            {
                let forced_stops = Arc::clone(&forced_stops);
                let process_alive = Arc::clone(&process_alive);
                move |pid| {
                    Box::pin({
                        let forced_stops = Arc::clone(&forced_stops);
                        let process_alive = Arc::clone(&process_alive);
                        async move {
                            assert_eq!(pid, 4242);
                            forced_stops.fetch_add(1, Ordering::SeqCst);
                            process_alive.store(false, Ordering::SeqCst);
                            Ok(())
                        }
                    })
                }
            },
            1,
            Duration::from_millis(1),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(graceful_requests.load(Ordering::SeqCst), 1);
        assert_eq!(forced_stops.load(Ordering::SeqCst), 1);
    }
}
