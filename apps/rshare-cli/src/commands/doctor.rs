//! Product readiness diagnostics for dual-machine validation.

use anyhow::Result;
use colored::Colorize;
use rshare_core::{
    daemon_client, BackendHealth, DaemonDeviceSnapshot, DeviceId, EndpointEvent,
    EndpointEventDirection, EndpointEventFilter, EndpointEventKind, EndpointEventPayload,
    EndpointInjectMode, EndpointInjectRequest, EndpointInjectResult, EndpointInjectTarget,
    LayoutGraph, LocalControlDeviceSnapshot, ServiceStatusSnapshot,
};

use crate::output::{header, kv};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckState {
    Pass,
    Warn,
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DoctorCheck {
    key: &'static str,
    label: &'static str,
    state: CheckState,
    detail: String,
}

/// Execute the doctor command.
pub async fn execute(
    connect: bool,
    inject: bool,
    endpoint_events: u16,
    strict: bool,
) -> Result<()> {
    header("R-ShareMouse Doctor");

    let status_result = daemon_client::request_status().await;
    let status_error = status_result.as_ref().err().map(|error| error.to_string());
    let status = status_result.ok();

    let mut devices = if status.is_some() {
        daemon_client::request_devices().await.unwrap_or_default()
    } else {
        Vec::new()
    };

    if connect {
        connect_discovered_devices(&devices).await;
        devices = daemon_client::request_devices().await.unwrap_or_default();
    }
    let layout = if status.is_some() {
        daemon_client::request_layout().await.ok()
    } else {
        None
    };
    let local_controls = if status.is_some() {
        daemon_client::request_local_controls().await.ok()
    } else {
        None
    };
    let inject_results = if inject {
        run_remote_inject_probe(&devices).await
    } else {
        Vec::new()
    };
    let endpoint_events = collect_endpoint_events(status.as_ref(), &devices, endpoint_events).await;

    let checks = build_doctor_checks(
        status.as_ref(),
        status_error.as_deref(),
        &devices,
        layout.as_ref(),
        local_controls.as_ref(),
        &endpoint_events,
        inject,
        &inject_results,
    );

    for check in &checks {
        print_check(check);
    }

    if let Some(status) = &status {
        println!();
        kv(
            "Device",
            &format!("{} ({})", status.device_name, status.hostname),
        );
        kv("Endpoint", &status.bind_address);
        kv("Remote Devices", &devices.len().to_string());
        kv("Endpoint Events", &endpoint_events.len().to_string());
    }

    if strict && checks.iter().any(|check| check.state == CheckState::Block) {
        anyhow::bail!("Doctor found blocking readiness checks.");
    }

    Ok(())
}

async fn connect_discovered_devices(devices: &[DaemonDeviceSnapshot]) {
    for device in devices.iter().filter(|device| !device.connected) {
        match daemon_client::request_connect(device.id).await {
            Ok(()) => println!("  {}", format!("Connected {}", device.name).green()),
            Err(error) => eprintln!(
                "{}",
                format!("  [WARN] Connect {} failed: {error}", device.name).yellow()
            ),
        }
    }
}

async fn collect_endpoint_events(
    status: Option<&ServiceStatusSnapshot>,
    devices: &[DaemonDeviceSnapshot],
    limit: u16,
) -> Vec<EndpointEvent> {
    let Some(status) = status else {
        return Vec::new();
    };

    let mut collected = Vec::new();
    let connected: Vec<DeviceId> = devices
        .iter()
        .filter(|device| device.connected)
        .map(|device| device.id)
        .collect();

    if connected.is_empty() {
        if let Ok(events) = daemon_client::request_endpoint_events(
            EndpointEventFilter {
                endpoint_id: None,
                device_id: None,
                kinds: Vec::new(),
                sources: Vec::new(),
                include_loopback: true,
            },
            None,
            Some(limit),
        )
        .await
        {
            collected.extend(events);
        }
        return collected;
    }

    for endpoint_id in connected {
        if endpoint_id == status.device_id {
            continue;
        }
        if let Ok(events) = daemon_client::request_endpoint_events(
            EndpointEventFilter {
                endpoint_id: Some(endpoint_id),
                device_id: None,
                kinds: Vec::new(),
                sources: Vec::new(),
                include_loopback: true,
            },
            None,
            Some(limit),
        )
        .await
        {
            collected.extend(events);
        }
    }

    collected
}

async fn run_remote_inject_probe(devices: &[DaemonDeviceSnapshot]) -> Vec<EndpointInjectResult> {
    let Some(target) = devices
        .iter()
        .find(|device| device.connected)
        .map(|device| device.id)
    else {
        return Vec::new();
    };

    let requests = [
        EndpointInjectRequest {
            correlation_id: format!("doctor-key-press-{}", rshare_core::timestamp_ms()),
            device_kind: EndpointEventKind::Keyboard,
            payload: EndpointEventPayload::Keyboard {
                key: "ShiftLeft".to_string(),
                state: "Pressed".to_string(),
            },
            mode: EndpointInjectMode::TestLoopback,
            timeout_ms: 1_000,
        },
        EndpointInjectRequest {
            correlation_id: format!("doctor-key-release-{}", rshare_core::timestamp_ms()),
            device_kind: EndpointEventKind::Keyboard,
            payload: EndpointEventPayload::Keyboard {
                key: "ShiftLeft".to_string(),
                state: "Released".to_string(),
            },
            mode: EndpointInjectMode::TestLoopback,
            timeout_ms: 1_000,
        },
    ];

    let mut results = Vec::new();
    for request in requests {
        match daemon_client::request_endpoint_inject(EndpointInjectTarget::Remote(target), request)
            .await
        {
            Ok(result) => results.push(result),
            Err(error) => {
                eprintln!(
                    "{}",
                    format!("  [WARN] Remote inject request failed: {error}").yellow()
                );
            }
        }
    }
    results
}

fn build_doctor_checks(
    status: Option<&ServiceStatusSnapshot>,
    status_error: Option<&str>,
    devices: &[DaemonDeviceSnapshot],
    layout: Option<&LayoutGraph>,
    local_controls: Option<&LocalControlDeviceSnapshot>,
    endpoint_events: &[EndpointEvent],
    inject_requested: bool,
    inject_results: &[EndpointInjectResult],
) -> Vec<DoctorCheck> {
    let daemon_online = status.is_some();
    let connected_count = devices.iter().filter(|device| device.connected).count();
    let remote_ids: Vec<DeviceId> = devices.iter().map(|device| device.id).collect();
    let remote_event_count = endpoint_events
        .iter()
        .filter(|event| remote_ids.contains(&event.endpoint_id))
        .count();
    let injected_remote_event_count = endpoint_events
        .iter()
        .filter(|event| {
            remote_ids.contains(&event.endpoint_id)
                && matches!(
                    event.direction,
                    EndpointEventDirection::Injected | EndpointEventDirection::InjectedLoopback
                )
        })
        .count();
    let inject_probe_passed = inject_requested
        && !inject_results.is_empty()
        && inject_results.iter().all(|result| result.accepted);
    let input_backend_ready = status
        .and_then(|snapshot| snapshot.backend_health.as_ref())
        .map_or(false, |health| matches!(health, BackendHealth::Healthy));
    let local_capture_ready = local_controls.map_or(false, |snapshot| {
        snapshot.keyboard.detected
            || snapshot.mouse.detected
            || snapshot.gamepads.iter().any(|gamepad| gamepad.connected)
            || !snapshot.recent_events.is_empty()
    });
    let layout_nodes = layout.map_or(0, |layout| layout.nodes.len());

    vec![
        DoctorCheck {
            key: "ipc",
            label: "IPC",
            state: if daemon_online {
                CheckState::Pass
            } else {
                CheckState::Block
            },
            detail: status
                .map(|snapshot| format!("daemon PID {}", snapshot.pid))
                .unwrap_or_else(|| status_error.unwrap_or("daemon unavailable").to_string()),
        },
        DoctorCheck {
            key: "discovery",
            label: "局域网发现",
            state: if devices.is_empty() {
                if daemon_online {
                    CheckState::Warn
                } else {
                    CheckState::Block
                }
            } else {
                CheckState::Pass
            },
            detail: format!("已发现 {} 台，已连接 {} 台", devices.len(), connected_count),
        },
        DoctorCheck {
            key: "layout",
            label: "布局图",
            state: if layout_nodes > 1 {
                CheckState::Pass
            } else if daemon_online {
                CheckState::Warn
            } else {
                CheckState::Block
            },
            detail: format!("LayoutGraph 节点 {} 个", layout_nodes),
        },
        DoctorCheck {
            key: "local-capture",
            label: "本机捕获",
            state: if local_capture_ready {
                CheckState::Pass
            } else if daemon_online {
                CheckState::Warn
            } else {
                CheckState::Block
            },
            detail: local_controls
                .map(|snapshot| {
                    format!(
                        "keyboard={} mouse={} recent_events={}",
                        snapshot.keyboard.detected,
                        snapshot.mouse.detected,
                        snapshot.recent_events.len()
                    )
                })
                .unwrap_or_else(|| "local controls unavailable".to_string()),
        },
        DoctorCheck {
            key: "input-backend",
            label: "输入后端",
            state: if input_backend_ready {
                CheckState::Pass
            } else if daemon_online {
                CheckState::Warn
            } else {
                CheckState::Block
            },
            detail: status
                .and_then(|snapshot| snapshot.backend_health.as_ref())
                .map(|health| format!("{health:?}"))
                .unwrap_or_else(|| "backend health unavailable".to_string()),
        },
        DoctorCheck {
            key: "remote-events",
            label: "远端事件镜像",
            state: if remote_event_count > 0 {
                CheckState::Pass
            } else if connected_count > 0 {
                CheckState::Warn
            } else {
                CheckState::Block
            },
            detail: format!("远端事件 {} 条", remote_event_count),
        },
        DoctorCheck {
            key: "remote-inject",
            label: "远端注入",
            state: if inject_probe_passed || injected_remote_event_count > 0 {
                CheckState::Pass
            } else if inject_requested {
                CheckState::Block
            } else if connected_count > 0 {
                CheckState::Warn
            } else {
                CheckState::Block
            },
            detail: if inject_requested {
                format!(
                    "注入结果 {}/{} 成功",
                    inject_results
                        .iter()
                        .filter(|result| result.accepted)
                        .count(),
                    inject_results.len()
                )
            } else {
                "未运行注入探测；使用 --inject 执行 Shift loopback".to_string()
            },
        },
    ]
}

fn print_check(check: &DoctorCheck) {
    let state = match check.state {
        CheckState::Pass => "OK".green(),
        CheckState::Warn => "WARN".yellow(),
        CheckState::Block => "BLOCK".red(),
    };
    println!(
        "  [{}] {:<12} {}",
        state,
        check.label,
        check.detail.replace('\n', " ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use rshare_core::{EndpointDeviceRef, EndpointEventSource};
    use uuid::Uuid;

    fn status(id: DeviceId) -> ServiceStatusSnapshot {
        let mut status = ServiceStatusSnapshot::new(
            id,
            "local-R-ShareMouse".to_string(),
            "local".to_string(),
            "0.0.0.0:27431".to_string(),
            27432,
            42,
        );
        status.backend_health = Some(BackendHealth::Healthy);
        status
    }

    fn device(id: DeviceId, connected: bool) -> DaemonDeviceSnapshot {
        DaemonDeviceSnapshot {
            id,
            name: "remote-R-ShareMouse".to_string(),
            hostname: "remote".to_string(),
            addresses: vec!["192.168.1.10".to_string()],
            connected,
            last_seen_secs: Some(1),
        }
    }

    fn endpoint_event(endpoint_id: DeviceId, direction: EndpointEventDirection) -> EndpointEvent {
        EndpointEvent {
            event_id: 1,
            sequence: 1,
            timestamp_ms: 1,
            endpoint_id,
            origin_endpoint_id: endpoint_id,
            device: EndpointDeviceRef {
                device_id: "keyboard".to_string(),
                instance_id: None,
                display_name: "Keyboard".to_string(),
                kind: EndpointEventKind::Keyboard,
                attribution: rshare_core::DeviceAttribution::Aggregate,
            },
            direction,
            source: EndpointEventSource::RemoteMirror,
            kind: EndpointEventKind::Keyboard,
            payload: EndpointEventPayload::Keyboard {
                key: "A".to_string(),
                state: "Pressed".to_string(),
            },
            correlation_id: None,
        }
    }

    #[test]
    fn doctor_checks_pass_when_remote_events_and_inject_loopback_exist() {
        let local = Uuid::new_v4();
        let remote = Uuid::new_v4();
        let mut layout = LayoutGraph::new(local);
        layout.add_node(rshare_core::LayoutNode::new(local, 0, 0, 1920, 1080));
        layout.add_node(rshare_core::LayoutNode::new(remote, 1920, 0, 1920, 1080));
        let mut controls = LocalControlDeviceSnapshot::default();
        controls.keyboard.detected = true;

        let checks = build_doctor_checks(
            Some(&status(local)),
            None,
            &[device(remote, true)],
            Some(&layout),
            Some(&controls),
            &[
                endpoint_event(remote, EndpointEventDirection::Observed),
                endpoint_event(remote, EndpointEventDirection::InjectedLoopback),
            ],
            false,
            &[],
        );

        assert_eq!(
            checks
                .iter()
                .map(|check| (check.key, check.state))
                .collect::<Vec<_>>(),
            vec![
                ("ipc", CheckState::Pass),
                ("discovery", CheckState::Pass),
                ("layout", CheckState::Pass),
                ("local-capture", CheckState::Pass),
                ("input-backend", CheckState::Pass),
                ("remote-events", CheckState::Pass),
                ("remote-inject", CheckState::Pass),
            ],
        );
    }

    #[test]
    fn doctor_checks_warn_for_connected_remote_without_events() {
        let local = Uuid::new_v4();
        let remote = Uuid::new_v4();
        let checks = build_doctor_checks(
            Some(&status(local)),
            None,
            &[device(remote, true)],
            None,
            Some(&LocalControlDeviceSnapshot::default()),
            &[],
            false,
            &[],
        );

        assert_eq!(
            checks
                .iter()
                .filter(|check| matches!(check.key, "remote-events" | "remote-inject"))
                .map(|check| check.state)
                .collect::<Vec<_>>(),
            vec![CheckState::Warn, CheckState::Warn],
        );
    }
}
