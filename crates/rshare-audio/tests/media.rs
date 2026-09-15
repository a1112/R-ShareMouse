use rshare_audio::engine::*;
use rshare_audio::media::*;
use rshare_core::{network_audio::*, DeviceId};
fn format() -> Format {
    Format {
        sample_rate: 96000,
        channels: 8,
        encoding: Encoding::Float32,
    }
}
#[test]
fn mtu_fragmentation_round_trip_and_old_generation_rejection() {
    let stream = DeviceId::new_v4();
    let format = format();
    let pcm = vec![42; format.packet_bytes()];
    let packets = encode_frame(stream, 4, 12, 1152, format, &pcm, 1100).unwrap();
    assert!(packets.len() > 1);
    assert!(packets.iter().all(|p| p.len() <= 1100));
    let mut receiver = Reassembler::new(stream, 4, format).unwrap();
    let mut frame = None;
    for packet in packets.iter().rev() {
        if let Some(f) = receiver.push(packet, 0).unwrap() {
            frame = Some(f);
        }
    }
    assert_eq!(frame.unwrap().data, pcm);
    assert!(receiver.push(&packets[0], 1).unwrap().is_none());
    let stale = encode_frame(stream, 3, 13, 1248, format, &pcm, 1100).unwrap();
    assert_eq!(
        receiver.push(&stale[0], 1),
        Err(AudioError::StaleGeneration)
    );
}
#[test]
fn malformed_and_incomplete_frames_remain_bounded() {
    let stream = DeviceId::new_v4();
    let mut receiver = Reassembler::new(stream, 1, format()).unwrap();
    assert!(receiver.push(&vec![0; 65536], 0).is_err());
    for i in 0..1000 {
        let packets = encode_frame(stream, 1, i, i * 96, format(), &vec![0; 3072], 1100).unwrap();
        receiver.push(&packets[0], i).unwrap();
    }
    assert!(receiver.pending() <= 20);
    receiver.expire(2000);
    assert_eq!(receiver.pending(), 0);
}
#[test]
fn float_pcm24_conversion_preserves_channel_order_and_sanitizes_nan() {
    let input = [-1.0, -0.5, 0.0, 0.5, 0.999, f32::NAN];
    let mut encoded = vec![0; 18];
    encode_samples(&input, Encoding::Pcm24, &mut encoded).unwrap();
    let mut decoded = [0.0; 6];
    decode_samples(&encoded, Encoding::Pcm24, &mut decoded).unwrap();
    for i in 0..5 {
        assert!((input[i] - decoded[i]).abs() < 0.000001);
    }
    assert_eq!(decoded[5], 0.0);
}
#[test]
fn playout_reorders_and_never_plays_stale_or_wrong_stream_data() {
    let mut buffer = Playout::new(
        Format {
            sample_rate: 48000,
            channels: 2,
            encoding: Encoding::Float32,
        },
        3,
    )
    .unwrap();
    let a = vec![0.25; 96];
    assert!(buffer.insert(48, &a));
    assert!(buffer.insert(0, &a));
    assert!(buffer.insert(96, &a));
    assert!(!buffer.insert(48, &a));
    let mut out = vec![0.0; 96];
    buffer.render(&mut out);
    assert!(out.iter().any(|x| *x != 0.0));
    for _ in 0..10 {
        buffer.render(&mut out);
    }
    assert_eq!(out, vec![0.0; 96]);
    assert!(!buffer.insert(0, &a));
}
#[test]
fn drift_controller_tracks_both_clock_directions_without_unbounded_latency() {
    for ppm in [-200.0, 200.0] {
        let mut clock = DriftController::new(288.0);
        let mut depth = 288.0;
        for _ in 0..480000 {
            let ratio = clock.update(depth);
            depth += 96.0 * (1.0 + ppm / 1_000_000.0 - ratio);
        }
        assert!((depth - 288.0).abs() < 3.0, "depth {depth} ppm {ppm}");
        assert!((clock.ppm() - ppm).abs() < 2.0);
    }
}
