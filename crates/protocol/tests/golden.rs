//! golden bytes. Rust encoder와 독립적으로 계산한 값이다 (PROTOCOL.md §2 규칙을 Python struct로 직접 적용).
//! encode/decode 양방향으로 대조해 같은 버그가 양쪽에서 상쇄되지 않게 한다.

use edgebridge_protocol::*;

fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect()
}

fn check(seq: u64, message: Message, golden: &str) {
    let golden = hex(golden);
    assert_eq!(
        encode_frame(seq, &message).unwrap(),
        golden,
        "encode {:?}",
        message.kind()
    );
    let frame = decode_frame(&golden[4..]).unwrap();
    assert_eq!(frame.header.seq, seq);
    assert_eq!(frame.header.version, PROTOCOL_VERSION);
    assert_eq!(frame.message, message);
}

fn mac_screen() -> ScreenInfo {
    ScreenInfo {
        id: 1,
        origin_x: 0.0,
        origin_y: 0.0,
        width: 1512.0,
        height: 982.0,
        pixel_width: 3024,
        pixel_height: 1964,
        scale_x: 2.0,
        scale_y: 2.0,
    }
}

#[test]
fn ping_matches_protocol_md_example() {
    check(
        1,
        Message::Ping(Ping { token: 0 }),
        "00 00 00 15 45 44 47 42 00 01 00 00 00 08 00 00 00 00 00 00 00 00 00 01 00",
    );
}

#[test]
fn server_hello() {
    check(
        1,
        Message::Hello(Hello {
            role: Role::Server,
            name: "windows".into(),
            capabilities: Capabilities::ALL_V1,
            screens: vec![],
        }),
        "00 00 00 1f 45 44 47 42 00 01 00 00 00 01 00 00 00 00 00 00 00 00 00 01
         00 07 77 69 6e 64 6f 77 73 0f 00",
    );
}

#[test]
fn client_hello_with_retina_screen() {
    check(
        1,
        Message::Hello(Hello {
            role: Role::Client,
            name: "mac".into(),
            capabilities: Capabilities::POINTER,
            screens: vec![mac_screen()],
        }),
        "00 00 00 50 45 44 47 42 00 01 00 00 00 01 00 00 00 00 00 00 00 00 00 01
         01 03 6d 61 63 01 01
         01
         00 00 00 00 00 00 00 00
         00 00 00 00 00 00 00 00
         00 00 00 00 00 a0 97 40
         00 00 00 00 00 b0 8e 40
         d0 17 ac 0f
         00 00 00 00 00 00 00 40
         00 00 00 00 00 00 00 40",
    );
}

#[test]
fn mouse_move() {
    check(
        5,
        Message::MouseMove(MouseMove {
            epoch: 1,
            position: Position { x: 100.0, y: 50.5 },
        }),
        "00 00 00 25 45 44 47 42 00 01 00 00 00 10 00 00 00 00 00 00 00 00 00 05
         01 00 00 00 00 00 00 59 40 00 00 00 00 00 40 49 40",
    );
}

#[test]
fn key_down_a() {
    check(
        300,
        Message::Key(Key {
            epoch: 1,
            hid: HidUsage::keyboard(0x04),
            action: KeyAction::Down,
        }),
        "00 00 00 18 45 44 47 42 00 01 00 00 00 12 00 00 00 00 00 00 00 00 01 2c
         01 07 04 00",
    );
}

#[test]
fn scroll_pixels_with_multibyte_epoch() {
    check(
        2,
        Message::Scroll(Scroll {
            epoch: 200,
            unit: ScrollUnit::Pixels,
            dx: -1.5,
            dy: 3.0,
            position: Position { x: 10.0, y: 20.0 },
        }),
        "00 00 00 37 45 44 47 42 00 01 00 00 00 13 00 00 00 00 00 00 00 00 00 02
         c8 01 01
         00 00 00 00 00 00 f8 bf
         00 00 00 00 00 00 08 40
         00 00 00 00 00 00 24 40
         00 00 00 00 00 00 34 40",
    );
}
