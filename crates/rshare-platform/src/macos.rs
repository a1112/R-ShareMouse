//! macOS platform-specific implementations.

#[cfg(any(test, target_os = "macos"))]
const MAX_MACOS_UNICODE_SCALARS_PER_EVENT: usize = 20;

#[cfg(any(test, target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MacosTextOperation<'a> {
    Unicode(&'a str),
    TabClick,
    GuardedControl(char),
}

#[cfg(test)]
impl MacosTextOperation<'_> {
    pub(crate) fn unicode_scalar_count(&self) -> usize {
        match self {
            Self::Unicode(text) => text.chars().count(),
            Self::TabClick | Self::GuardedControl(_) => 1,
        }
    }
}

#[cfg(any(test, target_os = "macos"))]
pub(crate) fn macos_text_operation_plan(mut text: &str) -> Vec<MacosTextOperation<'_>> {
    let mut operations = Vec::new();
    while !text.is_empty() {
        let chunk_end = text
            .char_indices()
            .nth(MAX_MACOS_UNICODE_SCALARS_PER_EVENT)
            .map(|(index, _)| index)
            .unwrap_or(text.len());
        let mut chunk = &text[..chunk_end];
        text = &text[chunk_end..];

        loop {
            match chunk.chars().next() {
                Some('\t') => operations.push(MacosTextOperation::TabClick),
                Some(control @ ('\r' | '\n')) => {
                    operations.push(MacosTextOperation::GuardedControl(control));
                }
                _ => break,
            }
            chunk = &chunk[1..];
        }

        if !chunk.is_empty() {
            operations.push(MacosTextOperation::Unicode(chunk));
        }
    }
    operations
}

cfg_if::cfg_if! {
    if #[cfg(target_os = "macos")] {
        pub use macos_impl::*;
    }
}

#[cfg(target_os = "macos")]
mod macos_impl {
    use super::{macos_text_operation_plan, MacosTextOperation};
    use anyhow::{anyhow, bail, Context, Result};
    use cocoa::appkit::{NSFilenamesPboardType, NSPasteboard, NSPasteboardItem, NSURLPboardType};
    use cocoa::base::{id, nil};
    use cocoa::foundation::{NSArray, NSString};
    use core_foundation::base::TCFType;
    use core_foundation::runloop::CFRunLoop;
    use core_foundation::string::CFString;
    use core_graphics::display::CGDisplay;
    use core_graphics::event::{
        CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
        CGEventType, CGMouseButton, EventField, KeyCode as MacKeyCode, ScrollEventUnit,
    };
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::{CGPoint, CGRect};
    use rshare_common::ScreenInfo;
    use std::collections::BTreeSet;
    use std::ffi::CStr;
    use std::panic::{self, AssertUnwindSafe};
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc, Mutex, MutexGuard};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    /// Marker reserved for events posted by R-ShareMouse's CoreGraphics injector.
    ///
    /// This is intentionally a per-event marker rather than a broad
    /// source-class filter: accessibility tools and other legitimate synthetic
    /// events must still be observed by the capture path.
    const RSHARE_EVENT_SOURCE_MARKER: i64 = 0x5253_4841_5245;
    const MACOS_INPUT_EVENT_TAP_RUN_LOOP_MODE_NAME: &str = "com.rshare.input-event-tap";

    static LOCAL_INPUT_SUPPRESSED: AtomicBool = AtomicBool::new(false);

    fn macos_input_event_tap_location() -> CGEventTapLocation {
        CGEventTapLocation::Session
    }

    fn macos_input_event_tap_run_loop_mode() -> CFString {
        // `kCFRunLoopCommonModes` is a pseudo-mode used when registering
        // sources, not a mode that can be passed to `CFRunLoopRunInMode`.
        // Running that pseudo-mode returns `Finished` immediately, which
        // makes the listener appear to start and then stop synchronously.
        CFString::from_static_string(MACOS_INPUT_EVENT_TAP_RUN_LOOP_MODE_NAME)
    }

    /// Non-interactive snapshot of the permissions needed for the native input loop.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MacosInputPermissionState {
        input_monitoring: bool,
        accessibility: bool,
    }

    /// The macOS privacy pane that grants one of the input permissions.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum MacosInputPermissionKind {
        Accessibility,
        InputMonitoring,
    }

    impl MacosInputPermissionKind {
        fn settings_url(self) -> &'static str {
            match self {
                Self::Accessibility => {
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
                }
                Self::InputMonitoring => {
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
                }
            }
        }
    }

    impl MacosInputPermissionState {
        pub fn can_capture(self) -> bool {
            self.input_monitoring
        }

        pub fn can_inject(self) -> bool {
            self.accessibility
        }

        pub fn is_ready(self) -> bool {
            self.can_capture() && self.can_inject()
        }

        pub fn actionable_error(self) -> Option<&'static str> {
            match (self.can_capture(), self.can_inject()) {
                (true, true) => None,
                (false, false) => Some(
                    "macOS Input Monitoring and Accessibility permissions are required for input sharing",
                ),
                (false, true) => {
                    Some("macOS Input Monitoring permission is required to capture input events")
                }
                (true, false) => {
                    Some("macOS Accessibility permission is required to post input events")
                }
            }
        }

        /// Construct a permission snapshot from preflight results.
        ///
        /// This is useful for deterministic callers and does not request any
        /// macOS privacy prompt.
        pub fn from_preflight(input_monitoring: bool, accessibility: bool) -> Self {
            Self {
                input_monitoring,
                accessibility,
            }
        }
    }

    /// Read the native input permissions without presenting system prompts.
    pub fn macos_input_permission_state() -> MacosInputPermissionState {
        MacosInputPermissionState::from_preflight(
            permissions::can_listen_events(),
            permissions::can_post_events(),
        )
    }

    /// Ask CoreGraphics to request any missing TCC permissions, then return a
    /// fresh non-interactive snapshot. The caller can follow up by opening the
    /// exact System Settings pane when macOS still requires a manual toggle.
    pub fn request_macos_input_permissions() -> MacosInputPermissionState {
        let current = macos_input_permission_state();
        if !current.can_capture() {
            let _ = permissions::request_listen_events();
        }
        if !current.can_inject() {
            let _ = permissions::request_post_events();
        }
        macos_input_permission_state()
    }

    /// Open the exact macOS Privacy & Security pane for the requested input
    /// permission. The app remains responsible for refreshing the snapshot
    /// after the user enables the permission.
    pub fn open_macos_input_permission_settings(
        permission: MacosInputPermissionKind,
    ) -> Result<()> {
        let url = permission.settings_url();
        let status = Command::new("open")
            .arg(url)
            .status()
            .with_context(|| format!("failed to open macOS permission settings: {url}"))?;
        if !status.success() {
            bail!("macOS permission settings exited with status {status}");
        }
        Ok(())
    }

    /// Toggle delivery of physical events to the local macOS session while a
    /// remote control session owns local input. R-ShareMouse's own marked
    /// injected events are always allowed through.
    pub fn set_macos_local_input_suppressed(enabled: bool) {
        LOCAL_INPUT_SUPPRESSED.store(enabled, Ordering::Release);
    }

    /// Input event captured by the native macOS listener.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum MacosInputEvent {
        MouseMove { x: i32, y: i32 },
        MouseButton { button: u8, down: bool },
        MouseWheel { delta_x: i32, delta_y: i32 },
        Key { keycode: u32, down: bool },
    }

    /// Read-only status for a running macOS input listener.
    ///
    /// This intentionally contains no callback or worker handle, so daemon
    /// health monitoring can retain it without participating in listener
    /// shutdown or allowing a status read to panic.
    #[derive(Debug, Clone)]
    pub struct MacosInputListenerStatus {
        running: Arc<AtomicBool>,
        capture_fault_reported: Arc<AtomicBool>,
    }

    impl MacosInputListenerStatus {
        pub fn is_running(&self) -> bool {
            self.running.load(Ordering::Acquire)
        }

        pub fn has_capture_fault(&self) -> bool {
            self.capture_fault_reported.load(Ordering::Acquire)
        }
    }

    /// macOS input listener using a CoreGraphics event tap.
    pub struct MacosInputListener {
        running: Arc<AtomicBool>,
        capture_fault_reported: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    struct RunningFlagReset {
        running: Arc<AtomicBool>,
        capture_started: Arc<AtomicBool>,
        capture_fault_reported: Arc<AtomicBool>,
        fault_callback: Arc<dyn Fn() + Send + Sync>,
    }

    impl RunningFlagReset {
        fn new(
            running: Arc<AtomicBool>,
            capture_started: Arc<AtomicBool>,
            capture_fault_reported: Arc<AtomicBool>,
            fault_callback: Arc<dyn Fn() + Send + Sync>,
        ) -> Self {
            Self {
                running,
                capture_started,
                capture_fault_reported,
                fault_callback,
            }
        }
    }

    impl Drop for RunningFlagReset {
        fn drop(&mut self) {
            let was_running = self.running.swap(false, Ordering::SeqCst);
            if was_running && self.capture_started.load(Ordering::Acquire) {
                // A panic or an unexpected worker return can lose a physical
                // release just like a disabled tap. Reuse the same once-only
                // discontinuity lane; normal stop/startup-error paths clear
                // `running` before joining and therefore do not report here.
                report_capture_discontinuity(
                    &self.capture_fault_reported,
                    self.fault_callback.as_ref(),
                );
            }
        }
    }

    impl MacosInputListener {
        pub fn new() -> Self {
            Self {
                running: Arc::new(AtomicBool::new(false)),
                capture_fault_reported: Arc::new(AtomicBool::new(false)),
                worker: None,
            }
        }

        /// Start listening and send captured events through the provided channel.
        pub fn start(&mut self, sender: mpsc::Sender<MacosInputEvent>) -> Result<()> {
            self.start_with_callback(move |event| {
                let _ = sender.send(event);
            })
        }

        /// Start listening and invoke a callback for each captured event.
        pub fn start_with_callback<F>(&mut self, callback: F) -> Result<()>
        where
            F: Fn(MacosInputEvent) + Send + Sync + 'static,
        {
            self.start_with_callback_and_fault(callback, || {})
        }

        /// Start listening with a callback for capture discontinuities.
        ///
        /// A CoreGraphics tap can be disabled asynchronously. The fault
        /// callback is invoked at most once for the worker lifetime, before
        /// any possible asynchronous recovery is attempted.
        pub fn start_with_callback_and_fault<F, G>(
            &mut self,
            callback: F,
            fault_callback: G,
        ) -> Result<()>
        where
            F: Fn(MacosInputEvent) + Send + Sync + 'static,
            G: Fn() + Send + Sync + 'static,
        {
            if self.is_running() {
                return Ok(());
            }
            self.reap_finished_worker()?;

            let permissions = macos_input_permission_state();
            if !permissions.is_ready() {
                bail!(
                    "{}",
                    permissions
                        .actionable_error()
                        .expect("missing macOS permission")
                );
            }

            self.capture_fault_reported.store(false, Ordering::Release);
            self.running.store(true, Ordering::SeqCst);
            let running = self.running.clone();
            let callback = Arc::new(callback);
            let fault_callback = Arc::new(fault_callback);
            let tap_reenable_requested = Arc::new(AtomicBool::new(false));
            let tap_reenable_for_callback = tap_reenable_requested.clone();
            let capture_fault_reported = self.capture_fault_reported.clone();
            let capture_fault_reported_for_callback = capture_fault_reported.clone();
            let capture_started = Arc::new(AtomicBool::new(false));
            let capture_started_for_worker = capture_started.clone();
            let (startup_tx, startup_rx) = mpsc::sync_channel(1);
            let worker = match thread::Builder::new()
                .name("rshare-macos-input-listener".to_owned())
                .spawn(move || {
                    let _running_flag_reset = RunningFlagReset::new(
                        running.clone(),
                        capture_started_for_worker,
                        capture_fault_reported.clone(),
                        fault_callback.clone(),
                    );
                    let current_loop = CFRunLoop::get_current();
                    let callback = callback.clone();
                    let fault_callback_for_callback = fault_callback.clone();
                    let running_for_callback = running.clone();
                    let modifier_state = Arc::new(std::sync::Mutex::new(ModifierState::default()));

                    let tap = match CGEventTap::new(
                        macos_input_event_tap_location(),
                        CGEventTapPlacement::HeadInsertEventTap,
                        CGEventTapOptions::Default,
                        vec![
                            CGEventType::MouseMoved,
                            CGEventType::LeftMouseDragged,
                            CGEventType::RightMouseDragged,
                            CGEventType::OtherMouseDragged,
                            CGEventType::LeftMouseDown,
                            CGEventType::LeftMouseUp,
                            CGEventType::RightMouseDown,
                            CGEventType::RightMouseUp,
                            CGEventType::OtherMouseDown,
                            CGEventType::OtherMouseUp,
                            CGEventType::ScrollWheel,
                            CGEventType::KeyDown,
                            CGEventType::KeyUp,
                            CGEventType::FlagsChanged,
                        ],
                        move |_proxy, event_type, event| {
                            handle_event_tap_callback(
                                event_type,
                                event,
                                &running_for_callback,
                                &tap_reenable_for_callback,
                                &capture_fault_reported_for_callback,
                                callback.as_ref(),
                                fault_callback_for_callback.as_ref(),
                                modifier_state.as_ref(),
                            )
                        },
                    ) {
                        Ok(tap) => tap,
                        Err(_) => {
                            let _ = startup_tx
                                .send(Err("failed to create macOS CGEventTap".to_owned()));
                            running.store(false, Ordering::SeqCst);
                            return;
                        }
                    };

                    let source = match tap.mach_port.create_runloop_source(0) {
                        Ok(source) => source,
                        Err(_) => {
                            let _ =
                                startup_tx
                                    .send(Err("failed to create macOS event tap run-loop source"
                                        .to_owned()));
                            running.store(false, Ordering::SeqCst);
                            return;
                        }
                    };

                    let run_loop_mode = macos_input_event_tap_run_loop_mode();
                    let run_loop_mode_ref = run_loop_mode.as_concrete_TypeRef();
                    current_loop.add_source(&source, run_loop_mode_ref);
                    tap.enable();
                    tracing::info!("macOS input listener started");
                    capture_started.store(true, Ordering::Release);
                    if startup_tx.send(Ok(())).is_err() {
                        running.store(false, Ordering::SeqCst);
                    }

                    while running.load(Ordering::SeqCst) {
                        if tap_reenable_requested.swap(false, Ordering::Acquire) {
                            tap.enable();
                        }
                        let result = CFRunLoop::run_in_mode(
                            run_loop_mode_ref,
                            Duration::from_millis(100),
                            false,
                        );
                        if handle_abnormal_run_loop_result(
                            result,
                            &running,
                            &capture_started,
                            &capture_fault_reported,
                            fault_callback.as_ref(),
                        ) {
                            break;
                        }
                    }

                    current_loop.remove_source(&source, run_loop_mode_ref);
                    tracing::info!("macOS input listener stopped");
                }) {
                Ok(worker) => worker,
                Err(error) => {
                    self.running.store(false, Ordering::SeqCst);
                    return Err(anyhow!("failed to spawn macOS input listener: {error}"));
                }
            };

            match startup_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(())) => {
                    self.worker = Some(worker);
                    Ok(())
                }
                Ok(Err(error)) => {
                    self.running.store(false, Ordering::SeqCst);
                    let _ = worker.join();
                    bail!("{error}")
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.abandon_timed_out_worker(worker);
                    bail!("timed out waiting for macOS CGEventTap startup")
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.running.store(false, Ordering::SeqCst);
                    let _ = worker.join();
                    bail!("macOS input listener exited before CGEventTap startup completed")
                }
            }
        }

        fn abandon_timed_out_worker(&mut self, worker: JoinHandle<()>) {
            self.running.store(false, Ordering::SeqCst);
            // A worker that did not complete startup may be blocked in a CoreGraphics
            // call. Drop its handle after setting the stop flag so startup remains
            // bounded; replacing the token prevents a later start from sharing it.
            self.running = Arc::new(AtomicBool::new(false));
            drop(worker);
        }

        fn reap_finished_worker(&mut self) -> Result<()> {
            let Some(worker) = self.worker.take() else {
                return Ok(());
            };
            if worker.is_finished() {
                worker
                    .join()
                    .map_err(|_| anyhow!("macOS input listener thread panicked"))?;
                return Ok(());
            }
            self.worker = Some(worker);
            bail!("macOS input listener worker is still stopping; stop it before restarting")
        }

        pub fn stop(&mut self) -> Result<()> {
            self.running.store(false, Ordering::SeqCst);
            LOCAL_INPUT_SUPPRESSED.store(false, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                worker
                    .join()
                    .map_err(|_| anyhow!("macOS input listener thread panicked"))?;
            }
            Ok(())
        }

        pub fn is_running(&self) -> bool {
            self.running.load(Ordering::SeqCst)
        }

        /// Clone a non-mutating status handle for independent runtime health
        /// monitoring. It remains valid even if the owning listener is later
        /// dropped during shutdown.
        pub fn status_handle(&self) -> MacosInputListenerStatus {
            MacosInputListenerStatus {
                running: self.running.clone(),
                capture_fault_reported: self.capture_fault_reported.clone(),
            }
        }

        /// True when this listener lifetime observed a capture discontinuity.
        pub fn has_capture_fault(&self) -> bool {
            self.capture_fault_reported.load(Ordering::Acquire)
        }
    }

    impl Default for MacosInputListener {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Drop for MacosInputListener {
        fn drop(&mut self) {
            let _ = self.stop();
        }
    }

    /// macOS input emulator using CoreGraphics events.
    pub struct MacosInputEmulator {
        active: bool,
        pressed_buttons: [bool; 6],
    }

    impl MacosInputEmulator {
        pub fn new() -> Self {
            Self {
                active: false,
                pressed_buttons: [false; 6],
            }
        }

        pub fn activate(&mut self) -> Result<()> {
            permissions::ensure_can_post_events()?;
            self.active = true;
            tracing::info!("macOS input emulator activated");
            Ok(())
        }

        pub fn deactivate(&mut self) -> Result<()> {
            self.active = false;
            self.pressed_buttons = [false; 6];
            tracing::info!("macOS input emulator deactivated");
            Ok(())
        }

        pub fn send_mouse_move(&mut self, x: i32, y: i32) -> Result<()> {
            if !self.active {
                return Ok(());
            }

            let (event_type, mouse_button, button_number) =
                macos_mouse_move_event_spec(&self.pressed_buttons);
            let event = CGEvent::new_mouse_event(
                new_event_source()?,
                event_type,
                CGPoint::new(x as f64, y as f64),
                mouse_button,
            )
            .map_err(|_| anyhow!("Failed to create macOS mouse move event"))?;
            event.set_integer_value_field(
                EventField::MOUSE_EVENT_BUTTON_NUMBER,
                button_number as i64,
            );
            post_rshare_injected_event(&event)?;
            Ok(())
        }

        pub fn send_mouse_move_relative(&mut self, dx: i32, dy: i32) -> Result<()> {
            if !self.active {
                return Ok(());
            }

            let position = current_mouse_position()?;
            // With no held button this helper selects CGEventType::MouseMoved;
            // held buttons select the corresponding dragged event.
            let (event_type, mouse_button, button_number) =
                macos_mouse_move_event_spec(&self.pressed_buttons);
            let event = CGEvent::new_mouse_event(
                new_event_source()?,
                event_type,
                CGPoint::new(position.x + dx as f64, position.y + dy as f64),
                mouse_button,
            )
            .map_err(|_| anyhow!("Failed to create macOS relative mouse move event"))?;
            event.set_integer_value_field(
                EventField::MOUSE_EVENT_BUTTON_NUMBER,
                button_number as i64,
            );
            post_rshare_injected_event(&event)?;
            Ok(())
        }

        pub fn send_button(&mut self, button: u8, down: bool) -> Result<()> {
            if !self.active {
                return Ok(());
            }

            let (event_type, mouse_button) = mouse_button_to_cg(button, down)?;
            let pos = current_mouse_position()?;
            let event =
                CGEvent::new_mouse_event(new_event_source()?, event_type, pos, mouse_button)
                    .map_err(|_| anyhow!("Failed to create macOS mouse button event"))?;
            let Some(button_number) = cg_mouse_button_number_for_code(button) else {
                bail!("Unsupported macOS mouse button: {}", button);
            };
            event.set_integer_value_field(
                EventField::MOUSE_EVENT_BUTTON_NUMBER,
                button_number as i64,
            );
            post_rshare_injected_event(&event)?;
            self.pressed_buttons[button as usize] = down;
            Ok(())
        }

        pub fn send_wheel(&mut self, delta_x: i32, delta_y: i32) -> Result<()> {
            if !self.active {
                return Ok(());
            }

            let event = CGEvent::new_scroll_event(
                new_event_source()?,
                ScrollEventUnit::LINE,
                2,
                delta_y,
                delta_x,
                0,
            )
            .map_err(|_| anyhow!("Failed to create macOS scroll event"))?;
            post_rshare_injected_event(&event)?;
            Ok(())
        }

        pub fn send_key(&mut self, keycode: u32, down: bool) -> Result<()> {
            if !self.active {
                return Ok(());
            }

            let keycode = mac_key_code(keycode)?;
            self.send_hardware_key(keycode, down)
        }

        /// Post a key whose value is already a native macOS hardware keycode.
        ///
        /// `send_key` is the canonical-wire boundary; callers that already
        /// normalized a semantic key must use this method so the native code
        /// is not interpreted a second time.
        pub fn send_hardware_key(&mut self, keycode: u16, down: bool) -> Result<()> {
            if !self.active {
                return Ok(());
            }

            let post = hardware_key_post_record(keycode, down);
            let event = CGEvent::new_keyboard_event(new_event_source()?, post.keycode, post.down)
                .map_err(|_| anyhow!("Failed to create macOS keyboard event"))?;
            post_rshare_injected_event(&event)?;
            Ok(())
        }

        /// Commit Unicode text at the current insertion point.
        pub fn send_text(&mut self, text: &str) -> Result<()> {
            if !self.active {
                return Ok(());
            }
            if text.is_empty() {
                return Ok(());
            }

            for operation in macos_text_operation_plan(text) {
                match operation {
                    MacosTextOperation::Unicode(chunk) => self.post_unicode_text(chunk)?,
                    MacosTextOperation::TabClick => {
                        self.send_key(0x09, true)?;
                        self.send_key(0x09, false)?;
                    }
                    MacosTextOperation::GuardedControl('\r') => {
                        self.post_unicode_text("\u{200B}\r")?;
                    }
                    MacosTextOperation::GuardedControl('\n') => {
                        self.post_unicode_text("\u{200B}\n")?;
                    }
                    MacosTextOperation::GuardedControl(_) => {
                        unreachable!("text planner only guards carriage returns and line feeds")
                    }
                }
            }
            Ok(())
        }

        fn post_unicode_text(&mut self, text: &str) -> Result<()> {
            let event = CGEvent::new_keyboard_event(new_event_source()?, 0, true)
                .map_err(|_| anyhow!("Failed to create macOS text commit event"))?;
            event.set_string(text);
            post_rshare_injected_event(&event)?;
            Ok(())
        }
    }

    impl Default for MacosInputEmulator {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Get primary screen information.
    pub fn get_screen_info() -> ScreenInfo {
        display_to_screen_info(CGDisplay::main())
    }

    /// Get all active screens.
    pub fn get_all_screens() -> Vec<ScreenInfo> {
        CGDisplay::active_displays()
            .map(|ids| {
                ids.into_iter()
                    .map(CGDisplay::new)
                    .map(display_to_screen_info)
                    .collect()
            })
            .unwrap_or_else(|_| vec![get_screen_info()])
    }

    /// Read file-list content from the general pasteboard.
    pub fn current_pasteboard_file_list() -> Result<Vec<String>> {
        unsafe {
            let pasteboard = NSPasteboard::generalPasteboard(nil);
            if pasteboard == nil {
                return Ok(Vec::new());
            }

            let mut files = BTreeSet::new();

            let names = NSPasteboard::propertyListForType(pasteboard, NSFilenamesPboardType);
            collect_ns_string_array(names, &mut files);

            let items = pasteboard.pasteboardItems();
            if items != nil {
                for idx in 0..items.count() {
                    let item = items.objectAtIndex(idx);
                    collect_file_url_string(
                        NSPasteboardItem::stringForType(item, NSURLPboardType),
                        &mut files,
                    );

                    let public_file_url = NSString::alloc(nil).init_str("public.file-url");
                    collect_file_url_string(
                        NSPasteboardItem::stringForType(item, public_file_url),
                        &mut files,
                    );
                }
            }

            Ok(files.into_iter().collect())
        }
    }

    #[derive(Default)]
    struct ModifierState {
        pressed: [bool; 10],
    }

    fn convert_cg_event(
        event_type: CGEventType,
        event: &CGEvent,
        modifier_state: &mut ModifierState,
    ) -> Option<MacosInputEvent> {
        match event_type {
            CGEventType::MouseMoved
            | CGEventType::LeftMouseDragged
            | CGEventType::RightMouseDragged
            | CGEventType::OtherMouseDragged => {
                let pos = event.location();
                Some(MacosInputEvent::MouseMove {
                    x: pos.x.round() as i32,
                    y: pos.y.round() as i32,
                })
            }
            CGEventType::LeftMouseDown => Some(MacosInputEvent::MouseButton {
                button: 1,
                down: true,
            }),
            CGEventType::LeftMouseUp => Some(MacosInputEvent::MouseButton {
                button: 1,
                down: false,
            }),
            CGEventType::RightMouseDown => Some(MacosInputEvent::MouseButton {
                button: 3,
                down: true,
            }),
            CGEventType::RightMouseUp => Some(MacosInputEvent::MouseButton {
                button: 3,
                down: false,
            }),
            CGEventType::OtherMouseDown => Some(MacosInputEvent::MouseButton {
                button: cg_mouse_button_number_to_code(
                    event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER),
                )?,
                down: true,
            }),
            CGEventType::OtherMouseUp => Some(MacosInputEvent::MouseButton {
                button: cg_mouse_button_number_to_code(
                    event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER),
                )?,
                down: false,
            }),
            CGEventType::ScrollWheel => Some(MacosInputEvent::MouseWheel {
                delta_x: event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2)
                    as i32,
                delta_y: event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1)
                    as i32,
            }),
            CGEventType::KeyDown => Some(MacosInputEvent::Key {
                keycode: event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u32,
                down: true,
            }),
            CGEventType::FlagsChanged => {
                let keycode =
                    event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u32;
                Some(MacosInputEvent::Key {
                    keycode,
                    down: modifier_flags_changed_down(
                        keycode,
                        event.get_flags().bits(),
                        modifier_state,
                    ),
                })
            }
            CGEventType::KeyUp => Some(MacosInputEvent::Key {
                keycode: event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u32,
                down: false,
            }),
            _ => None,
        }
    }

    fn modifier_flags_changed_down(keycode: u32, flags: u64, state: &mut ModifierState) -> bool {
        let Some((slot, flag)) = modifier_slot_and_flag(keycode) else {
            // FlagsChanged is also emitted for keys such as Fn where CoreGraphics
            // does not expose a stable event flag. Preserve the legacy key-down
            // behavior for those non-routing modifiers.
            return true;
        };
        let flag_is_set = flags & flag != 0;
        let down = if !flag_is_set {
            false
        } else if !state.pressed[slot] {
            true
        } else if let Some(peer) = paired_modifier_slot(slot) {
            // Left and right variants share a CoreGraphics flag. If the changed
            // key was already down while its peer remains down, this event is
            // that key's release rather than a second press.
            !state.pressed[peer]
        } else {
            true
        };
        state.pressed[slot] = down;
        down
    }

    fn modifier_slot_and_flag(keycode: u32) -> Option<(usize, u64)> {
        const ALPHA_SHIFT: u64 = 0x0001_0000;
        const SHIFT: u64 = 0x0002_0000;
        const CONTROL: u64 = 0x0004_0000;
        const ALTERNATE: u64 = 0x0008_0000;
        const COMMAND: u64 = 0x0010_0000;
        const SECONDARY_FN: u64 = 0x0080_0000;

        match keycode {
            0x38 => Some((0, SHIFT)),        // left shift
            0x3C => Some((1, SHIFT)),        // right shift
            0x3B => Some((2, CONTROL)),      // left control
            0x3E => Some((3, CONTROL)),      // right control
            0x3A => Some((4, ALTERNATE)),    // left option
            0x3D => Some((5, ALTERNATE)),    // right option
            0x37 => Some((6, COMMAND)),      // left command
            0x36 => Some((7, COMMAND)),      // right command
            0x39 => Some((8, ALPHA_SHIFT)),  // caps lock
            0x3F => Some((9, SECONDARY_FN)), // fn
            _ => None,
        }
    }

    fn paired_modifier_slot(slot: usize) -> Option<usize> {
        match slot {
            0 => Some(1),
            1 => Some(0),
            2 => Some(3),
            3 => Some(2),
            4 => Some(5),
            5 => Some(4),
            6 => Some(7),
            7 => Some(6),
            _ => None,
        }
    }

    /// Recheck Accessibility immediately before posting. Unlike activation,
    /// normal injection must never present a TCC prompt.
    fn require_macos_accessibility_permission(accessibility: bool) -> Result<()> {
        if accessibility {
            Ok(())
        } else {
            bail!("macOS Accessibility permission is required to post input events")
        }
    }

    fn post_rshare_injected_event(event: &CGEvent) -> Result<()> {
        require_macos_accessibility_permission(permissions::can_post_events())?;
        event.set_integer_value_field(
            EventField::EVENT_SOURCE_USER_DATA,
            RSHARE_EVENT_SOURCE_MARKER,
        );
        event.post(CGEventTapLocation::HID);
        Ok(())
    }

    fn is_rshare_injected_event(event: &CGEvent) -> bool {
        is_rshare_injected_event_marker(
            event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA),
        )
    }

    fn is_rshare_injected_event_marker(marker: i64) -> bool {
        marker == RSHARE_EVENT_SOURCE_MARKER
    }

    fn is_tap_disabled_notification(event_type: CGEventType) -> bool {
        matches!(
            event_type,
            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
        )
    }

    fn is_abnormal_run_loop_result(result: core_foundation::runloop::CFRunLoopRunResult) -> bool {
        matches!(
            result,
            core_foundation::runloop::CFRunLoopRunResult::Stopped
                | core_foundation::runloop::CFRunLoopRunResult::Finished
        )
    }

    fn handle_abnormal_run_loop_result(
        result: core_foundation::runloop::CFRunLoopRunResult,
        running: &AtomicBool,
        capture_started: &AtomicBool,
        capture_fault_reported: &AtomicBool,
        fault_callback: &dyn Fn(),
    ) -> bool {
        if !is_abnormal_run_loop_result(result) {
            return false;
        }

        // `swap` distinguishes an explicit stop from an unexpected run-loop
        // termination. The guard still covers panic/early-return paths.
        let was_running = running.swap(false, Ordering::SeqCst);
        if was_running && capture_started.load(Ordering::Acquire) {
            report_capture_discontinuity(capture_fault_reported, fault_callback);
        }
        true
    }

    fn report_capture_discontinuity(
        capture_fault_reported: &AtomicBool,
        fault_callback: &dyn Fn(),
    ) -> bool {
        // A disabled tap may have lost a physical release. Keep the local
        // session usable immediately; the daemon's reserved ingress fault
        // lane performs the epoch reset and remote release safety action.
        LOCAL_INPUT_SUPPRESSED.store(false, Ordering::Release);
        if !capture_fault_reported.swap(true, Ordering::AcqRel) {
            // This may be invoked from CoreGraphics' unsafe extern callback;
            // no user-provided fault bridge may unwind across that boundary.
            // A failed bridge means the daemon did not receive the required
            // safety fault, so callers must fail closed rather than recover
            // the event tap.
            return panic::catch_unwind(AssertUnwindSafe(fault_callback)).is_ok();
        }

        true
    }

    fn recover_modifier_state(
        modifier_state: &Mutex<ModifierState>,
    ) -> MutexGuard<'_, ModifierState> {
        match modifier_state.lock() {
            Ok(state) => state,
            // A prior callback panic poisons the lock, but callback containment
            // must not turn the next physical event into another FFI panic.
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn handle_event_tap_callback(
        event_type: CGEventType,
        event: &CGEvent,
        running: &AtomicBool,
        tap_reenable_requested: &AtomicBool,
        capture_fault_reported: &AtomicBool,
        callback: &dyn Fn(MacosInputEvent),
        fault_callback: &dyn Fn(),
        modifier_state: &Mutex<ModifierState>,
    ) -> Option<CGEvent> {
        let original_event = event.clone();
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            if handle_tap_disabled_notification(
                event_type,
                running,
                tap_reenable_requested,
                capture_fault_reported,
                fault_callback,
            ) {
                return Some(event.clone());
            }

            let disposition = macos_tap_event_disposition(
                is_rshare_injected_event(event),
                LOCAL_INPUT_SUPPRESSED.load(Ordering::Acquire),
            );
            match disposition {
                MacosTapEventDisposition::CaptureAndKeep
                | MacosTapEventDisposition::CaptureAndDrop => {
                    let mut modifier_state = recover_modifier_state(modifier_state);
                    if let Some(input_event) =
                        convert_cg_event(event_type, event, &mut modifier_state)
                    {
                        callback(input_event);
                    }
                    if matches!(disposition, MacosTapEventDisposition::CaptureAndDrop) {
                        let replacement = event.clone();
                        replacement.set_type(CGEventType::Null);
                        Some(replacement)
                    } else {
                        Some(event.clone())
                    }
                }
                MacosTapEventDisposition::IgnoreAndKeep => Some(event.clone()),
            }
        }));

        match result {
            Ok(event) => event,
            Err(_) => {
                // Fail closed on callback faults: do not suppress this physical
                // event, stop capture, and trigger the once-only safety lane.
                running.store(false, Ordering::SeqCst);
                report_capture_discontinuity(capture_fault_reported, fault_callback);
                Some(original_event)
            }
        }
    }

    fn handle_tap_disabled_notification(
        event_type: CGEventType,
        running: &AtomicBool,
        tap_reenable_requested: &AtomicBool,
        capture_fault_reported: &AtomicBool,
        fault_callback: &dyn Fn(),
    ) -> bool {
        if !is_tap_disabled_notification(event_type) {
            return false;
        }

        if !report_capture_discontinuity(capture_fault_reported, fault_callback) {
            // The fault bridge did not complete, so the daemon cannot perform
            // its release/reset safety action. Do not revive this tap; stop
            // the listener after returning from CoreGraphics instead.
            running.store(false, Ordering::SeqCst);
            return true;
        }

        // The callback only signals; the owning worker re-enables the same tap
        // after CoreGraphics has returned from the callback.
        tap_reenable_requested.store(true, Ordering::Release);
        true
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum MacosTapEventDisposition {
        CaptureAndKeep,
        CaptureAndDrop,
        IgnoreAndKeep,
    }

    fn macos_tap_event_disposition(
        rshare_injected: bool,
        suppression_enabled: bool,
    ) -> MacosTapEventDisposition {
        match (rshare_injected, suppression_enabled) {
            (true, _) => MacosTapEventDisposition::IgnoreAndKeep,
            (false, true) => MacosTapEventDisposition::CaptureAndDrop,
            (false, false) => MacosTapEventDisposition::CaptureAndKeep,
        }
    }

    fn new_event_source() -> Result<CGEventSource> {
        CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| anyhow!("Failed to create macOS event source"))
    }

    fn current_mouse_position() -> Result<CGPoint> {
        CGEvent::new(new_event_source()?)
            .map(|event| event.location())
            .map_err(|_| anyhow!("Failed to read current macOS mouse position"))
    }

    pub fn mouse_button_to_cg(button: u8, down: bool) -> Result<(CGEventType, CGMouseButton)> {
        match (button, down) {
            (1, true) => Ok((CGEventType::LeftMouseDown, CGMouseButton::Left)),
            (1, false) => Ok((CGEventType::LeftMouseUp, CGMouseButton::Left)),
            (2, true) => Ok((CGEventType::OtherMouseDown, CGMouseButton::Center)),
            (2, false) => Ok((CGEventType::OtherMouseUp, CGMouseButton::Center)),
            (3, true) => Ok((CGEventType::RightMouseDown, CGMouseButton::Right)),
            (3, false) => Ok((CGEventType::RightMouseUp, CGMouseButton::Right)),
            (4, true) | (5, true) => Ok((CGEventType::OtherMouseDown, CGMouseButton::Center)),
            (4, false) | (5, false) => Ok((CGEventType::OtherMouseUp, CGMouseButton::Center)),
            _ => bail!("Unsupported macOS mouse button: {}", button),
        }
    }

    fn cg_mouse_button_number_for_code(button: u8) -> Option<u8> {
        match button {
            1 => Some(0),
            2 => Some(2),
            3 => Some(1),
            4 => Some(3),
            5 => Some(4),
            _ => None,
        }
    }

    fn cg_mouse_button_number_to_code(button_number: i64) -> Option<u8> {
        match button_number {
            0 => Some(1),
            1 => Some(3),
            2 => Some(2),
            3 => Some(4),
            4 => Some(5),
            _ => None,
        }
    }

    #[cfg(test)]
    fn macos_mouse_move_event_type(pressed_buttons: &[bool; 6]) -> CGEventType {
        macos_mouse_move_event_spec(pressed_buttons).0
    }

    fn macos_mouse_move_event_spec(
        pressed_buttons: &[bool; 6],
    ) -> (CGEventType, CGMouseButton, u8) {
        if pressed_buttons[1] {
            (CGEventType::LeftMouseDragged, CGMouseButton::Left, 0)
        } else if pressed_buttons[3] {
            (CGEventType::RightMouseDragged, CGMouseButton::Right, 1)
        } else if pressed_buttons[2] || pressed_buttons[4] || pressed_buttons[5] {
            let button_number = if pressed_buttons[2] {
                2
            } else if pressed_buttons[4] {
                3
            } else {
                4
            };
            (
                CGEventType::OtherMouseDragged,
                CGMouseButton::Center,
                button_number,
            )
        } else {
            (CGEventType::MouseMoved, CGMouseButton::Left, 0)
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct HardwareKeyPostRecord {
        keycode: u16,
        down: bool,
    }

    fn hardware_key_post_record(keycode: u16, down: bool) -> HardwareKeyPostRecord {
        HardwareKeyPostRecord { keycode, down }
    }

    pub fn mac_key_code(keycode: u32) -> Result<u16> {
        let mapped = match keycode {
            // Cross-platform special-key values to macOS hardware keycodes.
            0x08 => MacKeyCode::DELETE, // Backspace
            0x09 => MacKeyCode::TAB,
            0x0D => MacKeyCode::RETURN,
            0x1B => MacKeyCode::ESCAPE,
            0x20 => MacKeyCode::SPACE,
            0x21 => MacKeyCode::PAGE_UP,
            0x22 => MacKeyCode::PAGE_DOWN,
            0x23 => MacKeyCode::END,
            0x24 => MacKeyCode::HOME,
            0x25 => MacKeyCode::LEFT_ARROW,
            0x26 => MacKeyCode::UP_ARROW,
            0x27 => MacKeyCode::RIGHT_ARROW,
            0x28 => MacKeyCode::DOWN_ARROW,
            0x2D => MacKeyCode::HELP, // Insert has no direct macOS equivalent.
            0x2E => MacKeyCode::FORWARD_DELETE,
            0x5B => MacKeyCode::COMMAND,
            0x5C => MacKeyCode::RIGHT_COMMAND,
            0x70 => MacKeyCode::F1,
            0x71 => MacKeyCode::F2,
            0x72 => MacKeyCode::F3,
            0x73 => MacKeyCode::F4,
            0x74 => MacKeyCode::F5,
            0x75 => MacKeyCode::F6,
            0x76 => MacKeyCode::F7,
            0x77 => MacKeyCode::F8,
            0x78 => MacKeyCode::F9,
            0x79 => MacKeyCode::F10,
            0x7A => MacKeyCode::F11,
            0x7B => MacKeyCode::F12,
            0x90 => 0x47, // NumLock / keypad clear
            0xA0 => MacKeyCode::SHIFT,
            0xA1 => MacKeyCode::RIGHT_SHIFT,
            0xA2 => MacKeyCode::CONTROL,
            0xA3 => MacKeyCode::RIGHT_CONTROL,
            0xA4 => MacKeyCode::OPTION,
            0xA5 => MacKeyCode::RIGHT_OPTION,
            0x14 => MacKeyCode::CAPS_LOCK,
            // Canonical wire punctuation values to macOS hardware keycodes.
            0xBA => 0x29, // ;
            0xBB => 0x18, // =
            0xBC => 0x2B, // ,
            0xBD => 0x1B, // -
            0xBE => 0x2F, // .
            0xBF => 0x2C, // /
            0xC0 => 0x32, // `
            0xDB => 0x21, // [
            0xDC => 0x2A, // \
            0xDD => 0x1E, // ]
            0xDE => 0x27, // '
            0xE2 => 0x0A, // ISO_SECTION / VK_OEM_102
            0x60 => 0x52,
            0x61 => 0x53,
            0x62 => 0x54,
            0x63 => 0x55,
            0x64 => 0x56,
            0x65 => 0x57,
            0x66 => 0x58,
            0x67 => 0x59,
            0x68 => 0x5B,
            0x69 => 0x5C,
            0x6A => 0x43,
            0x6B => 0x45,
            0x6D => 0x4E,
            0x6E => 0x41,
            0x6F => 0x4B,
            0xE01C => 0x4C,
            _ => bail!("Unsupported macOS keycode: {}", keycode),
        };
        Ok(mapped)
    }

    fn display_to_screen_info(display: CGDisplay) -> ScreenInfo {
        screen_info_from_bounds(
            display.bounds(),
            display.pixels_wide(),
            display.pixels_high(),
        )
    }

    pub fn screen_info_from_bounds(
        bounds: CGRect,
        pixels_wide: u64,
        pixels_high: u64,
    ) -> ScreenInfo {
        let width = if pixels_wide > 0 {
            pixels_wide as u32
        } else {
            bounds.size.width.round().max(0.0) as u32
        };
        let height = if pixels_high > 0 {
            pixels_high as u32
        } else {
            bounds.size.height.round().max(0.0) as u32
        };

        ScreenInfo::new(
            bounds.origin.x.round() as i32,
            bounds.origin.y.round() as i32,
            width,
            height,
        )
    }

    unsafe fn collect_ns_string_array(array: id, files: &mut BTreeSet<String>) {
        if array == nil {
            return;
        }

        for idx in 0..array.count() {
            let value = array.objectAtIndex(idx);
            collect_file_path_string(value, files);
        }
    }

    unsafe fn collect_file_path_string(value: id, files: &mut BTreeSet<String>) {
        if let Some(path) = ns_string_to_string(value) {
            if !path.trim().is_empty() {
                files.insert(path);
            }
        }
    }

    unsafe fn collect_file_url_string(value: id, files: &mut BTreeSet<String>) {
        if let Some(url) = ns_string_to_string(value) {
            for path in parse_file_list_text(&url) {
                files.insert(path);
            }
        }
    }

    unsafe fn ns_string_to_string(value: id) -> Option<String> {
        if value == nil {
            return None;
        }

        let c_string = value.UTF8String();
        if c_string.is_null() {
            return None;
        }

        Some(CStr::from_ptr(c_string).to_string_lossy().into_owned())
    }

    pub fn parse_file_list_text(text: &str) -> Vec<String> {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(parse_file_url_or_path)
            .collect()
    }

    fn parse_file_url_or_path(value: &str) -> Option<String> {
        if value.starts_with("file://") {
            let mut path = value.trim_start_matches("file://");
            if let Some(stripped) = path.strip_prefix("localhost") {
                path = stripped;
            }
            return Some(percent_decode(path));
        }

        value.starts_with('/').then(|| value.to_string())
    }

    fn percent_decode(value: &str) -> String {
        let bytes = value.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len());
        let mut idx = 0;

        while idx < bytes.len() {
            if bytes[idx] == b'%' && idx + 2 < bytes.len() {
                if let (Some(hi), Some(lo)) = (hex_value(bytes[idx + 1]), hex_value(bytes[idx + 2]))
                {
                    decoded.push((hi << 4) | lo);
                    idx += 3;
                    continue;
                }
            }

            decoded.push(bytes[idx]);
            idx += 1;
        }

        String::from_utf8_lossy(&decoded).into_owned()
    }

    fn hex_value(value: u8) -> Option<u8> {
        match value {
            b'0'..=b'9' => Some(value - b'0'),
            b'a'..=b'f' => Some(value - b'a' + 10),
            b'A'..=b'F' => Some(value - b'A' + 10),
            _ => None,
        }
    }

    pub mod permissions {
        use anyhow::{bail, Result};

        pub fn can_listen_events() -> bool {
            unsafe { CGPreflightListenEventAccess() }
        }

        pub fn request_listen_events() -> bool {
            unsafe { CGRequestListenEventAccess() }
        }

        pub fn can_post_events() -> bool {
            unsafe { CGPreflightPostEventAccess() }
        }

        pub fn request_post_events() -> bool {
            unsafe { CGRequestPostEventAccess() }
        }

        pub fn ensure_can_listen_events() -> Result<()> {
            if can_listen_events() || request_listen_events() {
                return Ok(());
            }
            bail!("macOS Input Monitoring permission is required to capture input events")
        }

        pub fn ensure_can_post_events() -> Result<()> {
            if can_post_events() || request_post_events() {
                return Ok(());
            }
            bail!("macOS Accessibility permission is required to post input events")
        }

        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGPreflightListenEventAccess() -> bool;
            fn CGRequestListenEventAccess() -> bool;
            fn CGPreflightPostEventAccess() -> bool;
            fn CGRequestPostEventAccess() -> bool;
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use core_graphics::geometry::{CGPoint, CGRect, CGSize};

        #[test]
        fn maps_mouse_buttons_to_core_graphics() {
            assert!(mouse_button_to_cg(1, true).is_ok());
            assert!(mouse_button_to_cg(2, false).is_ok());
            assert!(mouse_button_to_cg(3, true).is_ok());
            assert!(mouse_button_to_cg(4, true).is_ok());
            assert!(mouse_button_to_cg(5, false).is_ok());
            assert!(mouse_button_to_cg(6, true).is_err());
        }

        #[test]
        fn macos_mouse_button_numbers_preserve_side_buttons_and_reject_unknowns() {
            assert_eq!(cg_mouse_button_number_to_code(0), Some(1));
            assert_eq!(cg_mouse_button_number_to_code(1), Some(3));
            assert_eq!(cg_mouse_button_number_to_code(2), Some(2));
            assert_eq!(cg_mouse_button_number_to_code(3), Some(4));
            assert_eq!(cg_mouse_button_number_to_code(4), Some(5));
            assert_eq!(cg_mouse_button_number_to_code(5), None);
            assert_eq!(cg_mouse_button_number_for_code(1), Some(0));
            assert_eq!(cg_mouse_button_number_for_code(4), Some(3));
            assert_eq!(cg_mouse_button_number_for_code(5), Some(4));
            assert_eq!(cg_mouse_button_number_for_code(6), None);
        }

        #[test]
        fn macos_mouse_motion_uses_drag_event_for_held_buttons() {
            assert!(matches!(
                macos_mouse_move_event_type(&[false; 6]),
                CGEventType::MouseMoved
            ));

            let mut buttons = [false; 6];
            buttons[1] = true;
            assert!(matches!(
                macos_mouse_move_event_type(&buttons),
                CGEventType::LeftMouseDragged
            ));

            buttons = [false; 6];
            buttons[3] = true;
            assert!(matches!(
                macos_mouse_move_event_type(&buttons),
                CGEventType::RightMouseDragged
            ));

            for button in [2, 4, 5] {
                buttons = [false; 6];
                buttons[button] = true;
                assert!(matches!(
                    macos_mouse_move_event_type(&buttons),
                    CGEventType::OtherMouseDragged
                ));
            }
        }

        #[test]
        fn maps_common_keycodes_to_macos_codes() {
            assert_eq!(mac_key_code(0x20).unwrap(), MacKeyCode::SPACE);
            assert_eq!(mac_key_code(0x1B).unwrap(), MacKeyCode::ESCAPE);
            assert_eq!(mac_key_code(0x0D).unwrap(), MacKeyCode::RETURN);
            assert_eq!(mac_key_code(0x70).unwrap(), MacKeyCode::F1);
        }

        #[test]
        fn maps_semantic_modifiers_navigation_and_keypad_to_hardware_codes() {
            assert_eq!(mac_key_code(0xA0).unwrap(), MacKeyCode::SHIFT);
            assert_eq!(mac_key_code(0xA1).unwrap(), MacKeyCode::RIGHT_SHIFT);
            assert_eq!(mac_key_code(0xA2).unwrap(), MacKeyCode::CONTROL);
            assert_eq!(mac_key_code(0xA3).unwrap(), MacKeyCode::RIGHT_CONTROL);
            assert_eq!(mac_key_code(0xA4).unwrap(), MacKeyCode::OPTION);
            assert_eq!(mac_key_code(0xA5).unwrap(), MacKeyCode::RIGHT_OPTION);
            assert_eq!(mac_key_code(0x5B).unwrap(), MacKeyCode::COMMAND);
            assert_eq!(mac_key_code(0x5C).unwrap(), MacKeyCode::RIGHT_COMMAND);
            assert_eq!(mac_key_code(0x14).unwrap(), MacKeyCode::CAPS_LOCK);
            assert_eq!(mac_key_code(0x25).unwrap(), MacKeyCode::LEFT_ARROW);
            assert_eq!(mac_key_code(0x24).unwrap(), MacKeyCode::HOME);
            assert!(mac_key_code(0x3F).is_err());
            assert_eq!(mac_key_code(0x60).unwrap(), 0x52);
            assert_eq!(mac_key_code(0xE01C).unwrap(), 0x4C);
        }

        #[test]
        fn maps_canonical_wire_punctuation_and_rejects_unknown_raw_codes() {
            assert_eq!(mac_key_code(0xBA).unwrap(), 0x29);
            assert_eq!(mac_key_code(0xBB).unwrap(), 0x18);
            assert_eq!(mac_key_code(0xBC).unwrap(), 0x2B);
            assert_eq!(mac_key_code(0xBD).unwrap(), 0x1B);
            assert_eq!(mac_key_code(0xBE).unwrap(), 0x2F);
            assert_eq!(mac_key_code(0xBF).unwrap(), 0x2C);
            assert_eq!(mac_key_code(0xC0).unwrap(), 0x32);
            assert_eq!(mac_key_code(0xDB).unwrap(), 0x21);
            assert_eq!(mac_key_code(0xDC).unwrap(), 0x2A);
            assert_eq!(mac_key_code(0xDD).unwrap(), 0x1E);
            assert_eq!(mac_key_code(0xDE).unwrap(), 0x27);
            assert_eq!(mac_key_code(0xE2).unwrap(), 0x0A);
            assert!(mac_key_code(0x04).is_err());
            assert!(mac_key_code(0x1234).is_err());
        }

        #[test]
        fn converts_display_bounds_to_screen_info() {
            let bounds = CGRect::new(&CGPoint::new(-1440.0, 0.0), &CGSize::new(1440.0, 900.0));
            let screen = screen_info_from_bounds(bounds, 2880, 1800);
            assert_eq!(screen.x, -1440);
            assert_eq!(screen.y, 0);
            assert_eq!(screen.width, 2880);
            assert_eq!(screen.height, 1800);
        }

        #[test]
        fn parses_file_urls_and_paths() {
            let files = parse_file_list_text(
                "# comment\nfile:///Users/me/Test%20File.txt\nfile://localhost/tmp/a.txt\n/Users/me/plain.txt",
            );
            assert_eq!(
                files,
                vec![
                    "/Users/me/Test File.txt",
                    "/tmp/a.txt",
                    "/Users/me/plain.txt"
                ]
            );
        }

        #[test]
        fn permission_state_is_derived_without_requesting_system_prompts() {
            let missing_capture = MacosInputPermissionState::from_preflight(false, true);
            let missing_inject = MacosInputPermissionState::from_preflight(true, false);

            assert!(!missing_capture.can_capture());
            assert!(missing_capture.can_inject());
            assert!(!missing_capture.is_ready());
            assert_eq!(
                missing_capture.actionable_error(),
                Some("macOS Input Monitoring permission is required to capture input events")
            );
            assert!(!missing_inject.can_inject());
            assert_eq!(
                missing_inject.actionable_error(),
                Some("macOS Accessibility permission is required to post input events")
            );
        }

        #[test]
        fn permission_kinds_target_the_matching_system_settings_panes() {
            assert!(MacosInputPermissionKind::Accessibility
                .settings_url()
                .ends_with("Privacy_Accessibility"));
            assert!(MacosInputPermissionKind::InputMonitoring
                .settings_url()
                .ends_with("Privacy_ListenEvent"));
        }

        #[test]
        fn posting_permission_gate_rejects_revocation_without_requesting_a_prompt() {
            assert!(require_macos_accessibility_permission(true).is_ok());
            assert_eq!(
                require_macos_accessibility_permission(false)
                    .unwrap_err()
                    .to_string(),
                "macOS Accessibility permission is required to post input events"
            );
        }

        #[test]
        fn event_tap_keeps_or_drops_events_without_recapturing_marked_injection() {
            assert_eq!(
                macos_tap_event_disposition(false, false),
                MacosTapEventDisposition::CaptureAndKeep
            );
            assert_eq!(
                macos_tap_event_disposition(false, true),
                MacosTapEventDisposition::CaptureAndDrop
            );
            assert_eq!(
                macos_tap_event_disposition(true, true),
                MacosTapEventDisposition::IgnoreAndKeep
            );
            assert!(is_rshare_injected_event_marker(RSHARE_EVENT_SOURCE_MARKER));
            assert!(!is_rshare_injected_event_marker(0));
        }

        #[test]
        fn event_tap_uses_the_logged_in_users_session_location() {
            assert!(matches!(
                macos_input_event_tap_location(),
                CGEventTapLocation::Session
            ));
        }

        #[test]
        fn event_tap_uses_a_concrete_run_loop_mode() {
            assert_eq!(
                macos_input_event_tap_run_loop_mode().to_string(),
                MACOS_INPUT_EVENT_TAP_RUN_LOOP_MODE_NAME
            );
        }

        #[test]
        fn event_tap_disabled_notifications_request_reenable() {
            assert!(is_tap_disabled_notification(
                CGEventType::TapDisabledByTimeout
            ));
            assert!(is_tap_disabled_notification(
                CGEventType::TapDisabledByUserInput
            ));
            assert!(!is_tap_disabled_notification(CGEventType::KeyDown));
        }

        #[test]
        fn stopped_and_finished_run_loop_results_are_abnormal() {
            assert!(is_abnormal_run_loop_result(
                core_foundation::runloop::CFRunLoopRunResult::Stopped
            ));
            assert!(is_abnormal_run_loop_result(
                core_foundation::runloop::CFRunLoopRunResult::Finished
            ));
            assert!(!is_abnormal_run_loop_result(
                core_foundation::runloop::CFRunLoopRunResult::TimedOut
            ));
        }

        #[test]
        fn event_tap_discontinuity_clears_suppression_and_reports_once() {
            let running = AtomicBool::new(true);
            let reenable_requested = AtomicBool::new(false);
            let fault_reported = AtomicBool::new(false);
            let fault_count = std::sync::atomic::AtomicUsize::new(0);
            let fault = || {
                fault_count.fetch_add(1, Ordering::SeqCst);
            };

            set_macos_local_input_suppressed(true);
            assert!(handle_tap_disabled_notification(
                CGEventType::TapDisabledByTimeout,
                &running,
                &reenable_requested,
                &fault_reported,
                &fault,
            ));
            assert!(running.load(Ordering::SeqCst));
            assert!(reenable_requested.load(Ordering::SeqCst));
            assert!(!LOCAL_INPUT_SUPPRESSED.load(Ordering::SeqCst));
            assert_eq!(fault_count.load(Ordering::SeqCst), 1);

            assert!(handle_tap_disabled_notification(
                CGEventType::TapDisabledByUserInput,
                &running,
                &reenable_requested,
                &fault_reported,
                &fault,
            ));
            assert_eq!(fault_count.load(Ordering::SeqCst), 1);
            assert!(!handle_tap_disabled_notification(
                CGEventType::KeyDown,
                &running,
                &reenable_requested,
                &fault_reported,
                &fault,
            ));
            set_macos_local_input_suppressed(false);
        }

        #[test]
        fn panicking_tap_fault_bridge_stops_listener_without_requesting_reenable() {
            let running = AtomicBool::new(true);
            let reenable_requested = AtomicBool::new(false);
            let reported = AtomicBool::new(false);
            set_macos_local_input_suppressed(true);

            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                handle_tap_disabled_notification(
                    CGEventType::TapDisabledByTimeout,
                    &running,
                    &reenable_requested,
                    &reported,
                    &|| panic!("fault bridge panic"),
                )
            }));

            assert_eq!(result.unwrap(), true);
            assert!(reported.load(Ordering::Acquire));
            assert!(!LOCAL_INPUT_SUPPRESSED.load(Ordering::Acquire));
            assert!(!running.load(Ordering::Acquire));
            assert!(!reenable_requested.load(Ordering::Acquire));
        }

        #[test]
        fn poisoned_modifier_state_is_recovered_without_panicking() {
            let modifier_state = Mutex::new(ModifierState::default());
            let _ = panic::catch_unwind(AssertUnwindSafe(|| {
                let _guard = modifier_state.lock().unwrap();
                panic!("callback conversion panic");
            }));

            assert!(modifier_state.is_poisoned());
            let mut recovered = recover_modifier_state(&modifier_state);
            assert!(modifier_flags_changed_down(
                0x38,
                0x0002_0000,
                &mut recovered
            ));
        }

        #[test]
        fn hardware_key_posting_preserves_native_codes_for_press_and_release() {
            for keycode in [
                0x00, // A
                0x0D, // W
                0x08, // C
                0x38, 0x3C, 0x3B, 0x3E, 0x3A, 0x3D, 0x37, 0x36, 0x39, 0x7B, // left arrow
                0x7A, 0x6F, // F1/F12
                0x52, // keypad 0
                0x18, 0x29, 0x0A, // punctuation and ISO 102nd
            ] {
                assert_eq!(
                    hardware_key_post_record(keycode, true),
                    HardwareKeyPostRecord {
                        keycode,
                        down: true
                    }
                );
                assert_eq!(
                    hardware_key_post_record(keycode, false),
                    HardwareKeyPostRecord {
                        keycode,
                        down: false
                    }
                );
            }
        }

        #[test]
        fn flags_changed_releases_each_side_of_a_shared_modifier() {
            const SHIFT: u64 = 0x0002_0000;
            let mut state = ModifierState::default();

            assert!(modifier_flags_changed_down(0x38, SHIFT, &mut state));
            assert!(modifier_flags_changed_down(0x3C, SHIFT, &mut state));
            assert!(!modifier_flags_changed_down(0x38, SHIFT, &mut state));
            assert!(!modifier_flags_changed_down(0x3C, 0, &mut state));
        }

        #[test]
        fn flags_changed_tracks_fn_press_and_release() {
            const SECONDARY_FN: u64 = 0x0080_0000;
            let mut state = ModifierState::default();

            assert!(modifier_flags_changed_down(0x3F, SECONDARY_FN, &mut state));
            assert!(!modifier_flags_changed_down(0x3F, 0, &mut state));
        }

        #[test]
        fn timed_out_worker_cleanup_replaces_running_token_without_joining() {
            let mut listener = MacosInputListener::new();
            let old_token = listener.running.clone();
            old_token.store(true, Ordering::SeqCst);
            let worker = thread::spawn(|| {});

            listener.abandon_timed_out_worker(worker);

            assert!(!old_token.load(Ordering::SeqCst));
            assert!(!listener.is_running());
            assert!(!Arc::ptr_eq(&old_token, &listener.running));
        }

        #[test]
        fn worker_exit_guard_clears_running_token() {
            let running = Arc::new(AtomicBool::new(true));
            let capture_started = Arc::new(AtomicBool::new(true));
            let capture_fault_reported = Arc::new(AtomicBool::new(false));
            let fault_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let fault_count_for_callback = fault_count.clone();
            set_macos_local_input_suppressed(true);
            {
                let _guard = RunningFlagReset::new(
                    running.clone(),
                    capture_started,
                    capture_fault_reported,
                    Arc::new(move || {
                        fault_count_for_callback.fetch_add(1, Ordering::SeqCst);
                    }),
                );
            }
            assert!(!running.load(Ordering::SeqCst));
            assert!(!LOCAL_INPUT_SUPPRESSED.load(Ordering::SeqCst));
            assert_eq!(fault_count.load(Ordering::SeqCst), 1);
        }

        #[test]
        fn normal_stop_and_startup_error_do_not_report_worker_faults() {
            for capture_started in [false, true] {
                let running = Arc::new(AtomicBool::new(false));
                let capture_fault_reported = Arc::new(AtomicBool::new(false));
                let fault_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let fault_count_for_callback = fault_count.clone();
                let _guard = RunningFlagReset::new(
                    running,
                    Arc::new(AtomicBool::new(capture_started)),
                    capture_fault_reported,
                    Arc::new(move || {
                        fault_count_for_callback.fetch_add(1, Ordering::SeqCst);
                    }),
                );
                drop(_guard);
                assert_eq!(fault_count.load(Ordering::SeqCst), 0);
            }
        }

        #[test]
        fn listener_status_handle_tracks_running_and_faulted_transitions() {
            let listener = MacosInputListener::new();
            let status = listener.status_handle();

            assert!(!status.is_running());
            assert!(!status.has_capture_fault());

            listener.running.store(true, Ordering::Release);
            assert!(status.is_running());

            listener
                .capture_fault_reported
                .store(true, Ordering::Release);
            assert!(status.has_capture_fault());
        }

        #[test]
        fn abnormal_run_loop_result_reports_started_capture_once() {
            let running = AtomicBool::new(true);
            let capture_started = AtomicBool::new(true);
            let capture_fault_reported = AtomicBool::new(false);
            let fault_count = std::sync::atomic::AtomicUsize::new(0);
            let fault = || {
                fault_count.fetch_add(1, Ordering::SeqCst);
            };
            set_macos_local_input_suppressed(true);

            assert!(handle_abnormal_run_loop_result(
                core_foundation::runloop::CFRunLoopRunResult::Stopped,
                &running,
                &capture_started,
                &capture_fault_reported,
                &fault,
            ));
            assert!(!running.load(Ordering::SeqCst));
            assert!(!LOCAL_INPUT_SUPPRESSED.load(Ordering::SeqCst));
            assert_eq!(fault_count.load(Ordering::SeqCst), 1);

            assert!(handle_abnormal_run_loop_result(
                core_foundation::runloop::CFRunLoopRunResult::Finished,
                &running,
                &capture_started,
                &capture_fault_reported,
                &fault,
            ));
            assert_eq!(fault_count.load(Ordering::SeqCst), 1);
        }
    }
}
