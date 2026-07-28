use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rshare_core::{KeyState, Message, MouseButton};
use rshare_net::{ControlMessageCodec, RealtimeInputCodec};

fn codec_benches(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("codec");
    let mouse = Message::MouseMove { x: 101, y: -303 };
    group.bench_function("realtime_mouse_encode_decode", |bencher| {
        let mut sequence = 0_u32;
        bencher.iter(|| {
            sequence = sequence.wrapping_add(1);
            let encoded = RealtimeInputCodec::encode_message(sequence, black_box(&mouse))
                .unwrap()
                .unwrap();
            black_box(RealtimeInputCodec::decode_message(&encoded).unwrap())
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
