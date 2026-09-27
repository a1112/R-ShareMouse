//! Full-contact snapshots for a session-owned Windows synthetic touch device.
//! Prepared state is committed only after successful OS injection.
use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Contact {
    pub id: u32,
    pub x: f64,
    pub y: f64,
    pub pressure: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Down,
    Update,
    Up,
}

#[derive(Debug, Clone)]
pub struct Point {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub pressure: u32,
    pub phase: Phase,
}

pub struct PreparedFrame {
    pub points: Vec<Point>,
    sequence: u64,
    active: BTreeMap<u32, Point>,
    bounds: Bounds,
}

#[derive(Debug, Default)]
pub struct TouchState {
    sequence: u64,
    active: BTreeMap<u32, Point>,
    bounds: Option<Bounds>,
}

impl TouchState {
    pub fn prepare(
        &self,
        sequence: u64,
        contacts: &[Contact],
        bounds: Bounds,
    ) -> Result<PreparedFrame> {
        ensure!(sequence > self.sequence, "stale touch sequence");
        ensure!(contacts.len() <= 10, "maximum 10 touch contacts");
        ensure!(
            bounds.width > 0 && bounds.height > 0,
            "empty display bounds"
        );
        ensure!(
            i64::from(bounds.x) + i64::from(bounds.width) - 1 <= i64::from(i32::MAX)
                && i64::from(bounds.y) + i64::from(bounds.height) - 1 <= i64::from(i32::MAX),
            "display bounds overflow"
        );
        ensure!(
            self.active.is_empty() || self.bounds == Some(bounds),
            "display geometry changed; cancel touch first"
        );
        let mut active = BTreeMap::new();
        for c in contacts {
            ensure!(
                (1..=10).contains(&c.id) && c.pressure <= 1024,
                "invalid touch id or pressure"
            );
            ensure!(
                c.x.is_finite()
                    && c.y.is_finite()
                    && (0.0..=1.0).contains(&c.x)
                    && (0.0..=1.0).contains(&c.y),
                "invalid touch coordinates"
            );
            let point = Point {
                id: c.id,
                x: (i64::from(bounds.x) + (c.x * f64::from(bounds.width - 1)).round() as i64)
                    as i32,
                y: (i64::from(bounds.y) + (c.y * f64::from(bounds.height - 1)).round() as i64)
                    as i32,
                pressure: c.pressure,
                phase: if self.active.contains_key(&c.id) {
                    Phase::Update
                } else {
                    Phase::Down
                },
            };
            ensure!(active.insert(c.id, point).is_none(), "duplicate touch id");
        }
        let mut points: Vec<_> = active.values().cloned().collect();
        for (id, previous) in &self.active {
            if !active.contains_key(id) {
                // UP must use the last injected coordinates, not pointerup coordinates.
                points.push(Point {
                    phase: Phase::Up,
                    ..previous.clone()
                });
            }
        }
        Ok(PreparedFrame {
            points,
            sequence,
            active,
            bounds,
        })
    }

    pub fn commit(&mut self, frame: PreparedFrame) {
        self.sequence = frame.sequence;
        self.active = frame.active;
        self.bounds = Some(frame.bounds);
    }
}

/// Construct and use on one dedicated injection thread. Drop destroys the device
/// and cancels contacts even if the application loses connectivity.
pub struct TouchInjector {
    state: TouchState,
    #[cfg(windows)]
    device: windows::Win32::UI::Controls::HSYNTHETICPOINTERDEVICE,
}

impl TouchInjector {
    pub fn new() -> Result<Self> {
        #[cfg(windows)]
        {
            use windows::Win32::UI::{Controls::*, WindowsAndMessaging::PT_TOUCH};
            let device =
                unsafe { CreateSyntheticPointerDevice(PT_TOUCH, 10, POINTER_FEEDBACK_NONE)? };
            Ok(Self {
                state: TouchState::default(),
                device,
            })
        }
        #[cfg(not(windows))]
        bail!("Windows native touch injection is unavailable on this platform")
    }

    pub fn inject(&mut self, sequence: u64, contacts: &[Contact], bounds: Bounds) -> Result<()> {
        let frame = self.state.prepare(sequence, contacts, bounds)?;
        #[cfg(windows)]
        if !frame.points.is_empty() {
            self.inject_points(&frame.points, false)?;
        }
        self.state.commit(frame);
        Ok(())
    }

    pub fn cancel(&mut self) -> Result<()> {
        #[cfg(windows)]
        if !self.state.active.is_empty() {
            let points: Vec<_> = self
                .state
                .active
                .values()
                .map(|p| Point {
                    phase: Phase::Up,
                    ..p.clone()
                })
                .collect();
            self.inject_points(&points, true)?;
        }
        self.state.active.clear();
        self.state.bounds = None;
        Ok(())
    }

    #[cfg(windows)]
    fn inject_points(&self, points: &[Point], cancel: bool) -> Result<()> {
        use windows::Win32::{
            Foundation::{POINT, RECT},
            UI::{
                Controls::*,
                Input::Pointer::*,
                WindowsAndMessaging::{PT_TOUCH, TOUCH_MASK_CONTACTAREA, TOUCH_MASK_PRESSURE},
            },
        };
        let pointers: Vec<_> = points
            .iter()
            .map(|p| {
                let mut touch = POINTER_TOUCH_INFO::default();
                touch.pointerInfo.pointerType = PT_TOUCH;
                touch.pointerInfo.pointerId = p.id;
                touch.pointerInfo.ptPixelLocation = POINT { x: p.x, y: p.y };
                touch.pointerInfo.pointerFlags = match p.phase {
                    Phase::Down => {
                        POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT
                    }
                    Phase::Update => {
                        POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT
                    }
                    Phase::Up => POINTER_FLAG_UP,
                };
                if cancel {
                    touch.pointerInfo.pointerFlags |= POINTER_FLAG_CANCELED;
                }
                touch.touchMask = TOUCH_MASK_CONTACTAREA | TOUCH_MASK_PRESSURE;
                touch.rcContact = RECT {
                    left: p.x.saturating_sub(2),
                    top: p.y.saturating_sub(2),
                    right: p.x.saturating_add(2),
                    bottom: p.y.saturating_add(2),
                };
                touch.pressure = p.pressure;
                POINTER_TYPE_INFO {
                    r#type: PT_TOUCH,
                    Anonymous: POINTER_TYPE_INFO_0 { touchInfo: touch },
                }
            })
            .collect();
        for attempt in 0..3 {
            match unsafe { InjectSyntheticPointerInput(self.device, &pointers) } {
                Ok(()) => return Ok(()),
                Err(error)
                    if error.code() == windows::core::HRESULT::from_win32(21) && attempt < 2 =>
                {
                    // Windows ERROR_NOT_READY: same frame must be retried >= 0.1ms later.
                    std::thread::sleep(std::time::Duration::from_micros(200));
                }
                Err(error) => return Err(error.into()),
            }
        }
        bail!("touch injection retry exhausted")
    }
}

impl Drop for TouchInjector {
    fn drop(&mut self) {
        let _ = self.cancel();
        #[cfg(windows)]
        unsafe {
            windows::Win32::UI::Controls::DestroySyntheticPointerDevice(self.device);
        }
    }
}
