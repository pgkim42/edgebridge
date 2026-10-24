use edgebridge_core::handshake::{
    Admission, AdmitError, ClientHandshake, ClientStep, HandshakeError, LocalHello,
    ServerHandshake, ServerStep,
};
use edgebridge_protocol::{
    Capabilities as Caps, Disconnect, ErrorCode, Message, MouseMove, Position, Ready, Reason,
    ScreenInfo,
};

const SESSION: [u8; 16] = [7; 16];

fn screen() -> ScreenInfo {
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

fn hello(name: &str, caps: Caps) -> LocalHello {
    LocalHello {
        name: name.into(),
        capabilities: caps,
    }
}

fn send(step: ServerStep) -> Message {
    match step {
        ServerStep::Send(m) => m,
        other => panic!("expected Send, got {other:?}"),
    }
}

fn csend(step: ClientStep) -> Message {
    match step {
        ClientStep::Send(m) => m,
        other => panic!("expected Send, got {other:?}"),
    }
}

/// server와 client를 메모리 안에서 끝까지 맞물려 본다.
fn run(server_caps: Caps, client_caps: Caps) -> Result<Caps, HandshakeError> {
    let (mut s, s_hello) =
        ServerHandshake::start(hello("windows", server_caps), "mac".into(), SESSION);
    let mut c = ClientHandshake::new(hello("mac", client_caps), screen());

    let c_hello = csend(c.on_message(s_hello)?);
    let ready = send(s.on_message(c_hello)?);
    let ClientStep::ReadyReceived { echo, established } = c.on_message(ready)? else {
        panic!("client should receive ready");
    };
    let ServerStep::Established(server_side) = s.on_message(echo)? else {
        panic!("server should establish");
    };
    assert_eq!(server_side.session_id, established.session_id);
    assert_eq!(server_side.capabilities, established.capabilities);
    assert_eq!(server_side.peer_name, "mac");
    assert_eq!(established.peer_name, "windows");
    assert_eq!(server_side.peer_screen, Some(screen()));
    Ok(established.capabilities)
}

#[test]
fn full_capabilities_negotiate_to_intersection() {
    assert_eq!(run(Caps::ALL_V1, Caps::ALL_V1), Ok(Caps::ALL_V1));
}

#[test]
fn pointer_only_fake_peer_shrinks_capabilities() {
    // M1 fake peer는 Pointer만 광고한다.
    assert_eq!(run(Caps::ALL_V1, Caps::POINTER), Ok(Caps::POINTER));
    assert_eq!(
        run(Caps::POINTER | Caps::KEYBOARD, Caps::ALL_V1),
        Ok(Caps::POINTER | Caps::KEYBOARD)
    );
}

#[test]
fn missing_pointer_is_unsupported() {
    let err = run(Caps::KEYBOARD, Caps::ALL_V1).unwrap_err();
    assert!(matches!(
        err,
        HandshakeError::Protocol {
            code: ErrorCode::Unsupported,
            ..
        }
    ));
}

#[test]
fn input_before_ready_is_bad_state() {
    let (mut s, _) = ServerHandshake::start(hello("windows", Caps::ALL_V1), "mac".into(), SESSION);
    let mv = Message::MouseMove(MouseMove {
        epoch: 1,
        position: Position { x: 1.0, y: 1.0 },
    });
    let err = s.on_message(mv.clone()).unwrap_err();
    assert!(matches!(
        err,
        HandshakeError::Protocol {
            code: ErrorCode::BadState,
            ..
        }
    ));
    assert!(err.to_message().is_some());

    let mut c = ClientHandshake::new(hello("mac", Caps::ALL_V1), screen());
    let err = c.on_message(mv).unwrap_err();
    assert!(matches!(
        err,
        HandshakeError::Protocol {
            code: ErrorCode::BadState,
            ..
        }
    ));
}

#[test]
fn wrong_peer_name_is_rejected() {
    let (mut s, s_hello) =
        ServerHandshake::start(hello("windows", Caps::ALL_V1), "mac".into(), SESSION);
    let mut c = ClientHandshake::new(hello("macbook", Caps::ALL_V1), screen());
    let c_hello = csend(c.on_message(s_hello).unwrap());
    let err = s.on_message(c_hello).unwrap_err();
    assert!(matches!(
        err,
        HandshakeError::Protocol {
            code: ErrorCode::InvalidValue,
            ..
        }
    ));
}

#[test]
fn tampered_ready_echo_is_rejected() {
    let (mut s, s_hello) =
        ServerHandshake::start(hello("windows", Caps::ALL_V1), "mac".into(), SESSION);
    let mut c = ClientHandshake::new(hello("mac", Caps::ALL_V1), screen());
    let c_hello = csend(c.on_message(s_hello).unwrap());
    let _ready = send(s.on_message(c_hello).unwrap());
    let forged = Message::Ready(Ready {
        session_id: [9; 16],
        capabilities: Caps::ALL_V1,
    });
    let err = s.on_message(forged).unwrap_err();
    assert!(matches!(
        err,
        HandshakeError::Protocol {
            code: ErrorCode::InvalidValue,
            ..
        }
    ));
}

#[test]
fn client_rejects_ready_with_wrong_capabilities() {
    let (_, s_hello) =
        ServerHandshake::start(hello("windows", Caps::POINTER), "mac".into(), SESSION);
    let mut c = ClientHandshake::new(hello("mac", Caps::ALL_V1), screen());
    csend(c.on_message(s_hello).unwrap());
    let inflated = Message::Ready(Ready {
        session_id: SESSION,
        capabilities: Caps::ALL_V1,
    });
    let err = c.on_message(inflated).unwrap_err();
    assert!(matches!(
        err,
        HandshakeError::Protocol {
            code: ErrorCode::InvalidValue,
            ..
        }
    ));
}

#[test]
fn duplicate_hello_is_bad_state() {
    let (mut s, s_hello) =
        ServerHandshake::start(hello("windows", Caps::ALL_V1), "mac".into(), SESSION);
    let mut c = ClientHandshake::new(hello("mac", Caps::ALL_V1), screen());
    let c_hello = csend(c.on_message(s_hello).unwrap());
    send(s.on_message(c_hello.clone()).unwrap());
    let err = s.on_message(c_hello).unwrap_err();
    assert!(matches!(
        err,
        HandshakeError::Protocol {
            code: ErrorCode::BadState,
            ..
        }
    ));
}

#[test]
fn peer_disconnect_and_error_close_without_reply() {
    let (mut s, _) = ServerHandshake::start(hello("windows", Caps::ALL_V1), "mac".into(), SESSION);
    let err = s
        .on_message(Message::Disconnect(Disconnect {
            reason: Reason::Shutdown,
        }))
        .unwrap_err();
    assert_eq!(err, HandshakeError::PeerDisconnected);
    assert!(err.to_message().is_none());

    let mut c = ClientHandshake::new(hello("mac", Caps::ALL_V1), screen());
    let err = c.on_message(Admission::busy_message()).unwrap_err();
    assert_eq!(err, HandshakeError::PeerError(ErrorCode::Busy));
}

#[test]
fn admission_rejects_second_client_while_ready() {
    let mut a = Admission::default();
    a.admit().unwrap();
    a.promote().unwrap();
    assert_eq!(a.admit(), Err(AdmitError::Busy));
    a.release();
    a.admit().unwrap();
}

#[test]
fn admission_does_not_let_late_handshake_steal_session() {
    let mut a = Admission::default();
    a.admit().unwrap();
    a.admit().unwrap();
    a.promote().unwrap();
    assert_eq!(a.promote(), Err(AdmitError::Busy));
}

#[test]
fn admission_caps_pending_handshakes() {
    let mut a = Admission::default();
    for _ in 0..4 {
        a.admit().unwrap();
    }
    assert_eq!(a.admit(), Err(AdmitError::TooManyPending));
    a.abandon();
    a.admit().unwrap();
}
