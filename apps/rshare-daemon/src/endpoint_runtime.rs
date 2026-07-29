use std::sync::Arc;

use rshare_core::{
    DeviceId, EndpointInjectError, EndpointInjectMode, EndpointInjectRequest, EndpointInjectResult,
    EndpointInjectTarget, LocalInputDiagnosticEvent, Message,
};
use rshare_input::InputInjectionHandle;
use rshare_net::NetworkManager;
use tokio::sync::{broadcast, oneshot, Mutex, RwLock};
use tokio::time::Duration;

use crate::{
    degraded_unavailable_health, endpoint_inject_error, endpoint_inject_failure_result,
    endpoint_payload_to_input_event, is_device_connected, record_endpoint_inject_event,
    timestamp_ms_now, DaemonState, PendingEndpointInject,
};

pub(crate) async fn inject_endpoint_event(
    network_manager: &Arc<Mutex<NetworkManager>>,
    injection: &InputInjectionHandle,
    state: &Arc<RwLock<DaemonState>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    target: EndpointInjectTarget,
    request: EndpointInjectRequest,
) -> EndpointInjectResult {
    let started_at = std::time::Instant::now();
    if let EndpointInjectTarget::Remote(device_id) = target {
        let connected = {
            let state = state.read().await;
            is_device_connected(&state, device_id)
        };
        if !connected {
            return endpoint_inject_failure_result(
                EndpointInjectTarget::Remote(device_id),
                &request,
                None,
                degraded_unavailable_health(),
                started_at.elapsed().as_millis() as u64,
                EndpointInjectError::TargetDisconnected,
            );
        }
        return inject_remote_endpoint_event(network_manager, state, device_id, request).await;
    }

    let input_event = match endpoint_payload_to_input_event(&request) {
        Ok(event) => event,
        Err(error) => {
            return endpoint_inject_failure_result(
                EndpointInjectTarget::Local,
                &request,
                None,
                degraded_unavailable_health(),
                started_at.elapsed().as_millis() as u64,
                endpoint_inject_error(&error),
            );
        }
    };

    let (backend_kind, health, result) = {
        let backend = injection.backend_snapshot();
        let backend_kind = Some(backend.kind);
        let health = backend.health;
        if matches!(
            request.mode,
            EndpointInjectMode::RequireHealthyBackend | EndpointInjectMode::TestLoopback
        ) && !backend.active
        {
            (
                backend_kind,
                health,
                Err(anyhow::anyhow!("Input injection backend is not active.")),
            )
        } else {
            (
                backend_kind,
                health,
                injection
                    .inject_trusted_local(input_event)
                    .await
                    .map_err(anyhow::Error::new),
            )
        }
    };

    match result {
        Ok(()) => {
            let (event, local_event) = record_endpoint_inject_event(state, &request).await;
            let _ = local_events_tx.send(local_event);
            EndpointInjectResult {
                correlation_id: request.correlation_id,
                target: EndpointInjectTarget::Local,
                accepted: true,
                backend_kind,
                health,
                elapsed_ms: started_at.elapsed().as_millis() as u64,
                loopback_event_id: Some(event.event_id),
                error: None,
            }
        }
        Err(error) => endpoint_inject_failure_result(
            EndpointInjectTarget::Local,
            &request,
            backend_kind,
            health,
            started_at.elapsed().as_millis() as u64,
            endpoint_inject_error(&error),
        ),
    }
}

async fn inject_remote_endpoint_event(
    network_manager: &Arc<Mutex<NetworkManager>>,
    state: &Arc<RwLock<DaemonState>>,
    device_id: DeviceId,
    request: EndpointInjectRequest,
) -> EndpointInjectResult {
    let started_at = std::time::Instant::now();
    let correlation_id = request.correlation_id.clone();
    let timeout_ms = if request.timeout_ms == 0 {
        1_000
    } else {
        request.timeout_ms
    };
    let (result_tx, result_rx) = oneshot::channel();
    {
        let mut state = state.write().await;
        if state.pending_endpoint_injects.contains_key(&correlation_id) {
            return endpoint_inject_failure_result(
                EndpointInjectTarget::Remote(device_id),
                &request,
                None,
                degraded_unavailable_health(),
                started_at.elapsed().as_millis() as u64,
                EndpointInjectError::RejectedByPolicy,
            );
        }
        state.pending_endpoint_injects.insert(
            correlation_id.clone(),
            PendingEndpointInject {
                target: device_id,
                started_at_ms: timestamp_ms_now(),
                result_tx,
            },
        );
    }

    let send_result = {
        let mut manager = network_manager.lock().await;
        manager
            .send_to(
                &device_id,
                Message::EndpointInjectRequest {
                    request: request.clone(),
                },
            )
            .await
    };
    if let Err(error) = send_result {
        let mut state = state.write().await;
        state.pending_endpoint_injects.remove(&correlation_id);
        tracing::debug!(
            "Failed to send endpoint inject request to {}: {}",
            device_id,
            error
        );
        return endpoint_inject_failure_result(
            EndpointInjectTarget::Remote(device_id),
            &request,
            None,
            degraded_unavailable_health(),
            started_at.elapsed().as_millis() as u64,
            EndpointInjectError::TransportFailed,
        );
    }

    match tokio::time::timeout(Duration::from_millis(timeout_ms), result_rx).await {
        Ok(Ok(mut result)) => {
            result.target = EndpointInjectTarget::Remote(device_id);
            result.elapsed_ms = started_at.elapsed().as_millis() as u64;
            result
        }
        Ok(Err(_)) => endpoint_inject_failure_result(
            EndpointInjectTarget::Remote(device_id),
            &request,
            None,
            degraded_unavailable_health(),
            started_at.elapsed().as_millis() as u64,
            EndpointInjectError::TransportFailed,
        ),
        Err(_) => {
            let mut state = state.write().await;
            state.pending_endpoint_injects.remove(&correlation_id);
            endpoint_inject_failure_result(
                EndpointInjectTarget::Remote(device_id),
                &request,
                None,
                degraded_unavailable_health(),
                started_at.elapsed().as_millis() as u64,
                EndpointInjectError::Timeout,
            )
        }
    }
}
