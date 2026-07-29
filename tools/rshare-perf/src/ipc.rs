use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};

use anyhow::{bail, Context, Result};
use rshare_core::{
    read_json_frame, write_json_frame, DaemonRequest, DaemonResponse, ServiceStatusSnapshot,
    IPC_FRAME_HEADER_LEN,
};
use rshare_daemon::ipc_server::handle_persistent_json_connection;
use serde::Serialize;
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinSet,
};

#[derive(Debug, Serialize)]
pub struct IpcPerfReport {
    pub scenario: &'static str,
    pub ephemeral_port: bool,
    pub implementation_contract: IpcImplementationContract,
    pub requests_per_connection: usize,
    pub runs: Vec<IpcPerfRun>,
}

#[derive(Debug, Serialize)]
pub struct IpcImplementationContract {
    pub handler: &'static str,
    pub header_bytes: usize,
    pub read_strategy: &'static str,
    pub connection_strategy: &'static str,
}

#[derive(Debug, Serialize)]
pub struct IpcPerfRun {
    pub concurrency: usize,
    pub connections: usize,
    pub completed_requests: usize,
    pub handler_dispatches: u64,
    pub elapsed_ms: f64,
    pub requests_per_second: f64,
    pub median_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
}

pub async fn run_matrix(requests: usize, concurrency: &[usize]) -> Result<IpcPerfReport> {
    if requests == 0 {
        bail!("IPC request count must be greater than zero");
    }
    if concurrency.is_empty() || concurrency.contains(&0) {
        bail!("IPC concurrency values must be greater than zero");
    }

    let mut runs = Vec::with_capacity(concurrency.len());
    let mut all_ephemeral = true;
    for &client_count in concurrency {
        let (run, ephemeral) = run_scenario(requests, client_count).await?;
        all_ephemeral &= ephemeral;
        runs.push(run);
    }
    Ok(IpcPerfReport {
        scenario: "daemon-framed-ipc",
        ephemeral_port: all_ephemeral,
        implementation_contract: IpcImplementationContract {
            handler: "rshare_daemon::ipc_server::handle_persistent_json_connection",
            header_bytes: IPC_FRAME_HEADER_LEN,
            read_strategy: "read-exact-header-bounded-payload",
            connection_strategy: "persistent-framed-requests",
        },
        requests_per_connection: requests,
        runs,
    })
}

async fn run_scenario(requests: usize, concurrency: usize) -> Result<(IpcPerfRun, bool)> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind ephemeral daemon IPC listener")?;
    let address = listener.local_addr()?;
    let dispatches = Arc::new(AtomicU64::new(0));
    let server_dispatches = Arc::clone(&dispatches);
    let server = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        for _ in 0..concurrency {
            let (stream, _) = listener.accept().await?;
            let dispatches = Arc::clone(&server_dispatches);
            connections.spawn(async move {
                handle_persistent_json_connection(stream, move |request| {
                    let dispatches = Arc::clone(&dispatches);
                    async move {
                        dispatches.fetch_add(1, Ordering::Relaxed);
                        Ok(dispatch_benchmark_request(request))
                    }
                })
                .await
            });
        }
        while let Some(result) = connections.join_next().await {
            result.context("daemon IPC connection task failed")??;
        }
        Ok::<_, anyhow::Error>(())
    });

    let started = Instant::now();
    let mut clients = JoinSet::new();
    for _ in 0..concurrency {
        clients.spawn(run_client(address, requests));
    }
    let mut latencies_us = Vec::with_capacity(requests * concurrency);
    while let Some(result) = clients.join_next().await {
        latencies_us.extend(result.context("IPC load client task failed")??);
    }
    let elapsed = started.elapsed();
    server.await.context("IPC load server task failed")??;

    latencies_us.sort_unstable();
    let completed_requests = latencies_us.len();
    let expected_requests = requests * concurrency;
    if completed_requests != expected_requests {
        bail!("IPC load completed {completed_requests} requests, expected {expected_requests}");
    }
    let handler_dispatches = dispatches.load(Ordering::Relaxed);
    if handler_dispatches != expected_requests as u64 {
        bail!(
            "daemon IPC handler dispatched {handler_dispatches} requests, expected {expected_requests}"
        );
    }

    Ok((
        IpcPerfRun {
            concurrency,
            connections: concurrency,
            completed_requests,
            handler_dispatches,
            elapsed_ms: elapsed.as_secs_f64() * 1_000.0,
            requests_per_second: completed_requests as f64 / elapsed.as_secs_f64(),
            median_us: percentile(&latencies_us, 50),
            p95_us: percentile(&latencies_us, 95),
            p99_us: percentile(&latencies_us, 99),
            max_us: *latencies_us.last().unwrap_or(&0),
        },
        address.port() != rshare_core::ipc::DEFAULT_IPC_PORT,
    ))
}

async fn run_client(address: std::net::SocketAddr, requests: usize) -> Result<Vec<u64>> {
    let mut stream = TcpStream::connect(address)
        .await
        .context("failed to connect to ephemeral daemon IPC handler")?;
    let mut latencies = Vec::with_capacity(requests);
    for _ in 0..requests {
        let started = Instant::now();
        write_json_frame(&mut stream, &DaemonRequest::Status).await?;
        let response: DaemonResponse = read_json_frame(&mut stream).await?;
        if !matches!(response, DaemonResponse::Status(_)) {
            bail!("daemon IPC load request returned a non-status response");
        }
        latencies.push(started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64);
    }
    Ok(latencies)
}

fn dispatch_benchmark_request(request: DaemonRequest) -> DaemonResponse {
    match request {
        DaemonRequest::Status => DaemonResponse::Status(ServiceStatusSnapshot::new(
            uuid::Uuid::nil(),
            "ipc-perf-daemon".to_string(),
            "localhost".to_string(),
            "127.0.0.1:0".to_string(),
            0,
            std::process::id(),
        )),
        _ => DaemonResponse::Error("IPC performance handler accepts Status only".to_string()),
    }
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() - 1) * percentile + 99) / 100;
    sorted[index.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::run_matrix;

    #[tokio::test]
    async fn real_daemon_handler_runs_sequential_and_concurrent_persistent_requests() {
        let report = run_matrix(3, &[1, 2]).await.unwrap();

        assert!(report.ephemeral_port);
        assert_eq!(report.implementation_contract.header_bytes, 5);
        assert_eq!(
            report.implementation_contract.read_strategy,
            "read-exact-header-bounded-payload"
        );
        assert_eq!(
            report.implementation_contract.connection_strategy,
            "persistent-framed-requests"
        );
        assert_eq!(report.runs.len(), 2);
        assert_eq!(report.runs[0].concurrency, 1);
        assert_eq!(report.runs[0].completed_requests, 3);
        assert_eq!(report.runs[0].connections, 1);
        assert_eq!(report.runs[1].concurrency, 2);
        assert_eq!(report.runs[1].completed_requests, 6);
        assert_eq!(report.runs[1].connections, 2);
    }
}
