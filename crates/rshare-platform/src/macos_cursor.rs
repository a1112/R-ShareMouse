/// Owns exactly one CoreGraphics hide count, including cleanup retries.
#[derive(Default)]
pub(crate) struct LocalCursorLease {
    hidden: bool,
}

impl LocalCursorLease {
    pub(crate) const fn new() -> Self {
        Self { hidden: false }
    }

    pub(crate) fn update<E>(
        &mut self,
        hidden: bool,
        prepare_hide: impl FnOnce() -> Result<(), E>,
        apply: impl FnOnce(bool) -> Result<(), E>,
    ) -> Result<(), E> {
        if self.hidden != hidden {
            if hidden {
                prepare_hide()?;
            }
            apply(hidden)?;
            self.hidden = hidden;
        }
        Ok(())
    }
}

#[cfg(not(test))]
pub(crate) fn prepare_background_cursor_access() -> Result<(), String> {
    native::prepare_background_cursor_access()
}

#[cfg(test)]
pub(crate) fn prepare_background_cursor_access() -> Result<(), String> {
    // Unit tests never change the developer's WindowServer connection.
    Ok(())
}

#[cfg(not(test))]
mod native {
    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::string::{CFString, CFStringRef};
    use std::ffi::{c_char, c_void};
    use std::sync::OnceLock;

    type MainConnection = unsafe extern "C" fn() -> i32;
    type SetConnectionProperty = unsafe extern "C" fn(i32, i32, CFStringRef, CFTypeRef) -> i32;

    #[derive(Clone, Copy)]
    struct BackgroundCursorApi {
        main_connection: MainConnection,
        set_connection_property: SetConnectionProperty,
    }

    extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    fn resolve_api() -> Option<BackgroundCursorApi> {
        // Darwin's RTLD_DEFAULT searches already loaded system frameworks.
        // Never hard-link these private symbols: a future OS can omit them.
        let handle = -2isize as *mut c_void;
        unsafe {
            let main_connection = dlsym(handle, c"CGSMainConnectionID".as_ptr());
            let set_connection_property = dlsym(handle, c"CGSSetConnectionProperty".as_ptr());
            if main_connection.is_null() || set_connection_property.is_null() {
                return None;
            }
            Some(BackgroundCursorApi {
                main_connection: std::mem::transmute::<*mut c_void, MainConnection>(
                    main_connection,
                ),
                set_connection_property: std::mem::transmute::<*mut c_void, SetConnectionProperty>(
                    set_connection_property,
                ),
            })
        }
    }

    pub(super) fn prepare_background_cursor_access() -> Result<(), String> {
        static API: OnceLock<Option<BackgroundCursorApi>> = OnceLock::new();
        let api = API.get_or_init(resolve_api).as_ref().ok_or_else(|| {
            "WindowServer background cursor API is unavailable on this macOS version".to_owned()
        })?;
        // Public CGDisplayHideCursor normally affects only the foreground app.
        // Enable background cursor changes on this daemon's own connection;
        // this changes neither application focus nor another process's state.
        // The private bridge is also used by Input Leap's macOS cursor backend:
        // https://github.com/input-leap/input-leap/blob/master/src/lib/platform/OSXScreen.mm
        let property = CFString::new("SetsCursorInBackground");
        let enabled = CFBoolean::true_value();
        let result = unsafe {
            let connection = (api.main_connection)();
            (api.set_connection_property)(
                connection,
                connection,
                property.as_concrete_TypeRef(),
                enabled.as_CFTypeRef(),
            )
        };
        if result != 0 {
            return Err(format!(
                "WindowServer background cursor access failed: {result}"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_remote_updates_balance_one_hide_with_one_restore() {
        let mut lease = LocalCursorLease::default();
        let mut calls = Vec::new();
        for hidden in [false, true, true, true, false, false] {
            lease
                .update(
                    hidden,
                    || Ok(()),
                    |hidden| {
                        calls.push(hidden);
                        Ok::<_, ()>(())
                    },
                )
                .unwrap();
        }
        assert_eq!(calls, [true, false]);
    }

    #[test]
    fn failed_hide_does_not_decrement_an_unowned_hide_count() {
        let mut lease = LocalCursorLease::default();
        assert_eq!(
            lease.update(true, || Ok(()), |_| Err("denied")),
            Err("denied")
        );
        lease
            .update::<&str>(
                false,
                || panic!("must not prepare a restore"),
                |_| panic!("must not show a cursor we did not hide"),
            )
            .unwrap();
        lease
            .update(true, || Ok(()), |_| Ok::<_, &str>(()))
            .unwrap();
        assert!(lease.hidden);
    }

    #[test]
    fn failed_restore_keeps_the_lease_for_a_later_cleanup_retry() {
        let mut lease = LocalCursorLease::default();
        lease
            .update(true, || Ok(()), |_| Ok::<_, &str>(()))
            .unwrap();
        assert_eq!(
            lease.update(
                false,
                || panic!("restore must not depend on background setup"),
                |_| Err("temporarily unavailable")
            ),
            Err("temporarily unavailable")
        );
        assert!(lease.hidden);
        lease
            .update(
                false,
                || panic!("restore must not depend on background setup"),
                |_| Ok::<_, &str>(()),
            )
            .unwrap();
        assert!(!lease.hidden);
    }

    #[test]
    fn background_access_failure_does_not_acquire_a_hide_count_and_can_retry() {
        let mut lease = LocalCursorLease::default();
        assert_eq!(
            lease.update(
                true,
                || Err("background access denied"),
                |_| panic!("must not hide before background access is ready")
            ),
            Err("background access denied")
        );
        assert!(!lease.hidden);
        lease
            .update(true, || Ok(()), |_| Ok::<_, &str>(()))
            .unwrap();
        assert!(lease.hidden);
    }

    #[test]
    fn background_access_is_prepared_before_hiding_and_not_repeated() {
        use std::cell::RefCell;
        let calls = RefCell::new(Vec::new());
        let mut lease = LocalCursorLease::default();
        for hidden in [true, true, false] {
            lease
                .update(
                    hidden,
                    || {
                        calls.borrow_mut().push("prepare");
                        Ok::<_, ()>(())
                    },
                    |hidden| {
                        calls
                            .borrow_mut()
                            .push(if hidden { "hide" } else { "show" });
                        Ok(())
                    },
                )
                .unwrap();
        }
        assert_eq!(*calls.borrow(), ["prepare", "hide", "show"]);
    }
}
