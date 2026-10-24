use edgebridge_core::{
    Millis,
    session::{ClientSession, Effect, Inject, global_position},
};
use edgebridge_protocol::*;
fn screen() -> ScreenInfo {
    ScreenInfo {
        id: 7,
        origin_x: -1512.0,
        origin_y: -100.0,
        width: 1512.0,
        height: 982.0,
        pixel_width: 3024,
        pixel_height: 1964,
        scale_x: 2.0,
        scale_y: 2.0,
    }
}
fn position() -> Position {
    Position { x: 100.0, y: 50.0 }
}
fn enter(epoch: u64) -> Message {
    Message::Enter(Enter {
        epoch,
        screen_id: 7,
        edge: Edge::Right,
        position: position(),
    })
}
fn button(epoch: u64, button: MouseButton, action: ButtonAction) -> Message {
    Message::Button(Button {
        epoch,
        button,
        action,
        position: position(),
        source_ms: 10,
    })
}
fn apply(s: &mut ClientSession, effects: Vec<Effect>) {
    for e in effects {
        if let Effect::Inject(c) = e {
            s.committed(c);
        }
    }
}
fn receive(s: &mut ClientSession, m: Message) -> Vec<Effect> {
    s.receive(m, Millis(0)).unwrap()
}
fn activate() -> ClientSession {
    let mut s = ClientSession::new(screen(), Millis(0)).unwrap();
    let e = receive(&mut s, enter(1));
    apply(&mut s, e);
    s
}
#[test]
fn origin_and_retina_are_not_double_scaled() {
    assert_eq!(
        global_position(&screen(), position()).unwrap(),
        Position {
            x: -1412.0,
            y: -50.0
        }
    );
    let mut s = screen();
    s.origin_x = 300.0;
    s.origin_y = 200.0;
    assert_eq!(
        global_position(&s, position()).unwrap(),
        Position { x: 400.0, y: 250.0 }
    );
    s.width = 0.0;
    assert!(global_position(&s, position()).is_err());
}
#[test]
fn rejects_invalid_screen_and_every_position_boundary() {
    let mut bad = screen();
    bad.scale_x = f64::NAN;
    assert!(ClientSession::new(bad, Millis(0)).is_err());
    for p in [
        Position {
            x: f64::NAN,
            y: 0.0,
        },
        Position {
            x: f64::INFINITY,
            y: 0.0,
        },
        Position { x: -1.0, y: 0.0 },
        Position { x: 1512.0, y: 0.0 },
        Position { x: 0.0, y: 982.0 },
    ] {
        let mut s = activate();
        assert!(
            s.receive(
                Message::MouseMove(MouseMove {
                    epoch: 1,
                    position: p
                }),
                Millis(0)
            )
            .is_err()
        );
    }
    assert!(
        global_position(
            &screen(),
            Position {
                x: 1511.0,
                y: 981.0
            }
        )
        .is_ok()
    );
    let mut s = ClientSession::new(screen(), Millis(0)).unwrap();
    let Message::Enter(mut e) = enter(1) else {
        unreachable!()
    };
    e.screen_id = 99;
    assert!(s.receive(Message::Enter(e), Millis(0)).is_err());
    assert_eq!(s.active_epoch(), None);
}
#[test]
fn before_enter_wrong_epoch_after_leave_and_reused_epoch_fail() {
    let mut s = ClientSession::new(screen(), Millis(0)).unwrap();
    let movement = |epoch| {
        Message::MouseMove(MouseMove {
            epoch,
            position: position(),
        })
    };
    assert!(s.receive(movement(1), Millis(0)).is_err());
    let e = receive(&mut s, enter(1));
    apply(&mut s, e);
    assert!(s.receive(movement(2), Millis(0)).is_err());
    assert!(s.receive(enter(2), Millis(0)).is_err());
    let e = receive(
        &mut s,
        Message::Leave(Leave {
            epoch: 1,
            reason: Reason::Normal,
        }),
    );
    apply(&mut s, e);
    assert!(s.receive(movement(1), Millis(0)).is_err());
    assert!(s.receive(enter(1), Millis(0)).is_err());
}
#[test]
fn hundred_epochs_release_before_ack_and_leave_empty_ledger() {
    let mut s = ClientSession::new(screen(), Millis(0)).unwrap();
    for epoch in 1..=100 {
        let e = receive(&mut s, enter(epoch));
        assert!(matches!(e[0], Effect::Inject(_)));
        assert!(matches!(e[1], Effect::Send(Message::EnterAck(_))));
        apply(&mut s, e);
        let e = receive(&mut s, button(epoch, MouseButton::Left, ButtonAction::Down));
        apply(&mut s, e);
        let e = receive(
            &mut s,
            Message::Leave(Leave {
                epoch,
                reason: Reason::Normal,
            }),
        );
        assert!(matches!(
            e[0],
            Effect::Inject(Inject::Button { down: false, .. })
        ));
        assert!(matches!(e[1], Effect::Send(Message::LeaveAck(_))));
        apply(&mut s, e);
        assert!(s.held().is_empty());
    }
}
#[test]
fn ledger_tracks_posted_effects_and_oldest_drag_owner() {
    let mut s = activate();
    let e = receive(&mut s, button(1, MouseButton::Right, ButtonAction::Down));
    assert!(s.held().is_empty());
    apply(&mut s, e);
    let e = receive(&mut s, button(1, MouseButton::Left, ButtonAction::Down));
    apply(&mut s, e);
    let e = receive(
        &mut s,
        Message::MouseMove(MouseMove {
            epoch: 1,
            position: position(),
        }),
    );
    assert!(matches!(
        e[0],
        Effect::Inject(Inject::Move {
            drag: Some(MouseButton::Right),
            ..
        })
    ));
    let e = receive(&mut s, button(1, MouseButton::Right, ButtonAction::Up));
    apply(&mut s, e);
    let e = receive(
        &mut s,
        Message::MouseMove(MouseMove {
            epoch: 1,
            position: position(),
        }),
    );
    assert!(matches!(
        e[0],
        Effect::Inject(Inject::Move {
            drag: Some(MouseButton::Left),
            ..
        })
    ));
    let e = s.abort();
    assert_eq!(s.active_epoch(), None);
    apply(&mut s, e);
    assert!(s.abort().is_empty());
}
#[test]
fn duplicate_down_orphan_up_and_backwards_time_fail() {
    let mut s = activate();
    assert!(
        s.receive(button(1, MouseButton::Left, ButtonAction::Up), Millis(0))
            .is_err()
    );
    let e = receive(&mut s, button(1, MouseButton::Left, ButtonAction::Down));
    apply(&mut s, e);
    assert!(
        s.receive(button(1, MouseButton::Left, ButtonAction::Down), Millis(0))
            .is_err()
    );
    let Message::Button(mut b) = button(1, MouseButton::Left, ButtonAction::Up) else {
        unreachable!()
    };
    b.source_ms = 9;
    assert!(s.receive(Message::Button(b), Millis(0)).is_err());
}
#[test]
fn lease_input_cannot_renew_and_expired_pong_cannot_resurrect() {
    let mut s = activate();
    let e = receive(&mut s, button(1, MouseButton::Left, ButtonAction::Down));
    apply(&mut s, e);
    assert!(s.tick(Millis(0)).is_ok());
    assert!(
        s.receive(
            Message::MouseMove(MouseMove {
                epoch: 1,
                position: position()
            }),
            Millis(999)
        )
        .is_ok()
    );
    assert!(s.tick(Millis(1000)).is_err());
    assert!(
        s.receive(Message::Pong(Pong { token: 0 }), Millis(1000))
            .is_err()
    );
    let e = s.abort();
    apply(&mut s, e);
    assert!(s.held().is_empty());
    assert!(s.receive(enter(2), Millis(1000)).is_err());
}
#[test]
fn disconnect_releases_and_unsupported_input_fails() {
    let mut s = activate();
    let e = receive(&mut s, button(1, MouseButton::Back, ButtonAction::Down));
    apply(&mut s, e);
    assert!(
        s.receive(
            Message::Key(Key {
                epoch: 1,
                hid: HidUsage::keyboard(4),
                action: KeyAction::Down
            }),
            Millis(0)
        )
        .is_err()
    );
    let e = receive(
        &mut s,
        Message::Disconnect(Disconnect {
            reason: Reason::Shutdown,
        }),
    );
    apply(&mut s, e);
    assert!(s.is_closed());
    assert!(s.held().is_empty());
}
