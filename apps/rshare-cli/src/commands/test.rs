//! Automatic dual-machine input verification and bounded stress testing.

use anyhow::{anyhow, Result};
use clap::{Args, Subcommand};
use rshare_core::{
    daemon_client, CrossDeviceTestKind, CrossDeviceTestReport, CrossDeviceTestRequest,
    CrossDeviceTestStatusSnapshot, DeviceId,
};
use std::time::Duration;

#[derive(Debug, Subcommand)]
pub enum TestCommands {
    /// Send a finite sequence of events to the automatically selected peer.
    Cross(CrossArgs),
    /// Continuously send events for a bounded duration and collect metrics.
    Stress(StressArgs),
    /// Request the current test to stop and wait briefly for cleanup.
    Stop,
    /// Show the latest test report.
    Status,
}

#[derive(Debug, Args)]
pub struct CrossArgs {
    /// Optional peer UUID. Omit to use the first connected peer.
    #[arg(long)]
    pub device_id: Option<String>,
    /// Number of events to send.
    #[arg(long, default_value = "20")]
    pub samples: u32,
    /// Delay between events in milliseconds.
    #[arg(long, default_value = "10")]
    pub interval_ms: u64,
    /// Per-event remote response timeout in milliseconds.
    #[arg(long, default_value = "1000")]
    pub timeout_ms: u64,
    /// Test Shift press/release events instead of mouse positions.
    #[arg(long)]
    pub keyboard: bool,
    #[arg(long, default_value = "0")]
    pub start_x: i32,
    #[arg(long, default_value = "0")]
    pub start_y: i32,
    #[arg(long, default_value = "1")]
    pub step_x: i32,
    #[arg(long, default_value = "1")]
    pub step_y: i32,
    /// Emit the complete JSON report.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct StressArgs {
    /// Optional peer UUID. Omit to use the first connected peer.
    #[arg(long)]
    pub device_id: Option<String>,
    /// Stress duration in seconds.
    #[arg(long, default_value = "10")]
    pub duration_seconds: u64,
    /// Delay between events in milliseconds.
    #[arg(long, default_value = "10")]
    pub interval_ms: u64,
    /// Per-event remote response timeout in milliseconds.
    #[arg(long, default_value = "1000")]
    pub timeout_ms: u64,
    /// Test Shift press/release events instead of mouse positions.
    #[arg(long)]
    pub keyboard: bool,
    #[arg(long, default_value = "0")]
    pub start_x: i32,
    #[arg(long, default_value = "0")]
    pub start_y: i32,
    #[arg(long, default_value = "1")]
    pub step_x: i32,
    #[arg(long, default_value = "1")]
    pub step_y: i32,
    /// Emit the complete JSON report.
    #[arg(long)]
    pub json: bool,
}

pub async fn execute(command: TestCommands) -> Result<()> {
    match command {
        TestCommands::Cross(args) => {
            let report =
                daemon_client::request_cross_device_test(request_from_cross(&args)?).await?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_report(&report);
            }
        }
        TestCommands::Stress(args) => {
            let status =
                daemon_client::start_cross_device_stress(request_from_stress(&args)?).await?;
            let report = wait_for_stress(status).await?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_report(&report);
            }
        }
        TestCommands::Stop => {
            let status = daemon_client::stop_cross_device_stress().await?;
            print_status(&status);
        }
        TestCommands::Status => {
            print_status(&daemon_client::request_cross_device_test_status().await?)
        }
    }
    Ok(())
}

fn request_from_cross(args: &CrossArgs) -> Result<CrossDeviceTestRequest> {
    Ok(CrossDeviceTestRequest {
        device_id: parse_device_id(args.device_id.as_deref())?,
        kind: if args.keyboard {
            CrossDeviceTestKind::KeyboardShift
        } else {
            CrossDeviceTestKind::MouseMove
        },
        sample_count: args.samples,
        duration_ms: 0,
        interval_ms: args.interval_ms,
        timeout_ms: args.timeout_ms,
        start_x: args.start_x,
        start_y: args.start_y,
        step_x: args.step_x,
        step_y: args.step_y,
    })
}

fn request_from_stress(args: &StressArgs) -> Result<CrossDeviceTestRequest> {
    let duration_ms = args
        .duration_seconds
        .checked_mul(1_000)
        .ok_or_else(|| anyhow!("duration_seconds is too large"))?;
    Ok(CrossDeviceTestRequest {
        device_id: parse_device_id(args.device_id.as_deref())?,
        kind: if args.keyboard {
            CrossDeviceTestKind::KeyboardShift
        } else {
            CrossDeviceTestKind::MouseMove
        },
        sample_count: 0,
        duration_ms,
        interval_ms: args.interval_ms,
        timeout_ms: args.timeout_ms,
        start_x: args.start_x,
        start_y: args.start_y,
        step_x: args.step_x,
        step_y: args.step_y,
    })
}

fn parse_device_id(value: Option<&str>) -> Result<Option<DeviceId>> {
    value
        .map(|value| {
            value
                .parse()
                .map_err(|error| anyhow!("invalid device UUID: {error}"))
        })
        .transpose()
}

async fn wait_for_stress(
    mut status: CrossDeviceTestStatusSnapshot,
) -> Result<CrossDeviceTestReport> {
    loop {
        if !status.active {
            return status
                .report
                .ok_or_else(|| anyhow!("stress test finished without a report"));
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                status = daemon_client::stop_cross_device_stress().await?;
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                status = daemon_client::request_cross_device_test_status().await?;
            }
        }
    }
}

fn print_status(status: &CrossDeviceTestStatusSnapshot) {
    println!("active: {}", status.active);
    if let Some(report) = &status.report {
        print_report(report);
    } else {
        println!("no cross-device test report");
    }
}

fn print_report(report: &CrossDeviceTestReport) {
    let metrics = &report.metrics;
    println!(
        "test={} state={:?} target={} kind={:?} duration={}ms",
        report.test_id,
        report.state,
        report
            .target_device_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "auto".to_string()),
        report.kind,
        report.duration_ms
    );
    println!(
        "requested={} sent={} accepted={} observed={} failed={} timeout={} cancelled={} throughput={:.3}/s",
        metrics.requested,
        metrics.sent,
        metrics.accepted,
        metrics.observed,
        metrics.failed,
        metrics.timed_out,
        metrics.cancelled,
        metrics.throughput_per_second_milli.unwrap_or_default() as f64 / 1000.0,
    );
    println!(
        "latency_us p50={} p95={} p99={} max={}",
        format_optional(metrics.p50_us),
        format_optional(metrics.p95_us),
        format_optional(metrics.p99_us),
        format_optional(metrics.max_us),
    );
    if let Some(error) = &report.error {
        println!("error: {error}");
    }
    for sample in report.samples.iter().rev().take(3).rev() {
        println!(
            "sample[{}] event={:?} pos=({}, {}) sent_ms={} remote_ms={} elapsed_us={} accepted={} observed={} remote_event_id={}",
            sample.index,
            sample.event_kind,
            sample.position_x.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string()),
            sample.position_y.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string()),
            sample.sent_timestamp_ms,
            sample
                .remote_event_timestamp_ms
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string()),
            sample
                .elapsed_us
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string()),
            sample.accepted,
            sample.observed,
            sample
                .remote_event_id
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string()),
        );
    }
}

fn format_optional(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}
