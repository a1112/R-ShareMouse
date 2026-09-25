use rshare_net::wake::{broadcast_for, magic_packet};
use std::net::Ipv4Addr;

#[test]
fn packet_has_magic_prefix_and_sixteen_mac_copies() {
    let packet = magic_packet("AA:BB:CC:DD:EE:02").unwrap();
    assert_eq!(packet.len(), 102);
    assert_eq!(&packet[..6], &[0xFF; 6]);
    for copy in packet[6..].chunks_exact(6) {
        assert_eq!(copy, &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0x02]);
    }
}

#[test]
fn only_same_subnet_interface_is_selected() {
    let local = Ipv4Addr::new(192, 168, 4, 10);
    let mask = Ipv4Addr::new(255, 255, 255, 0);
    assert_eq!(
        broadcast_for(local, mask, Some(Ipv4Addr::new(192, 168, 4, 77))),
        Some(Ipv4Addr::new(192, 168, 4, 255))
    );
    assert_eq!(
        broadcast_for(local, mask, Some(Ipv4Addr::new(192, 168, 5, 77))),
        None
    );
    assert_eq!(
        broadcast_for(local, mask, Some(Ipv4Addr::new(192, 168, 4, 255))),
        None
    );
}

#[test]
fn point_to_point_masks_have_no_broadcast_target() {
    assert_eq!(
        broadcast_for(
            Ipv4Addr::new(192, 168, 4, 10),
            Ipv4Addr::new(255, 255, 255, 254),
            None,
        ),
        None
    );
    assert_eq!(
        broadcast_for(
            Ipv4Addr::new(192, 168, 4, 10),
            Ipv4Addr::new(0, 0, 0, 0),
            None,
        ),
        None
    );
    assert_eq!(
        broadcast_for(
            Ipv4Addr::new(192, 168, 4, 10),
            Ipv4Addr::new(255, 0, 255, 0),
            None,
        ),
        None
    );
}
