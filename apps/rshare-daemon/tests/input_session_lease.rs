use std::sync::{Arc, Mutex};
use std::time::Duration;

use rshare_core::{
    BackendHealth, BackendKind, ControlSessionState, DeviceId, Direction, InputRouter, LayoutGraph,
    LayoutLink, LayoutNode, PixelRect, RealtimeInputFrame, ReliableInputFrame, RouterCommand,
    SessionEpoch, VirtualDesktopGeometry,
};
use rshare_daemon::input_runtime::{InputRuntime, InputTransport};
use rshare_daemon::input_state::{input_state_channel, ControlMetrics};
use rshare_input::{
    CaptureOrigin, CaptureSource, CapturedInputPayload, ContinuousInput, InjectBackend,
    InjectionActorConfig, InputEvent, InputInjectionHandle, PointerSample, SemanticInputIngress,
};
use rshare_net::qos::TransportSendError;
use tokio::sync::mpsc;

#[derive(Debug)]
struct NoopInjectBackend;

impl InjectBackend for NoopInjectBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Portable
    }
    fn health(&self) -> BackendHealth {
        BackendHealth::Healthy
    }
    fn inject(&mut self, _event: InputEvent) -> anyhow::Result<()> {
        Ok(())
    }
    fn inject_relative_pointer(&mut self, _dx: i32, _dy: i32) -> anyhow::Result<()> {
        Ok(())
    }
    fn is_active(&self) -> bool {
        true
    }
}

#[derive(Default)]
struct LeaseTransport {
    lease_attempts: Mutex<Vec<(DeviceId, SessionEpoch)>>,
    input_count: Mutex<usize>,
    next_lease_error: Mutex<Option<TransportSendError>>,
}

impl InputTransport for LeaseTransport {
    type Binding = DeviceId;
    fn bind(&self, target: DeviceId) -> Option<DeviceId> {
        Some(target)
    }
    fn try_send_realtime(&self, _binding: &DeviceId, _frame: RealtimeInputFrame) {
        *self.input_count.lock().unwrap() += 1;
    }
    fn try_send_reliable(&self, _binding: &DeviceId, _frame: ReliableInputFrame) -> bool {
        *self.input_count.lock().unwrap() += 1;
        true
    }
    fn try_send_session_lease(
        &self,
        binding: &DeviceId,
        epoch: SessionEpoch,
    ) -> Result<(), TransportSendError> {
        self.lease_attempts.lock().unwrap().push((*binding, epoch));
        match self.next_lease_error.lock().unwrap().take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

async fn verify_idle_lease_stops_on_return(with_safety: bool) {
    let local = DeviceId::new_v4();
    let remote = DeviceId::new_v4();
    let mut layout = LayoutGraph::new(local);
    layout.add_node(LayoutNode::new(local, 0, 0, 100, 100));
    layout.add_node(LayoutNode::new(remote, 100, 0, 100, 100));
    layout.add_link(LayoutLink::new(
        local,
        Direction::Right,
        remote,
        Direction::Left,
    ));
    let router = InputRouter::new(
        local,
        layout,
        VirtualDesktopGeometry::new(PixelRect::new(0, 0, 100, 100)),
        [remote],
    );
    let (producer, consumer) = SemanticInputIngress::new(8);
    let transport = Arc::new(LeaseTransport::default());
    let (state, feeds) = input_state_channel(8);
    let injection =
        InputInjectionHandle::spawn(Box::new(NoopInjectBackend), InjectionActorConfig::default())
            .unwrap();
    let mut runtime = InputRuntime::new(
        consumer,
        router,
        transport.clone(),
        state,
        Arc::new(ControlMetrics::default()),
        injection,
    );
    producer.try_push(producer.capture(
        CaptureOrigin {
            source: CaptureSource::PortableHook,
            device_token: 7,
            instance_token: 9,
        },
        CapturedInputPayload::Continuous(ContinuousInput::Pointer(PointerSample::Absolute {
            x: 99,
            y: 50,
        })),
    ));
    assert!(runtime.process_next().await);
    let epoch = runtime.session_epoch();
    assert!(matches!(feeds.authoritative_rx.borrow().session,
        ControlSessionState::RemoteActive { target, .. } if target == remote));
    let input_count = *transport.input_count.lock().unwrap();
    *transport.next_lease_error.lock().unwrap() = Some(TransportSendError::ControlLaneFull);
    let (commands_tx, commands_rx) = mpsc::channel(8);
    let (_safety_tx, safety_rx) = mpsc::unbounded_channel();
    let task = if with_safety {
        tokio::spawn(runtime.run_with_safety(commands_rx, safety_rx))
    } else {
        tokio::spawn(runtime.run(commands_rx))
    };
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(499)).await;
    tokio::task::yield_now().await;
    assert!(transport.lease_attempts.lock().unwrap().is_empty());
    tokio::time::advance(Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(*transport.lease_attempts.lock().unwrap(), [(remote, epoch)]);
    assert!(
        matches!(
            feeds.authoritative_rx.borrow().session,
            ControlSessionState::RemoteActive { .. }
        ),
        "one full control queue must not suspend healthy input"
    );
    for _ in 0..8 {
        tokio::time::advance(Duration::from_millis(500)).await;
        tokio::task::yield_now().await;
    }
    assert_eq!(transport.lease_attempts.lock().unwrap().len(), 9);
    assert!(transport
        .lease_attempts
        .lock()
        .unwrap()
        .iter()
        .all(|lease| *lease == (remote, epoch)));
    assert_eq!(
        *transport.input_count.lock().unwrap(),
        input_count,
        "idle session renewal must never synthesize mouse or keyboard input"
    );

    commands_tx.send(RouterCommand::QuickReturn).await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(
        feeds.authoritative_rx.borrow().session,
        ControlSessionState::LocalReady
    );
    let attempts = transport.lease_attempts.lock().unwrap().len();
    tokio::time::advance(Duration::from_secs(3)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        transport.lease_attempts.lock().unwrap().len(),
        attempts,
        "returning to local ownership must stop the remote lease heartbeat"
    );
    drop(commands_tx);
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn idle_session_lease_renews_without_input_and_stops_on_return() {
    verify_idle_lease_stops_on_return(false).await;
}

#[tokio::test(start_paused = true)]
async fn safety_actor_idle_lease_renews_without_input_and_stops_on_return() {
    verify_idle_lease_stops_on_return(true).await;
}
