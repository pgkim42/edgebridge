//! 실제 Quartz backend를 생성하지 않는 네트워크 + actor 통합 검증.
use edgebridge_client::{Backend, run};
use edgebridge_core::session::Inject;
use edgebridge_protocol::*;
use edgebridge_test_peer::{Conn, Script, ServerOptions, run_script_server, sample_mac_screen};
use std::{
    net::TcpListener,
    time::{Duration, Instant},
};
#[derive(Default)]
struct Recorder {
    commands: Vec<Inject>,
    deny: bool,
    fail_down: bool,
}
impl Backend for Recorder {
    fn check(&mut self) -> Result<(), String> {
        if self.deny {
            Err("permission denied".into())
        } else {
            Ok(())
        }
    }
    fn inject(&mut self, c: Inject) -> Result<(), String> {
        if self.fail_down && matches!(c, Inject::Button { down: true, .. }) {
            return Err("allocation failed".into());
        }
        self.commands.push(c);
        Ok(())
    }
}
fn options() -> ServerOptions {
    ServerOptions {
        name: "windows".into(),
        expected_peer: "mac".into(),
        capabilities: Capabilities::POINTER,
        hold: Duration::ZERO,
    }
}
#[test]
fn permission_denied_never_connects_or_posts() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut backend = Recorder {
        deny: true,
        ..Default::default()
    };
    assert!(
        run(
            listener.local_addr().unwrap(),
            "mac".into(),
            sample_mac_screen(),
            &mut backend
        )
        .is_err()
    );
    assert!(backend.commands.is_empty());
    assert!(listener.accept().is_err());
}
fn scenario(script: Script) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let peer =
        std::thread::spawn(move || run_script_server(&listener, &options(), script).unwrap());
    let mut backend = Recorder::default();
    let start = Instant::now();
    let result = run(addr, "mac".into(), sample_mac_screen(), &mut backend);
    if script == Script::Blackhole {
        assert!(
            result.as_ref().unwrap_err().contains("lease expired"),
            "{result:?}"
        );
    }
    peer.join().unwrap();
    if matches!(script, Script::M1 | Script::Click) {
        assert!(result.is_ok(), "{result:?}");
    } else {
        assert!(result.is_err());
    }
    assert!(start.elapsed() < Duration::from_secs(12));
    assert!(
        backend
            .commands
            .iter()
            .filter(|c| matches!(c, Inject::Move { .. }))
            .count()
            >= 6
    );
    if script != Script::M1 {
        assert_eq!(
            backend
                .commands
                .iter()
                .filter(|c| matches!(c, Inject::Button { down: true, .. }))
                .count(),
            1
        );
        assert_eq!(
            backend
                .commands
                .iter()
                .filter(|c| matches!(c, Inject::Button { down: false, .. }))
                .count(),
            1
        );
        assert!(matches!(
            backend.commands.last(),
            Some(Inject::Button { down: false, .. })
        ));
    }
}
#[test]
fn normal_m1() {
    scenario(Script::M1)
}
#[test]
fn click() {
    scenario(Script::Click)
}
#[test]
fn eof_releases_held_button() {
    scenario(Script::Eof)
}
#[test]
fn blackhole_releases_held_button() {
    scenario(Script::Blackhole)
}

fn handshake(conn: &mut Conn) {
    use edgebridge_core::handshake::{LocalHello, ServerHandshake, ServerStep};
    let (mut hs, hello) = ServerHandshake::start(
        LocalHello {
            name: "windows".into(),
            capabilities: Capabilities::POINTER,
        },
        "mac".into(),
        [1; 16],
    );
    conn.send(&hello).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        assert!(Instant::now() < deadline);
        if let Some(m) = conn.recv().unwrap() {
            match hs.on_message(m).unwrap() {
                ServerStep::Send(m) => conn.send(&m).unwrap(),
                ServerStep::Established(_) => break,
            }
        }
    }
}
fn malicious(messages: Vec<Message>, mut backend: Recorder) -> Recorder {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let mut conn = Conn::new(listener.accept().unwrap().0).unwrap();
        handshake(&mut conn);
        for m in messages {
            conn.send(&m).unwrap();
        }
        std::thread::sleep(Duration::from_millis(150));
    });
    assert!(run(addr, "mac".into(), sample_mac_screen(), &mut backend).is_err());
    peer.join().unwrap();
    backend
}
fn enter() -> Message {
    Message::Enter(Enter {
        epoch: 1,
        screen_id: 1,
        edge: Edge::Right,
        position: Position { x: 10.0, y: 10.0 },
    })
}
#[test]
fn invalid_epoch_aborts_without_posting_bad_move() {
    let b = malicious(
        vec![
            enter(),
            Message::MouseMove(MouseMove {
                epoch: 2,
                position: Position { x: 20.0, y: 20.0 },
            }),
        ],
        Recorder::default(),
    );
    assert_eq!(b.commands.len(), 1);
    let b = malicious(
        vec![Message::MouseMove(MouseMove {
            epoch: 1,
            position: Position { x: 20.0, y: 20.0 },
        })],
        Recorder::default(),
    );
    assert!(b.commands.is_empty());
}
#[test]
fn failed_down_is_not_released() {
    let b = malicious(
        vec![
            enter(),
            Message::Button(Button {
                epoch: 1,
                button: MouseButton::Left,
                action: ButtonAction::Down,
                position: Position { x: 10.0, y: 10.0 },
                source_ms: 0,
            }),
            Message::Ping(Ping { token: 0 }),
        ],
        Recorder {
            fail_down: true,
            ..Default::default()
        },
    );
    assert!(b.commands.iter().all(|c| matches!(c, Inject::Move { .. })));
}

#[test]
fn pong_is_emitted_only_after_backend_finishes_prior_input() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct SlowBackend(Arc<AtomicBool>);
    impl Backend for SlowBackend {
        fn check(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn inject(&mut self, _: Inject) -> Result<(), String> {
            std::thread::sleep(Duration::from_millis(30));
            self.0.store(true, Ordering::SeqCst);
            Ok(())
        }
    }
    let posted = Arc::new(AtomicBool::new(false));
    let observed = posted.clone();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let mut conn = Conn::new(listener.accept().unwrap().0).unwrap();
        handshake(&mut conn);
        conn.send(&enter()).unwrap();
        conn.send(&Message::Ping(Ping { token: 77 })).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut ack = false;
        loop {
            assert!(Instant::now() < deadline);
            match conn.recv().unwrap() {
                Some(Message::EnterAck(_)) => {
                    assert!(observed.load(Ordering::SeqCst));
                    ack = true;
                }
                Some(Message::Pong(Pong { token: 77 })) => {
                    assert!(ack);
                    assert!(observed.load(Ordering::SeqCst));
                    break;
                }
                Some(Message::Ping(p)) => {
                    conn.send(&Message::Pong(Pong { token: p.token })).unwrap()
                }
                None => {}
                other => panic!("{other:?}"),
            }
        }
        conn.send(&Message::Disconnect(Disconnect {
            reason: Reason::Normal,
        }))
        .unwrap();
        std::thread::sleep(Duration::from_millis(50));
    });
    run(
        addr,
        "mac".into(),
        sample_mac_screen(),
        &mut SlowBackend(posted),
    )
    .unwrap();
    peer.join().unwrap();
}

#[test]
fn stale_queue_discards_input_and_releases_prior_down() {
    struct Stalled {
        commands: Vec<Inject>,
    }
    impl Backend for Stalled {
        fn check(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn inject(&mut self, c: Inject) -> Result<(), String> {
            self.commands.push(c);
            if matches!(c, Inject::Button { down: true, .. }) {
                std::thread::sleep(Duration::from_millis(150));
            }
            Ok(())
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let mut conn = Conn::new(listener.accept().unwrap().0).unwrap();
        handshake(&mut conn);
        conn.send(&enter()).unwrap();
        conn.send(&Message::Button(Button {
            epoch: 1,
            button: MouseButton::Left,
            action: ButtonAction::Down,
            position: Position { x: 10.0, y: 10.0 },
            source_ms: 0,
        }))
        .unwrap();
        conn.send(&Message::MouseMove(MouseMove {
            epoch: 1,
            position: Position { x: 80.0, y: 80.0 },
        }))
        .unwrap();
        std::thread::sleep(Duration::from_millis(400));
    });
    let mut backend = Stalled {
        commands: Vec::new(),
    };
    let error = run(addr, "mac".into(), sample_mac_screen(), &mut backend).unwrap_err();
    assert!(error.contains("queue too old"), "{error}");
    assert!(matches!(
        backend.commands.last(),
        Some(Inject::Button { down: false, .. })
    ));
    assert!(!backend.commands.iter().any(|c| matches!(
        c,
        Inject::Move {
            position: Position { x: 80.0, .. },
            ..
        }
    )));
    peer.join().unwrap();
}
