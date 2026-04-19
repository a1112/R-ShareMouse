use rshare_core::{
    ipc::{
        default_ipc_addr, write_json_line, read_json_line, DaemonDeviceSnapshot,
        DaemonRequest, DaemonResponse, ServiceStatusSnapshot,
    },
    service::{pid_file_path, state_dir},
};
use tokio::io::duplex;
use uuid::Uuid;

#[tokio::test]
async fn daemon_requests_round_trip_over_json_lines() {
    let (mut writer, mut reader) = duplex(1024);
    let request = DaemonRequest::Status;

    write_json_line(&mut writer, &request).await.unwrap();
    let decoded: DaemonRequest = read_json_line(&mut reader).await.unwrap();

    assert_eq!(decoded, request);
}

#[tokio::test]
async fn daemon_connect_request_round_trips_target_device() {
    let (mut writer, mut reader) = duplex(1024);
    let request = DaemonRequest::Connect {
        device_id: Uuid::nil(),
    };

    write_json_line(&mut writer, &request).await.unwrap();
    let decoded: DaemonRequest = read_json_line(&mut reader).await.unwrap();

    assert_eq!(decoded, request);
}

#[tokio::test]
async fn daemon_responses_round_trip_device_payloads() {
    let (mut writer, mut reader) = duplex(4096);
    let response = DaemonResponse::Devices(vec![DaemonDeviceSnapshot {
        id: Uuid::nil(),
        name: "desktop".to_string(),
        hostname: "desktop-host".to_string(),
        addresses: vec!["192.168.1.10:27431".to_string()],
        connected: false,
        last_seen_secs: Some(4),
    }]);

    write_json_line(&mut writer, &response).await.unwrap();
    let decoded: DaemonResponse = read_json_line(&mut reader).await.unwrap();

    assert_eq!(decoded, response);
}

#[test]
fn default_status_snapshot_starts_empty_and_healthy() {
    let snapshot = ServiceStatusSnapshot::new(
        Uuid::nil(),
        "desktop".to_string(),
        "desktop-host".to_string(),
        "0.0.0.0:27431".to_string(),
        27432,
        42,
    );

    assert_eq!(snapshot.discovered_devices, 0);
    assert_eq!(snapshot.connected_devices, 0);
    assert!(snapshot.healthy);
    assert_eq!(snapshot.pid, 42);
}

#[test]
fn service_paths_live_under_rshare_state_dir() {
    let state_dir = state_dir().unwrap();
    let pid_file = pid_file_path().unwrap();

    assert!(state_dir.ends_with("rshare"));
    assert_eq!(pid_file.parent(), Some(state_dir.as_path()));
}

#[test]
fn default_ipc_addr_binds_to_loopback() {
    let addr = default_ipc_addr();

    assert!(addr.ip().is_loopback());
    assert_eq!(addr.port(), 27435);
}
