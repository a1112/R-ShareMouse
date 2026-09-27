use rshare_platform::remote_touch::{Bounds, Contact, Phase, TouchState};

fn contact(id: u32, x: f64, y: f64) -> Contact {
    Contact {
        id,
        x,
        y,
        pressure: 512,
    }
}
fn bounds() -> Bounds {
    Bounds {
        x: -1920,
        y: 120,
        width: 1920,
        height: 1080,
    }
}

#[cfg(windows)]
#[test]
fn extended_display_lease_blocks_concurrent_display_mutation() {
    let _lease = rshare_platform::virtual_display::reserve_virtual_display().unwrap();
    assert!(std::thread::spawn(
        || rshare_platform::virtual_display::reserve_virtual_display().is_err()
    )
    .join()
    .unwrap());
}

#[test]
fn remote_touch_maps_full_monitor_including_negative_origin() {
    let mut state = TouchState::default();
    let frame = state
        .prepare(1, &[contact(1, 0.0, 1.0), contact(2, 1.0, 0.0)], bounds())
        .unwrap();
    assert_eq!((frame.points[0].x, frame.points[0].y), (-1920, 1199));
    assert_eq!((frame.points[1].x, frame.points[1].y), (-1, 120));
    assert!(frame.points.iter().all(|p| p.phase == Phase::Down));
    state.commit(frame);
    let update = state.prepare(2, &[contact(2, 0.4, 0.5)], bounds()).unwrap();
    let up = update.points.iter().find(|p| p.id == 1).unwrap();
    assert_eq!(up.phase, Phase::Up);
    assert_eq!((up.x, up.y), (-1920, 1199));
    assert_eq!(
        update.points.iter().find(|p| p.id == 2).unwrap().phase,
        Phase::Update
    );
}

#[test]
fn remote_touch_rejects_invalid_or_replayed_frames_without_advancing_state() {
    let mut state = TouchState::default();
    assert!(state.prepare(0, &[], bounds()).is_err());
    assert!(state
        .prepare(1, &[contact(1, f64::NAN, 0.0)], bounds())
        .is_err());
    assert!(state.prepare(1, &[contact(1, 1.1, 0.0)], bounds()).is_err());
    assert!(state
        .prepare(1, &[contact(1, 0.0, 0.0), contact(1, 0.2, 0.2)], bounds())
        .is_err());
    assert!(state
        .prepare(1, &[contact(11, 0.0, 0.0)], bounds())
        .is_err());
    let down = state.prepare(1, &[contact(1, 0.2, 0.2)], bounds()).unwrap();
    // Failed OS injection does not commit; retry remains a DOWN.
    assert_eq!(
        state
            .prepare(1, &[contact(1, 0.2, 0.2)], bounds())
            .unwrap()
            .points[0]
            .phase,
        Phase::Down
    );
    state.commit(down);
    assert!(state.prepare(1, &[], bounds()).is_err());
    assert!(state
        .prepare(
            2,
            &[contact(1, 0.2, 0.2)],
            Bounds {
                width: 0,
                ..bounds()
            }
        )
        .is_err());
}

#[test]
fn remote_touch_releases_all_on_empty_snapshot_and_rejects_geometry_changes() {
    let mut state = TouchState::default();
    state.commit(
        state
            .prepare(1, &[contact(1, 0.2, 0.2), contact(2, 0.3, 0.3)], bounds())
            .unwrap(),
    );
    assert!(state
        .prepare(2, &[contact(1, 0.2, 0.2)], Bounds { x: 0, ..bounds() })
        .is_err());
    let release = state.prepare(3, &[], bounds()).unwrap();
    assert_eq!(release.points.len(), 2);
    assert!(release.points.iter().all(|p| p.phase == Phase::Up));
    state.commit(release);
    assert!(state.prepare(4, &[], bounds()).unwrap().points.is_empty());
}
