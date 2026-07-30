use anyhow::{Context, Result};
use rshare_core::{DisplayCaptureRequest, DisplayCaptureResult};
use tokio::sync::Semaphore;

static CAPTURE_LIMIT: Semaphore = Semaphore::const_new(2);

pub async fn capture_display(request: DisplayCaptureRequest) -> Result<DisplayCaptureResult> {
    run_capture_job(move || rshare_platform::display::capture_display(&request)).await
}

async fn run_capture_job<F, T>(job: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    let _permit = CAPTURE_LIMIT
        .acquire()
        .await
        .context("display capture semaphore closed")?;
    tokio::task::spawn_blocking(job)
        .await
        .context("display capture worker panicked")?
}

#[cfg(test)]
mod tests {
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::Duration;

    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn capture_workers_are_bounded_to_two() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let mut tasks = Vec::new();
        for index in 0..3 {
            let started_tx = started_tx.clone();
            let release_rx = Arc::clone(&release_rx);
            tasks.push(tokio::spawn(run_capture_job(move || {
                started_tx.send(index).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                Ok(index)
            })));
        }

        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(started_rx.recv_timeout(Duration::from_millis(50)).is_err());
        release_tx.send(()).unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        release_tx.send(()).unwrap();
        release_tx.send(()).unwrap();
        for task in tasks {
            task.await.unwrap().unwrap();
        }
    }
}
