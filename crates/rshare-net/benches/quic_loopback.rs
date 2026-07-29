use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rshare_core::{
    ButtonState, ClockDomainId, KeyState, MonotonicStamp, MouseButton, RealtimeInputFrame,
    RealtimeInputPayload, ReliableInputEvent, ReliableInputFrame, SessionEpoch,
    INPUT_PROTOCOL_VERSION,
};
use rshare_net::{RealtimeInputCodec, ReliableInputCodec};

fn codec_benches(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("codec");
    let mut mouse = RealtimeInputFrame {
        protocol_version: INPUT_PROTOCOL_VERSION,
        session_epoch: SessionEpoch(1),
        sequence: 0,
        captured_at: MonotonicStamp::new(ClockDomainId(1), 1_000),
        payload: RealtimeInputPayload::RelativeMouse { dx: 101, dy: -303 },
    };
    group.bench_function("realtime_mouse_encode_decode", |bencher| {
        bencher.iter(|| {
            mouse.sequence = mouse.sequence.wrapping_add(1);
            let encoded = RealtimeInputCodec::encode(black_box(&mouse)).unwrap();
            black_box(RealtimeInputCodec::decode(&encoded).unwrap())
        })
    });

    let reliable = [
        ReliableInputFrame {
            protocol_version: INPUT_PROTOCOL_VERSION,
            session_epoch: SessionEpoch(1),
            sequence: 1,
            captured_at: MonotonicStamp::new(ClockDomainId(1), 1_001),
            event: ReliableInputEvent::Key {
                keycode: 0x41,
                state: KeyState::Pressed,
            },
        },
        ReliableInputFrame {
            protocol_version: INPUT_PROTOCOL_VERSION,
            session_epoch: SessionEpoch(1),
            sequence: 2,
            captured_at: MonotonicStamp::new(ClockDomainId(1), 1_002),
            event: ReliableInputEvent::MouseButton {
                button: MouseButton::Left,
                state: ButtonState::Pressed,
                x: 100,
                y: 200,
                realtime_anchor_sequence: 0,
            },
        },
    ];
    group.bench_function("reliable_key_button_encode_decode", |bencher| {
        bencher.iter(|| {
            for frame in &reliable {
                let encoded = ReliableInputCodec::encode(black_box(frame)).unwrap();
                black_box(ReliableInputCodec::decode(&encoded).unwrap());
            }
        })
    });
    group.finish();
}

criterion_group!(benches, codec_benches);
criterion_main!(benches);
