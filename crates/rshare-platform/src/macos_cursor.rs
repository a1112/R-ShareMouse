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
        apply: impl FnOnce(bool) -> Result<(), E>,
    ) -> Result<(), E> {
        if self.hidden != hidden {
            apply(hidden)?;
            self.hidden = hidden;
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
                .update(hidden, |hidden| {
                    calls.push(hidden);
                    Ok::<_, ()>(())
                })
                .unwrap();
        }
        assert_eq!(calls, [true, false]);
    }

    #[test]
    fn failed_hide_does_not_decrement_an_unowned_hide_count() {
        let mut lease = LocalCursorLease::default();
        assert_eq!(lease.update(true, |_| Err("denied")), Err("denied"));
        lease
            .update::<&str>(false, |_| panic!("must not show a cursor we did not hide"))
            .unwrap();
        lease.update(true, |_| Ok::<_, &str>(())).unwrap();
        assert!(lease.hidden);
    }

    #[test]
    fn failed_restore_keeps_the_lease_for_a_later_cleanup_retry() {
        let mut lease = LocalCursorLease::default();
        lease.update(true, |_| Ok::<_, &str>(())).unwrap();
        assert_eq!(
            lease.update(false, |_| Err("temporarily unavailable")),
            Err("temporarily unavailable")
        );
        assert!(lease.hidden);
        lease.update(false, |_| Ok::<_, &str>(())).unwrap();
        assert!(!lease.hidden);
    }
}
