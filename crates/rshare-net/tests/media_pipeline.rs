//! Real loopback QUIC integration; does not stand in for physical audio latency.
use rshare_audio::{
    engine::{decode_samples, encode_samples, Playout},
    media::{encode_frame, Reassembler},
    registry::Registry,
};
use rshare_core::{network_audio::*, DeviceId};
use rshare_net::{
    encryption::{Encryption, PeerCertificateFingerprint, QuicIdentity},
    media_transport::{Binding, Bindings, MediaListener, MediaSession},
};
use std::time::Duration;
fn identity() -> QuicIdentity {
    let (cert_der, key_der) = Encryption::generate_cert().unwrap();
    QuicIdentity { cert_der, key_der }
}
#[tokio::test]
async fn authorized_eight_channel_pcm_traverses_isolated_encrypted_media_path() {
    for encoding in [Encoding::Pcm24, Encoding::Float32] {
        for sample_rate in [48000, 96000] {
            let local = DeviceId::new_v4();
            let remote = DeviceId::new_v4();
            let stream = DeviceId::new_v4();
            let format = Format {
                sample_rate,
                channels: 8,
                encoding,
            };
            let endpoint = Endpoint {
                id: "native-studio-uid".into(),
                name: "Studio".into(),
                direction: Direction::Input,
                channels: 8,
                sample_rates: vec![48000, 96000],
                virtual_device: false,
                available: true,
            };
            let mut exporter = Registry::default();
            exporter.set_local(vec![endpoint.clone()]).unwrap();
            assert!(exporter.catalog_for(local, true).is_empty());
            exporter
                .grant(local, &endpoint.id, endpoint.direction)
                .unwrap();
            let mut importer = Registry::default();
            importer
                .reconcile(remote, "Studio PC", 1, exporter.catalog_for(local, true))
                .unwrap();
            let id = importer.devices()[0].id.clone();
            // Simulated registration is explicit; this test does not install an OS device.
            importer.set_registration(&id, Ok(()));
            let session = importer.open(&id, format, (0..8).collect()).unwrap();
            let client_identity = identity();
            let server_identity = identity();
            let bindings = Bindings::default();
            let binding = Binding {
                peer: local,
                fingerprint: PeerCertificateFingerprint::from_der(&client_identity.cert_der),
                token: DeviceId::new_v4(),
                generation: 1,
            };
            bindings.authorize(binding.clone()).unwrap();
            let server = MediaListener::bind(
                "127.0.0.1:0".parse().unwrap(),
                &server_identity,
                bindings.clone(),
            )
            .unwrap();
            let client_endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
            let (client, receiver) = tokio::join!(
                MediaSession::connect(
                    &client_endpoint,
                    server.local_addr().unwrap(),
                    &client_identity,
                    PeerCertificateFingerprint::from_der(&server_identity.cert_der),
                    &binding
                ),
                server.accept()
            );
            let mut client = client.unwrap();
            let receiver = receiver.unwrap();
            let mut reassembler = Reassembler::new(stream, 1, format).unwrap();
            let mut playout = Playout::new(format, 3).unwrap();
            let source: Vec<f32> = (0..format.frames_per_packet() * 8)
                .map(|i| (i % 8 + 1) as f32 / 16.0)
                .collect();
            let mut bytes = vec![0; format.packet_bytes()];
            encode_samples(&source, encoding, &mut bytes).unwrap();
            for sequence in 0..5 {
                let packets = encode_frame(
                    stream,
                    1,
                    sequence,
                    sequence * format.frames_per_packet() as u64,
                    format,
                    &bytes,
                    client.max_datagram_size().unwrap(),
                )
                .unwrap();
                for packet in packets {
                    client.send_datagram(packet.into()).unwrap();
                    let incoming =
                        tokio::time::timeout(Duration::from_secs(1), receiver.receive_datagram())
                            .await
                            .unwrap()
                            .unwrap();
                    if let Some(frame) = reassembler.push(&incoming, sequence).unwrap() {
                        let mut samples = vec![0.0; source.len()];
                        decode_samples(&frame.data, encoding, &mut samples).unwrap();
                        assert!(playout.insert(frame.sample_position, &samples));
                    }
                }
            }
            let mut output = vec![0.0; source.len()];
            playout.render(&mut output);
            playout.render(&mut output);
            for frame in output.chunks_exact(8) {
                for (channel, sample) in frame.iter().enumerate() {
                    assert!((*sample - (channel + 1) as f32 / 16.0).abs() < 0.00001);
                }
            }
            assert_eq!(playout.diagnostics.underruns, 0);
            bindings.revoke(local);
            importer.disconnected(remote);
            playout.reset();
            playout.render(&mut output);
            assert!(output.iter().all(|s| *s == 0.0));
            assert!(!importer.sessions().iter().any(|s| s.id == session.id));
            assert!(
                tokio::time::timeout(Duration::from_secs(1), client.connection.closed())
                    .await
                    .is_ok()
            );
            server.close();
            client_endpoint.close(0u32.into(), b"test complete");
        }
    }
}

#[tokio::test]
async fn unapproved_certificate_cannot_open_media_even_with_a_valid_control_token() {
    let server_identity = identity();
    let approved_identity = identity();
    let attacker = identity();
    let bindings = Bindings::default();
    let binding = Binding {
        peer: DeviceId::new_v4(),
        fingerprint: PeerCertificateFingerprint::from_der(&approved_identity.cert_der),
        token: DeviceId::new_v4(),
        generation: 1,
    };
    bindings.authorize(binding.clone()).unwrap();
    let listener =
        MediaListener::bind("127.0.0.1:0".parse().unwrap(), &server_identity, bindings).unwrap();
    let endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    let (_, accepted) = tokio::join!(
        MediaSession::connect(
            &endpoint,
            listener.local_addr().unwrap(),
            &attacker,
            PeerCertificateFingerprint::from_der(&server_identity.cert_der),
            &binding
        ),
        listener.accept()
    );
    assert!(accepted.is_err());
    listener.close();
    endpoint.close(0u32.into(), b"test complete");
}
