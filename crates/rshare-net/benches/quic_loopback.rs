use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rshare_core::{DeviceId, KeyState, Message, MouseButton};
use rshare_net::{ControlMessageCodec, QuicTransport, RealtimeInputCodec};
use std::time::Duration;

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

    let runtime = tokio::runtime::Runtime::new().expect("create benchmark Tokio runtime");
    let (mut server, mut client, sender, mut receiver) = runtime.block_on(async {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::new(server_id);
        server
            .start_server("127.0.0.1:0")
            .await
            .expect("start in-process QUIC benchmark server");
        let address = server.local_addr().expect("benchmark server local address");
        let mut incoming = server.incoming();
        let mut client = QuicTransport::new(client_id);
        let sender = client
            .connect(&address.to_string(), server_id)
            .await
            .expect("connect in-process QUIC benchmark client");
        let mut accepted = tokio::time::timeout(Duration::from_secs(3), incoming.recv())
            .await
            .expect("accept QUIC benchmark connection before deadline")
            .expect("QUIC benchmark incoming channel remains open")
            .connection;
        let receiver = accepted.message_channel();
        (server, client, sender, receiver)
    });
    let mut quic_group = criterion.benchmark_group("quic");
    quic_group.bench_function("in_process_mouse_roundtrip", |bencher| {
        bencher.iter(|| {
            runtime.block_on(async {
                sender
                    .send_message(black_box(&mouse))
                    .await
                    .expect("send benchmark message over QUIC");
                black_box(
                    tokio::time::timeout(Duration::from_secs(1), receiver.recv())
                        .await
                        .expect("receive benchmark message before deadline")
                        .expect("QUIC benchmark message channel remains open"),
                )
            })
        })
    });
    quic_group.finish();
    runtime.block_on(async {
        sender.close().await;
        client.close().await.expect("close QUIC benchmark client");
        server.close().await.expect("close QUIC benchmark server");
    });
}

criterion_group!(benches, codec_benches);
criterion_main!(benches);
