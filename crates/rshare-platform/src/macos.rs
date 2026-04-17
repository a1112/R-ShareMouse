//! macOS platform-specific implementations.

cfg_if::cfg_if! {
    if #[cfg(target_os = "macos")] {
        pub use macos_impl::*;
    }
}

#[cfg(target_os = "macos")]
mod macos_impl {
    use anyhow::{anyhow, bail, Result};
    use cocoa::appkit::{NSFilenamesPboardType, NSPasteboard, NSPasteboardItem, NSURLPboardType};
    use cocoa::base::{id, nil};
    use cocoa::foundation::{NSArray, NSString};
    use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    /// Input event captured by the native macOS listener.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum MacosInputEvent {
        MouseMove { x: i32, y: i32 },
        MouseButton { button: u8, down: bool },
        MouseWheel { delta_x: i32, delta_y: i32 },
        Key { keycode: u32, down: bool },
    }

    /// macOS input listener using a CoreGraphics event tap.
    pub struct MacosInputListener {
        running: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    impl MacosInputListener {
        pub fn new() -> Self {
            Self {
                running: Arc::new(AtomicBool::new(false)),
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
            if self.is_running() {
                return Ok(());
            }

            permissions::ensure_can_listen_events()?;

            self.running.store(true, Ordering::SeqCst);
            let running = self.running.clone();
            let callback = Arc::new(callback);

            self.worker = Some(thread::spawn(move || {
                let current_loop = CFRunLoop::get_current();
                let callback = callback.clone();

                let tap = match CGEventTap::new(
                    CGEventTapLocation::HID,
                    CGEventTapPlacement::HeadInsertEventTap,
                    CGEventTapOptions::ListenOnly,
                    vec![
                        CGEventType::MouseMoved,
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
                        if let Some(input_event) = convert_cg_event(event_type, event) {
                            callback(input_event);
                        }
                        None
                    },
                ) {
                    Ok(tap) => tap,
                    Err(_) => {
                        tracing::error!("Failed to create macOS CGEventTap");
                        running.store(false, Ordering::SeqCst);
                        return;
                    }
                };

                let source = match tap.mach_port.create_runloop_source(0) {
                    Ok(source) => source,
                    Err(_) => {
                        tracing::error!("Failed to create macOS event tap run-loop source");
                        running.store(false, Ordering::SeqCst);
                        return;
                    }
                };

                let run_loop_mode = unsafe { kCFRunLoopCommonModes };
                current_loop.add_source(&source, run_loop_mode);
                tap.enable();
                tracing::info!("macOS input listener started");

                while running.load(Ordering::SeqCst) {
                    let result =
                        CFRunLoop::run_in_mode(run_loop_mode, Duration::from_millis(100), false);
                    if matches!(
                        result,
                        core_foundation::runloop::CFRunLoopRunResult::Finished
                    ) {
                        break;
                    }
                }

                current_loop.remove_source(&source, run_loop_mode);
                tracing::info!("macOS input listener stopped");
            }));

            Ok(())
        }

        pub fn stop(&mut self) -> Result<()> {
            self.running.store(false, Ordering::SeqCst);
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
    }

    impl MacosInputEmulator {
        pub fn new() -> Self {
            Self { active: false }
        }

        pub fn activate(&mut self) -> Result<()> {
            permissions::ensure_can_post_events()?;
            self.active = true;
            tracing::info!("macOS input emulator activated");
            Ok(())
        }

        pub fn deactivate(&mut self) -> Result<()> {
            self.active = false;
            tracing::info!("macOS input emulator deactivated");
            Ok(())
        }

        pub fn send_mouse_move(&mut self, x: i32, y: i32) -> Result<()> {
            if !self.active {
                return Ok(());
            }

            let event = CGEvent::new_mouse_event(
                new_event_source()?,
                CGEventType::MouseMoved,
                CGPoint::new(x as f64, y as f64),
                CGMouseButton::Left,
            )
            .map_err(|_| anyhow!("Failed to create macOS mouse move event"))?;
            event.post(CGEventTapLocation::HID);
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
            event.post(CGEventTapLocation::HID);
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
            event.post(CGEventTapLocation::HID);
            Ok(())
        }

        pub fn send_key(&mut self, keycode: u32, down: bool) -> Result<()> {
            if !self.active {
                return Ok(());
            }

            let keycode = mac_key_code(keycode)?;
            let event = CGEvent::new_keyboard_event(new_event_source()?, keycode, down)
                .map_err(|_| anyhow!("Failed to create macOS keyboard event"))?;
            event.post(CGEventTapLocation::HID);
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

    fn convert_cg_event(event_type: CGEventType, event: &CGEvent) -> Option<MacosInputEvent> {
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
                ),
                down: true,
            }),
            CGEventType::OtherMouseUp => Some(MacosInputEvent::MouseButton {
                button: cg_mouse_button_number_to_code(
                    event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER),
                ),
                down: false,
            }),
            CGEventType::ScrollWheel => Some(MacosInputEvent::MouseWheel {
                delta_x: event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2)
                    as i32,
                delta_y: event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1)
                    as i32,
            }),
            CGEventType::KeyDown | CGEventType::FlagsChanged => Some(MacosInputEvent::Key {
                keycode: event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u32,
                down: true,
            }),
            CGEventType::KeyUp => Some(MacosInputEvent::Key {
                keycode: event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u32,
                down: false,
            }),
            _ => None,
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
            _ => bail!("Unsupported macOS mouse button: {}", button),
        }
    }

    fn cg_mouse_button_number_to_code(button_number: i64) -> u8 {
        match button_number {
            0 => 1,
            1 => 3,
            2 => 2,
            n if n > 0 && n <= u8::MAX as i64 => n as u8,
            _ => 0,
        }
    }

    pub fn mac_key_code(keycode: u32) -> Result<u16> {
        let mapped = match keycode {
            0x08 => MacKeyCode::DELETE,
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
            0x2E => MacKeyCode::FORWARD_DELETE,
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
            raw if raw <= u16::MAX as u32 => raw as u16,
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
            assert!(mouse_button_to_cg(4, true).is_err());
        }

        #[test]
        fn maps_common_keycodes_to_macos_codes() {
            assert_eq!(mac_key_code(0x20).unwrap(), MacKeyCode::SPACE);
            assert_eq!(mac_key_code(0x1B).unwrap(), MacKeyCode::ESCAPE);
            assert_eq!(mac_key_code(0x0D).unwrap(), MacKeyCode::RETURN);
            assert_eq!(mac_key_code(0x70).unwrap(), MacKeyCode::F1);
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
    }
}
