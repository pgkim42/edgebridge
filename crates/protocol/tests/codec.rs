//! stream 재조립과 거부 규칙. PROTOCOL.md §2, §8.

use edgebridge_protocol::*;

fn ping(token: u64) -> Message {
    Message::Ping(Ping { token })
}

fn frames(n: u64) -> (Vec<u8>, Vec<Message>) {
    let mut enc = FrameEncoder::new();
    let mut bytes = Vec::new();
    let mut msgs = Vec::new();
    for t in 0..n {
        let m = if t % 2 == 0 {
            ping(t)
        } else {
            Message::MouseMove(MouseMove {
                epoch: 1,
                position: Position {
                    x: t as f64,
                    y: 2.0,
                },
            })
        };
        bytes.extend(enc.encode(&m).unwrap());
        msgs.push(m);
    }
    (bytes, msgs)
}

fn drain(dec: &mut FrameDecoder) -> Vec<Message> {
    let mut out = Vec::new();
    while let Some(f) = dec.next_frame().unwrap() {
        out.push(f.message);
    }
    out
}

#[test]
fn byte_by_byte_fragmentation() {
    let (bytes, msgs) = frames(5);
    let mut dec = FrameDecoder::new();
    let mut got = Vec::new();
    for b in bytes {
        dec.push(&[b]);
        got.extend(drain(&mut dec));
    }
    assert_eq!(got, msgs);
    assert!(!dec.has_partial());
}

#[test]
fn concatenated_frames_in_one_read() {
    let (bytes, msgs) = frames(10);
    let mut dec = FrameDecoder::new();
    dec.push(&bytes);
    assert_eq!(drain(&mut dec), msgs);
}

#[test]
fn partial_frame_is_reported() {
    let (bytes, _) = frames(1);
    let mut dec = FrameDecoder::new();
    dec.push(&bytes[..bytes.len() - 1]);
    assert!(dec.next_frame().unwrap().is_none());
    assert!(dec.has_partial());
}

fn decode_err(bytes: &[u8]) -> DecodeError {
    let mut dec = FrameDecoder::new();
    dec.push(bytes);
    let err = dec.next_frame().unwrap_err();
    // 첫 오류 이후 decoder는 poisoned.
    assert!(matches!(dec.next_frame(), Err(DecodeError::Poisoned)));
    err
}

fn ping_bytes() -> Vec<u8> {
    encode_frame(1, &ping(0)).unwrap()
}

#[test]
fn rejects_length_out_of_range_before_payload_arrives() {
    assert!(matches!(
        decode_err(&[0, 0, 0, 19]),
        DecodeError::BadLength(19)
    ));
    assert!(matches!(
        decode_err(&[0, 1, 0, 1]),
        DecodeError::BadLength(65537)
    ));
    assert!(matches!(
        decode_err(&[0xff, 0xff, 0xff, 0xff]),
        DecodeError::BadLength(_)
    ));
}

#[test]
fn rejects_bad_magic_early() {
    let mut b = ping_bytes();
    b[4] = b'X';
    // 8 bytes만 와도 판정한다.
    assert!(matches!(decode_err(&b[..8]), DecodeError::BadMagic));
}

#[test]
fn rejects_other_versions_with_version_code() {
    for (major, minor) in [(1u16, 1u16), (2, 0), (0, 9)] {
        let mut b = ping_bytes();
        b[8..10].copy_from_slice(&major.to_be_bytes());
        b[10..12].copy_from_slice(&minor.to_be_bytes());
        let err = decode_err(&b);
        assert!(matches!(err, DecodeError::Version(v) if v == Version { major, minor }));
        assert_eq!(err.wire_code(), Some(ErrorCode::Version));
    }
}

#[test]
fn rejects_nonzero_flags() {
    let mut b = ping_bytes();
    b[15] = 1;
    assert!(matches!(decode_err(&b), DecodeError::BadFlags(1)));
}

#[test]
fn rejects_unknown_and_reserved_kinds() {
    let mut b = ping_bytes();
    b[13] = 11;
    assert!(matches!(decode_err(&b), DecodeError::UnknownKind(11)));
    b[13] = 33;
    let err = decode_err(&b);
    assert!(matches!(err, DecodeError::ReservedKind(33)));
    assert_eq!(err.wire_code(), Some(ErrorCode::Unsupported));
}

#[test]
fn rejects_trailing_payload_bytes() {
    let mut b = ping_bytes();
    b.push(0);
    b[3] += 1;
    assert!(matches!(decode_err(&b), DecodeError::TrailingBytes(1)));
}

#[test]
fn rejects_truncated_payload() {
    // Ready는 16-byte session id가 필요한데 payload가 비어 있다.
    let mut b = ping_bytes();
    b[13] = 2;
    b.truncate(24);
    b[3] = 20;
    assert!(matches!(decode_err(&b), DecodeError::Payload(_)));
}

#[test]
fn rejects_sequence_gap_and_restart() {
    let mut dec = FrameDecoder::new();
    dec.push(&encode_frame(2, &ping(0)).unwrap());
    assert!(matches!(
        dec.next_frame(),
        Err(DecodeError::Seq {
            expected: 1,
            got: 2
        })
    ));

    let mut dec = FrameDecoder::new();
    dec.push(&encode_frame(1, &ping(0)).unwrap());
    dec.push(&encode_frame(1, &ping(1)).unwrap());
    assert!(dec.next_frame().unwrap().is_some());
    assert!(matches!(
        dec.next_frame(),
        Err(DecodeError::Seq {
            expected: 2,
            got: 1
        })
    ));
}

#[test]
fn rejects_undefined_enum_values() {
    // Key action 3은 정의되지 않음.
    let key = Message::Key(Key {
        epoch: 1,
        hid: HidUsage::keyboard(4),
        action: KeyAction::Up,
    });
    let mut b = encode_frame(1, &key).unwrap();
    *b.last_mut().unwrap() = 3;
    assert!(matches!(decode_err(&b), DecodeError::Payload(_)));

    // Button action Repeat(2)은 Button에서 허용되지 않음.
    let button = Message::Button(Button {
        epoch: 1,
        button: MouseButton::Left,
        action: ButtonAction::Down,
        position: Position { x: 0.0, y: 0.0 },
        source_ms: 0,
    });
    let mut b = encode_frame(1, &button).unwrap();
    b[4 + 20 + 2] = 2; // epoch(1) + button(1) 다음이 action
    assert!(matches!(decode_err(&b), DecodeError::Payload(_)));
}

#[test]
fn rejects_non_finite_values_on_decode() {
    let mv = Message::MouseMove(MouseMove {
        epoch: 1,
        position: Position { x: 1.0, y: 1.0 },
    });
    let mut b = encode_frame(1, &mv).unwrap();
    let x = 4 + 20 + 1;
    b[x..x + 8].copy_from_slice(&f64::NAN.to_le_bytes());
    let err = decode_err(&b);
    assert!(matches!(err, DecodeError::Invalid(_)));
    assert_eq!(err.wire_code(), Some(ErrorCode::InvalidValue));
}

#[test]
fn encoder_refuses_invalid_messages() {
    let cases = [
        Message::MouseMove(MouseMove {
            epoch: 0,
            position: Position { x: 1.0, y: 1.0 },
        }),
        Message::MouseMove(MouseMove {
            epoch: 1,
            position: Position {
                x: f64::INFINITY,
                y: 1.0,
            },
        }),
        Message::MouseMove(MouseMove {
            epoch: 1,
            position: Position { x: -1.0, y: 1.0 },
        }),
        Message::Scroll(Scroll {
            epoch: 1,
            unit: ScrollUnit::Lines,
            dx: 10_001.0,
            dy: 0.0,
            position: Position { x: 0.0, y: 0.0 },
        }),
        Message::Hello(Hello {
            role: Role::Server,
            name: String::new(),
            capabilities: Capabilities::POINTER,
            screens: vec![],
        }),
        Message::Hello(Hello {
            role: Role::Server,
            name: "a\nb".into(),
            capabilities: Capabilities::POINTER,
            screens: vec![],
        }),
        Message::Hello(Hello {
            role: Role::Server,
            name: "x".repeat(65),
            capabilities: Capabilities::POINTER,
            screens: vec![],
        }),
        Message::Hello(Hello {
            role: Role::Client,
            name: "mac".into(),
            capabilities: Capabilities::POINTER,
            screens: vec![],
        }),
        Message::Hello(Hello {
            role: Role::Server,
            name: "w".into(),
            capabilities: Capabilities(1 << 10),
            screens: vec![],
        }),
        Message::Error(ErrorMessage {
            code: ErrorCode::BadFrame,
            detail: "x".repeat(257),
        }),
    ];
    for m in cases {
        assert!(
            matches!(encode_frame(1, &m), Err(EncodeError::Invalid(_))),
            "{m:?}"
        );
    }
}

#[test]
fn screen_info_validation() {
    let ok = ScreenInfo {
        id: 1,
        origin_x: -1512.0,
        origin_y: 0.0,
        width: 1512.0,
        height: 982.0,
        pixel_width: 3024,
        pixel_height: 1964,
        scale_x: 2.0,
        scale_y: 2.0,
    };
    ok.validate().unwrap();
    assert!(
        ScreenInfo { scale_x: 1.0, ..ok }.validate().is_err(),
        "ratio mismatch"
    );
    assert!(ScreenInfo { width: 8.0, ..ok }.validate().is_err());
    assert!(
        ScreenInfo {
            origin_x: 2_000_000.0,
            ..ok
        }
        .validate()
        .is_err()
    );
    assert!(
        ScreenInfo {
            height: f64::NAN,
            ..ok
        }
        .validate()
        .is_err()
    );

    let p = Position {
        x: 1511.0,
        y: 981.0,
    };
    assert!(p.is_within(&ok));
    assert!(!Position { x: 1511.5, y: 0.0 }.is_within(&ok));
}
