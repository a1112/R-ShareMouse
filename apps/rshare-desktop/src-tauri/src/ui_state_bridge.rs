use std::{
    future::Future,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rshare_core::{daemon_client, UiCursor, UiEnvelope};
use tauri::{AppHandle, Emitter, Manager};
use tokio::task::{AbortHandle, JoinHandle};

pub const UI_STATE_EVENT: &str = "rshare://ui-state";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

pub struct UiStateStreamState {
    lifecycle: tokio::sync::Mutex<UiStateStreamLifecycle>,
}

struct UiStateStreamLifecycle {
    next_id: Option<u64>,
    latest_reservation: Option<UiStateStreamReservation>,
    active: Option<ActiveUiStateStream>,
}

struct UiStateStreamReservation {
    id: u64,
    consumed: bool,
}

struct ActiveUiStateStream {
    id: u64,
    task: JoinHandle<()>,
}

impl Default for UiStateStreamState {
    fn default() -> Self {
        Self {
            lifecycle: tokio::sync::Mutex::new(UiStateStreamLifecycle {
                next_id: Some(1),
                latest_reservation: None,
                active: None,
            }),
        }
    }
}

impl UiStateStreamState {
    async fn reserve(&self) -> Result<u64, String> {
        let mut lifecycle = self.lifecycle.lock().await;
        let id = lifecycle
            .next_id
            .ok_or_else(|| "UI state stream id space exhausted".to_string())?;
        lifecycle.next_id = id.checked_add(1);
        lifecycle.latest_reservation = Some(UiStateStreamReservation {
            id,
            consumed: false,
        });
        Ok(id)
    }

    async fn start_reserved<F>(&self, stream_id: u64, task: F) -> Result<u64, String>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut lifecycle = self.lifecycle.lock().await;
        match lifecycle.latest_reservation.as_mut() {
            Some(reservation) if reservation.id == stream_id && !reservation.consumed => {
                reservation.consumed = true;
            }
            Some(reservation) if reservation.id == stream_id => {
                return Err(format!(
                    "UI state stream reservation {stream_id} was already consumed"
                ));
            }
            _ => {
                return Err(format!("UI state stream reservation {stream_id} is stale"));
            }
        }

        if let Some(previous) = lifecycle.active.take() {
            previous.task.abort();
            let _ = previous.task.await;
        }
        lifecycle.active = Some(ActiveUiStateStream {
            id: stream_id,
            task: tokio::spawn(task),
        });
        Ok(stream_id)
    }

    async fn stop(&self, stream_id: u64) {
        let mut lifecycle = self.lifecycle.lock().await;
        if !matches!(
            lifecycle.active.as_ref(),
            Some(active) if active.id == stream_id
        ) {
            return;
        }
        if let Some(active) = lifecycle.active.take() {
            active.task.abort();
            let _ = active.task.await;
        }
    }
}

struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tauri::command]
pub async fn reserve_ui_state_stream(app: AppHandle) -> Result<u64, String> {
    app.state::<UiStateStreamState>().reserve().await
}

#[tauri::command]
pub async fn start_ui_state_stream(
    app: AppHandle,
    stream_id: u64,
    cursor: Option<UiCursor>,
) -> Result<u64, String> {
    let state = app.state::<UiStateStreamState>();
    let app_for_task = app.clone();
    start_ui_state_stream_core(&state, stream_id, cursor, true, move |cursor| async move {
        if let Err(error) = proxy_ui_state(app_for_task, cursor).await {
            eprintln!("UI state stream ended: {error}");
        }
    })
    .await
}

pub async fn start_ui_state_stream_core<Dispatch, DispatchFuture>(
    state: &UiStateStreamState,
    stream_id: u64,
    cursor: Option<UiCursor>,
    // Gateway availability is advisory here: this is the definitive Tauri command path.
    _network_gateway_available: bool,
    dispatch: Dispatch,
) -> Result<u64, String>
where
    Dispatch: FnOnce(Option<UiCursor>) -> DispatchFuture + Send + 'static,
    DispatchFuture: Future<Output = ()> + Send + 'static,
{
    state
        .start_reserved(stream_id, async move {
            dispatch(cursor).await;
        })
        .await
}

#[tauri::command]
pub async fn stop_ui_state_stream(app: AppHandle, stream_id: u64) -> Result<(), String> {
    app.state::<UiStateStreamState>().stop(stream_id).await;
    Ok(())
}

async fn proxy_ui_state(app: AppHandle, cursor: Option<UiCursor>) -> anyhow::Result<()> {
    let mut subscription = daemon_client::subscribe_ui_state(cursor).await?;
    let latest_cursor = std::sync::Arc::new(Mutex::new(None::<UiCursor>));
    let heartbeat_cursor = latest_cursor.clone();
    let heartbeat_app = app.clone();
    let heartbeat_task = tokio::spawn(async move {
        let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        heartbeat.tick().await;
        loop {
            heartbeat.tick().await;
            let cursor = *heartbeat_cursor
                .lock()
                .expect("UI heartbeat cursor lock poisoned");
            if let Some(cursor) = cursor {
                let envelope = UiEnvelope::Heartbeat {
                    boot_id: cursor.boot_id,
                    revision: cursor.revision,
                    sent_at_ms: timestamp_ms(),
                };
                let _ = heartbeat_app.emit(UI_STATE_EVENT, envelope);
            }
        }
    });
    let _heartbeat_guard = AbortOnDrop(heartbeat_task.abort_handle());

    while let Some(envelope) = subscription.recv().await? {
        *latest_cursor
            .lock()
            .expect("UI heartbeat cursor lock poisoned") = Some(envelope.cursor());
        app.emit(UI_STATE_EVENT, envelope)?;
    }
    Ok(())
}

fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::sync::Notify;

    struct DropMarker(Arc<AtomicUsize>);

    impl Drop for DropMarker {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    async fn reserve_and_start<F>(state: &UiStateStreamState, task: F) -> u64
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let stream_id = state.reserve().await.unwrap();
        state.start_reserved(stream_id, task).await.unwrap()
    }

    #[tokio::test]
    async fn ui_state_stream_second_start_cancels_and_awaits_the_prior_task() {
        let state = UiStateStreamState::default();
        let first_started = Arc::new(Notify::new());
        let first_dropped = Arc::new(AtomicUsize::new(0));
        let second_started = Arc::new(AtomicUsize::new(0));

        let first_id = reserve_and_start(&state, {
            let first_started = first_started.clone();
            let first_dropped = first_dropped.clone();
            async move {
                let _marker = DropMarker(first_dropped);
                first_started.notify_one();
                std::future::pending::<()>().await;
            }
        })
        .await;
        first_started.notified().await;

        let second_id = reserve_and_start(&state, {
            let second_started = second_started.clone();
            async move {
                second_started.fetch_add(1, Ordering::SeqCst);
                std::future::pending::<()>().await;
            }
        })
        .await;
        tokio::task::yield_now().await;

        assert_ne!(first_id, second_id);
        assert_eq!(first_dropped.load(Ordering::SeqCst), 1);
        assert_eq!(second_started.load(Ordering::SeqCst), 1);
        state.stop(second_id).await;
    }

    #[tokio::test]
    async fn stale_owner_stop_cannot_cancel_replacement_and_matching_stop_is_idempotent() {
        let state = UiStateStreamState::default();
        let first_started = Arc::new(Notify::new());
        let first_id = reserve_and_start(&state, {
            let first_started = first_started.clone();
            async move {
                first_started.notify_one();
                std::future::pending::<()>().await;
            }
        })
        .await;
        first_started.notified().await;

        let replacement_dropped = Arc::new(AtomicUsize::new(0));
        let replacement_started = Arc::new(Notify::new());
        let replacement_id = reserve_and_start(&state, {
            let replacement_dropped = replacement_dropped.clone();
            let replacement_started = replacement_started.clone();
            async move {
                let _marker = DropMarker(replacement_dropped);
                replacement_started.notify_one();
                std::future::pending::<()>().await;
            }
        })
        .await;
        replacement_started.notified().await;

        state.stop(first_id).await;
        tokio::task::yield_now().await;
        assert_eq!(replacement_dropped.load(Ordering::SeqCst), 0);

        state.stop(replacement_id).await;
        state.stop(replacement_id).await;
        assert_eq!(replacement_dropped.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn gateway_availability_cannot_swallow_tauri_stream_dispatch() {
        let state = UiStateStreamState::default();
        let dispatched = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let stream_id = state.reserve().await.unwrap();

        let stream_id = start_ui_state_stream_core(&state, stream_id, None, true, {
            let dispatched = dispatched.clone();
            let started = started.clone();
            move |_| async move {
                dispatched.fetch_add(1, Ordering::SeqCst);
                started.notify_one();
                std::future::pending::<()>().await;
            }
        })
        .await
        .unwrap();
        started.notified().await;

        assert_eq!(dispatched.load(Ordering::SeqCst), 1);
        state.stop(stream_id).await;
    }

    #[tokio::test]
    async fn late_stale_start_cannot_replace_newer_active_reservation() {
        let state = UiStateStreamState::default();
        let initial_started = Arc::new(Notify::new());
        let initial_id = reserve_and_start(&state, {
            let initial_started = initial_started.clone();
            async move {
                initial_started.notify_one();
                std::future::pending::<()>().await;
            }
        })
        .await;
        initial_started.notified().await;

        let stale_id = state.reserve().await.unwrap();
        let replacement_id = state.reserve().await.unwrap();
        let replacement_dropped = Arc::new(AtomicUsize::new(0));
        let replacement_started = Arc::new(Notify::new());
        state
            .start_reserved(replacement_id, {
                let replacement_dropped = replacement_dropped.clone();
                let replacement_started = replacement_started.clone();
                async move {
                    let _marker = DropMarker(replacement_dropped);
                    replacement_started.notify_one();
                    std::future::pending::<()>().await;
                }
            })
            .await
            .unwrap();
        replacement_started.notified().await;

        let stale_start = state
            .start_reserved(stale_id, async {
                panic!("stale reservation task must never start");
            })
            .await;
        assert!(stale_start.is_err());
        assert_ne!(initial_id, replacement_id);
        assert_eq!(replacement_dropped.load(Ordering::SeqCst), 0);
        state.stop(replacement_id).await;
    }

    #[tokio::test]
    async fn duplicate_start_is_rejected_without_replacing_active_task() {
        let state = UiStateStreamState::default();
        let active_dropped = Arc::new(AtomicUsize::new(0));
        let active_started = Arc::new(Notify::new());
        let stream_id = state.reserve().await.unwrap();
        state
            .start_reserved(stream_id, {
                let active_dropped = active_dropped.clone();
                let active_started = active_started.clone();
                async move {
                    let _marker = DropMarker(active_dropped);
                    active_started.notify_one();
                    std::future::pending::<()>().await;
                }
            })
            .await
            .unwrap();
        active_started.notified().await;

        let duplicate = state
            .start_reserved(stream_id, async {
                panic!("duplicate reservation task must never start");
            })
            .await;
        assert!(duplicate.is_err());
        assert_eq!(active_dropped.load(Ordering::SeqCst), 0);
        state.stop(stream_id).await;
    }

    #[tokio::test]
    async fn reserving_without_starting_does_not_terminate_active_stream() {
        let state = UiStateStreamState::default();
        let active_dropped = Arc::new(AtomicUsize::new(0));
        let active_started = Arc::new(Notify::new());
        let active_id = reserve_and_start(&state, {
            let active_dropped = active_dropped.clone();
            let active_started = active_started.clone();
            async move {
                let _marker = DropMarker(active_dropped);
                active_started.notify_one();
                std::future::pending::<()>().await;
            }
        })
        .await;
        active_started.notified().await;

        let pending_id = state.reserve().await.unwrap();
        assert_ne!(active_id, pending_id);
        assert_eq!(active_dropped.load(Ordering::SeqCst), 0);
        state.stop(active_id).await;
        assert_eq!(active_dropped.load(Ordering::SeqCst), 1);
    }
}
