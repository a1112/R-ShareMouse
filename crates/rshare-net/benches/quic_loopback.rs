use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rshare_core::{
    ClockDomainId, KeyState, Message, MonotonicStamp, MouseButton, RealtimeInputFrame,
    RealtimeInputPayload, SessionEpoch, INPUT_PROTOCOL_VERSION,
};
use rshare_net::{ControlMessageCodec, RealtimeInputCodec};

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
        Message::Key {
            keycode: 0x41,
            state: KeyState::Pressed,
        },
        Message::MouseButton {
            button: MouseButton::Left,
            state: rshare_core::ButtonState::Pressed,
        },
    ];
    group.bench_function("reliable_key_button_encode_decode", |bencher| {
        bencher.iter(|| {
            for message in &reliable {
                let encoded = ControlMessageCodec::encode(black_box(message)).unwrap();
                black_box(ControlMessageCodec::decode(&encoded).unwrap());
            }
        })
    });
    group.finish();
}

criterion_group!(benches, codec_benches);
criterion_main!(benches);
