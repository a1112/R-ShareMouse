use anyhow::Result;

/// A platform event that requires immediately releasing remote input state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemSafetyEvent {
    /// The current interactive Windows session was locked.
    SessionLocked,
    /// The operating system is about to suspend.
    SystemSuspending,
}

const WINDOWS_WM_POWERBROADCAST: u32 = 0x0218;
const WINDOWS_WM_WTSSESSION_CHANGE: u32 = 0x02B1;
const WINDOWS_PBT_APMSUSPEND: usize = 4;
const WINDOWS_WTS_SESSION_LOCK: usize = 7;

fn classify_windows_system_message(
    message: u32,
    wparam: usize,
    _lparam: isize,
) -> Option<SystemSafetyEvent> {
    match (message, wparam) {
        (WINDOWS_WM_WTSSESSION_CHANGE, WINDOWS_WTS_SESSION_LOCK) => {
            Some(SystemSafetyEvent::SessionLocked)
        }
        (WINDOWS_WM_POWERBROADCAST, WINDOWS_PBT_APMSUSPEND) => {
            Some(SystemSafetyEvent::SystemSuspending)
        }
        _ => None,
    }
}

/// Owns the native system-event watcher for the current process.
///
/// On platforms without an implemented native event source, `start` succeeds
/// with a no-op handle and `is_supported` returns `false`.
pub struct SystemSafetyWatcher {
    #[cfg(windows)]
    inner: windows_watcher::WindowsSafetyWatcher,
}

impl SystemSafetyWatcher {
    /// Starts watching for session lock and system suspend events.
    pub fn start<F>(callback: F) -> Result<Self>
    where
        F: Fn(SystemSafetyEvent) + Send + 'static,
    {
        #[cfg(windows)]
        {
            return Ok(Self {
                inner: windows_watcher::WindowsSafetyWatcher::start(Box::new(callback))?,
            });
        }

        #[cfg(not(windows))]
        {
            let _ = callback;
            Ok(Self {})
        }
    }

    /// Reports whether this handle has a native safety-event source.
    #[must_use]
    pub fn is_supported(&self) -> bool {
        #[cfg(windows)]
        {
            let _ = &self.inner;
            true
        }

        #[cfg(not(windows))]
        {
            false
        }
    }
}

#[cfg(windows)]
mod windows_watcher {
    use std::{
        cell::RefCell,
        sync::{
            atomic::{AtomicU64, Ordering},
            mpsc,
        },
        thread::{self, JoinHandle},
    };

    use anyhow::{anyhow, Context, Result};
    use windows::{
        core::w,
        Win32::{
            Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
            System::{
                LibraryLoader::GetModuleHandleW,
                RemoteDesktop::{
                    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
                    NOTIFY_FOR_THIS_SESSION,
                },
            },
            UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
                PostMessageW, PostQuitMessage, RegisterClassW, TranslateMessage, UnregisterClassW,
                CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, HMENU, MSG, WM_CLOSE, WNDCLASSW,
                WS_EX_TOOLWINDOW, WS_OVERLAPPED,
            },
        },
    };

    use super::{classify_windows_system_message, SystemSafetyEvent};

    type SafetyCallback = Box<dyn Fn(SystemSafetyEvent) + Send + 'static>;

    static NEXT_CLASS_ID: AtomicU64 = AtomicU64::new(1);

    thread_local! {
        static CALLBACK: RefCell<Option<SafetyCallback>> = RefCell::new(None);
    }

    pub(super) struct WindowsSafetyWatcher {
        window: usize,
        thread: Option<JoinHandle<()>>,
    }

    impl WindowsSafetyWatcher {
        pub(super) fn start(callback: SafetyCallback) -> Result<Self> {
            let (startup_tx, startup_rx) = mpsc::sync_channel::<Result<usize, String>>(1);
            let class_id = NEXT_CLASS_ID.fetch_add(1, Ordering::Relaxed);
            let thread = thread::Builder::new()
                .name("rshare-system-safety".to_owned())
                .spawn(move || {
                    let result = run_message_loop(callback, class_id, &startup_tx);
                    if let Err(error) = result {
                        let _ = startup_tx.send(Err(format!("{error:#}")));
                    }
                })
                .context("failed to spawn system safety watcher thread")?;

            match startup_rx.recv() {
                Ok(Ok(window)) => Ok(Self {
                    window,
                    thread: Some(thread),
                }),
                Ok(Err(error)) => {
                    let _ = thread.join();
                    Err(anyhow!(error))
                }
                Err(_) => {
                    let _ = thread.join();
                    Err(anyhow!(
                        "system safety watcher exited before initialization"
                    ))
                }
            }
        }
    }

    impl Drop for WindowsSafetyWatcher {
        fn drop(&mut self) {
            // SAFETY: `window` is created by the owned watcher thread and remains
            // valid until that thread receives WM_CLOSE and performs cleanup.
            let _ = unsafe {
                PostMessageW(
                    HWND(self.window as *mut core::ffi::c_void),
                    WM_CLOSE,
                    WPARAM(0),
                    LPARAM(0),
                )
            };
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn run_message_loop(
        callback: SafetyCallback,
        class_id: u64,
        startup_tx: &mpsc::SyncSender<Result<usize, String>>,
    ) -> Result<()> {
        CALLBACK.with(|slot| {
            *slot.borrow_mut() = Some(callback);
        });

        let class_name = format!("RShareSystemSafetyWindow-{class_id}\0")
            .encode_utf16()
            .collect::<Vec<_>>();

        // SAFETY: Win32 handles and pointers remain valid for the lifetime of
        // this thread. The class name buffer outlives registration, creation,
        // message processing, and unregistration.
        unsafe {
            let module = GetModuleHandleW(None).context("failed to get process module")?;
            let instance = HINSTANCE(module.0);
            let window_class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: windows::core::PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };

            if RegisterClassW(&window_class) == 0 {
                return Err(anyhow!("failed to register system safety window class"));
            }

            let window = match CreateWindowExW(
                WS_EX_TOOLWINDOW,
                windows::core::PCWSTR(class_name.as_ptr()),
                w!("R-ShareMouse System Safety"),
                WS_OVERLAPPED,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                0,
                0,
                HWND(core::ptr::null_mut()),
                HMENU(core::ptr::null_mut()),
                instance,
                None,
            ) {
                Ok(window) => window,
                Err(error) => {
                    let _ = UnregisterClassW(windows::core::PCWSTR(class_name.as_ptr()), instance);
                    return Err(error).context("failed to create system safety window");
                }
            };

            if let Err(error) = WTSRegisterSessionNotification(window, NOTIFY_FOR_THIS_SESSION) {
                let _ = DestroyWindow(window);
                let _ = UnregisterClassW(windows::core::PCWSTR(class_name.as_ptr()), instance);
                return Err(error).context("failed to register Windows session notifications");
            }

            if startup_tx.send(Ok(window.0 as usize)).is_err() {
                let _ = WTSUnRegisterSessionNotification(window);
                let _ = DestroyWindow(window);
                let _ = UnregisterClassW(windows::core::PCWSTR(class_name.as_ptr()), instance);
                return Ok(());
            }

            let mut message = MSG::default();
            let message_loop_error = loop {
                let result = GetMessageW(&mut message, HWND(core::ptr::null_mut()), 0, 0);
                if result.0 == -1 {
                    break Some(windows::core::Error::from_win32());
                }
                if result.0 == 0 {
                    break None;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            };

            // Session notification registration is tied to this HWND and must
            // be removed while the window still exists.
            let _ = WTSUnRegisterSessionNotification(window);
            let _ = DestroyWindow(window);
            let _ = UnregisterClassW(windows::core::PCWSTR(class_name.as_ptr()), instance);

            if let Some(error) = message_loop_error {
                return Err(error).context("system safety message loop failed");
            }
        }

        CALLBACK.with(|slot| {
            slot.borrow_mut().take();
        });
        Ok(())
    }

    unsafe extern "system" fn window_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_CLOSE {
            PostQuitMessage(0);
            return LRESULT(0);
        }

        if let Some(event) = classify_windows_system_message(message, wparam.0, lparam.0) {
            CALLBACK.with(|slot| {
                let callback = { slot.borrow_mut().take() };
                if let Some(callback) = callback {
                    let _ =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(event)));
                    slot.borrow_mut().replace(callback);
                }
            });
            return LRESULT(0);
        }

        DefWindowProcW(window, message, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_windows_system_message, SystemSafetyEvent};

    const WM_WTSSESSION_CHANGE: u32 = 0x02B1;
    const WM_POWERBROADCAST: u32 = 0x0218;
    const WTS_SESSION_LOCK: usize = 7;
    const WTS_SESSION_UNLOCK: usize = 8;
    const PBT_APMSUSPEND: usize = 4;
    const PBT_APMRESUMEAUTOMATIC: usize = 18;

    #[test]
    fn classifies_session_lock_as_safety_event() {
        assert_eq!(
            classify_windows_system_message(WM_WTSSESSION_CHANGE, WTS_SESSION_LOCK, 0,),
            Some(SystemSafetyEvent::SessionLocked),
        );
    }

    #[test]
    fn classifies_power_suspend_as_safety_event() {
        assert_eq!(
            classify_windows_system_message(WM_POWERBROADCAST, PBT_APMSUSPEND, 0),
            Some(SystemSafetyEvent::SystemSuspending),
        );
    }

    #[test]
    fn ignores_session_unlock() {
        assert_eq!(
            classify_windows_system_message(WM_WTSSESSION_CHANGE, WTS_SESSION_UNLOCK, 0,),
            None,
        );
    }

    #[test]
    fn ignores_power_resume() {
        assert_eq!(
            classify_windows_system_message(WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC, 0,),
            None,
        );
    }

    #[test]
    fn ignores_unknown_messages() {
        assert_eq!(
            classify_windows_system_message(0xFFFF, WTS_SESSION_LOCK, 0),
            None,
        );
    }
}
