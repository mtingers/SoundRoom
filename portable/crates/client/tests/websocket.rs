use soundroom_client::websocket::{
    client_frame, upgrade_accept, validate_close, Decoder, FrameEvent,
};
fn frame(opcode: u8, fin: bool, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![opcode | if fin { 128 } else { 0 }];
    match payload.len() {
        0..=125 => out.push(payload.len() as u8),
        126..=65535 => {
            out.push(126);
            out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        _ => {
            out.push(127);
            out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(payload);
    out
}
#[test]
fn rfc6455_accept_known_vector() {
    assert_eq!(
        upgrade_accept("dGhlIHNhbXBsZSBub25jZQ=="),
        "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    );
}
#[test]
fn all_partial_boundaries_and_utf8_fragments_with_interleaved_ping() {
    let mut bytes = frame(1, false, &[0xe2]);
    bytes.extend(frame(9, true, b"ping"));
    bytes.extend(frame(0, false, &[0x82]));
    bytes.extend(frame(0, true, &[0xac]));
    for split in 0..=bytes.len() {
        let mut d = Decoder::default();
        d.feed(&bytes[..split]).unwrap();
        d.feed(&bytes[split..]).unwrap();
        assert_eq!(d.next(), Some(FrameEvent::Ping(b"ping".to_vec())));
        assert_eq!(d.next(), Some(FrameEvent::Message("€".as_bytes().to_vec())));
        assert_eq!(d.next(), None);
    }
    let mut d = Decoder::default();
    for byte in bytes {
        d.feed(&[byte]).unwrap();
    }
    assert!(matches!(d.next(), Some(FrameEvent::Ping(_))));
    assert!(matches!(d.next(), Some(FrameEvent::Message(_))));
}
#[test]
fn rejects_all_reserved_flags_and_opcodes_and_server_masks() {
    for flags in [16, 32, 64, 112] {
        let mut bytes = frame(1, true, b"ok");
        bytes[0] |= flags;
        assert!(Decoder::default().feed(&bytes).is_err());
    }
    for op in [3, 4, 5, 6, 7, 11, 12, 13, 14, 15] {
        assert!(Decoder::default().feed(&frame(op, true, b"")).is_err());
    }
    assert!(Decoder::default().feed(&[129, 128, 1, 2, 3, 4]).is_err());
}
#[test]
fn rejects_fragmentation_and_control_violations() {
    for bytes in [
        frame(0, true, b"unexpected"),
        frame(9, false, b"ping"),
        frame(10, false, b"pong"),
        frame(8, false, b""),
        frame(9, true, &[0; 126]),
    ] {
        assert!(Decoder::default().feed(&bytes).is_err());
    }
    let mut d = Decoder::default();
    d.feed(&frame(1, false, b"first")).unwrap();
    assert!(d.feed(&frame(2, true, b"second")).is_err());
}
#[test]
fn rejects_noncanonical_lengths_reserved_bit_and_overflow_before_allocation() {
    for bytes in [
        vec![129, 126, 0, 125],
        vec![129, 127, 0, 0, 0, 0, 0, 0, 255, 255],
        vec![129, 127, 128, 0, 0, 0, 0, 0, 0, 0],
        vec![129, 127, 0, 0, 0, 0, 0, 128, 0, 1],
    ] {
        assert!(Decoder::default().feed(&bytes).is_err());
    }
}
#[test]
fn validates_message_utf8_only_after_fragments_complete() {
    for payload in [&[0xc0, 0xaf][..], &[0xff][..], &[0xed, 0xa0, 0x80][..]] {
        assert!(Decoder::default().feed(&frame(1, true, payload)).is_err());
    }
    let mut d = Decoder::default();
    d.feed(&frame(1, false, &[0xf0, 0x90])).unwrap();
    assert!(d.feed(&frame(0, true, &[0x80])).is_err());
}
#[test]
fn close_codes_reasons_and_dead_stream_are_validated() {
    for code in [0u16, 999, 1004, 1005, 1006, 1015, 1016, 2999, 5000, 65535] {
        assert!(validate_close(&code.to_be_bytes()).is_err());
    }
    for code in [
        1000u16, 1001, 1002, 1003, 1007, 1008, 1009, 1010, 1011, 1012, 1013, 1014, 3000, 4999,
    ] {
        assert!(validate_close(&code.to_be_bytes()).is_ok());
    }
    assert!(validate_close(&[1]).is_err());
    assert!(validate_close(&[3, 232, 255]).is_err());
    assert!(validate_close(&[]).is_ok());
    let mut d = Decoder::default();
    d.feed(&frame(8, true, &1000u16.to_be_bytes())).unwrap();
    assert!(matches!(d.next(), Some(FrameEvent::Close(_))));
    assert!(d.feed(&frame(1, true, b"after close")).is_err());
}
#[test]
fn outgoing_frames_always_mask_and_preserve_every_length_class() {
    for size in [0, 1, 125, 126, 65535, 65536, 1048576] {
        let payload = vec![b'a'; size];
        let bytes = client_frame(1, &payload).unwrap();
        assert_eq!(bytes[0], 129);
        assert_ne!(bytes[1] & 128, 0);
        let offset = match bytes[1] & 127 {
            126 => 4,
            127 => 10,
            _ => 2,
        };
        let mask = &bytes[offset..offset + 4];
        let decoded: Vec<_> = bytes[offset + 4..]
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ mask[i % 4])
            .collect();
        assert_eq!(decoded, payload);
    }
    assert!(client_frame(1, &[255]).is_err());
    assert!(client_frame(9, &[0; 126]).is_err());
    assert!(client_frame(8, &[1]).is_err());
    assert!(client_frame(0, b"").is_err());
}
#[test]
fn bounds_control_flood_queue() {
    let mut bytes = Vec::new();
    for _ in 0..65 {
        bytes.extend(frame(9, true, b""));
    }
    assert!(Decoder::default().feed(&bytes).is_err());
}
#[test]
fn extended_frames_and_fragmented_aggregate_are_bounded() {
    let mut d = Decoder::default();
    let bytes = frame(1, true, &vec![b'a'; 65536]);
    for chunk in bytes.chunks(997) {
        d.feed(chunk).unwrap();
    }
    assert_eq!(d.next(), Some(FrameEvent::Message(vec![b'a'; 65536])));
    let mut d = Decoder::default();
    let bytes = frame(1, false, &vec![b'a'; 8 * 1024 * 1024]);
    for chunk in bytes.chunks(16384) {
        d.feed(chunk).unwrap();
    }
    assert!(d.feed(&frame(0, true, b"x")).is_err());
}
