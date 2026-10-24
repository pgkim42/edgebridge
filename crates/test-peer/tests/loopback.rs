//! fake server ↔ fake client를 실제 loopback TCP 위에서 맞물린다.

use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use edgebridge_core::handshake::HandshakeError;
use edgebridge_protocol::{Capabilities, ErrorCode, Reason};
use edgebridge_test_peer::*;

fn listen() -> (TcpListener, std::net::SocketAddr) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    (l, addr)
}

fn server_opts(caps: Capabilities, expected_peer: &str) -> ServerOptions {
    ServerOptions {
        name: "windows".into(),
        expected_peer: expected_peer.into(),
        capabilities: caps,
        hold: Duration::from_millis(800),
    }
}

fn client_opts(name: &str, caps: Capabilities) -> ClientOptions {
    ClientOptions {
        name: name.into(),
        capabilities: caps,
        screen: sample_mac_screen(),
    }
}

#[test]
fn handshake_heartbeat_and_disconnect_over_tcp() {
    let (listener, addr) = listen();
    let server = thread::spawn(move || {
        run_fake_server(&listener, &server_opts(Capabilities::ALL_V1, "mac"))
    });
    let client = run_fake_client(addr, &client_opts("mac", Capabilities::POINTER)).unwrap();
    let server = server.join().unwrap().unwrap();

    assert_eq!(server.established.session_id, client.established.session_id);
    assert_eq!(client.established.capabilities, Capabilities::POINTER);
    assert_eq!(server.ended_by, End::SentDisconnect);
    assert_eq!(client.ended_by, End::PeerDisconnect(Reason::Shutdown));
    // 800ms 동안 250ms 주기 Ping: 양방향 모두 최소 2회 왕복.
    assert!(server.pongs_received >= 2, "{server:?}");
    assert!(client.pongs_received >= 2, "{client:?}");
}

#[test]
fn wrong_client_name_gets_error_frame() {
    let (listener, addr) = listen();
    let server = thread::spawn(move || {
        run_fake_server(&listener, &server_opts(Capabilities::ALL_V1, "mac"))
    });
    let client_err =
        run_fake_client(addr, &client_opts("intruder", Capabilities::ALL_V1)).unwrap_err();
    let server_err = server.join().unwrap().unwrap_err();

    assert!(matches!(
        server_err,
        PeerError::Handshake(HandshakeError::Protocol {
            code: ErrorCode::InvalidValue,
            ..
        })
    ));
    // client는 server가 보낸 Error frame을 받고 끝난다.
    assert!(matches!(
        client_err,
        PeerError::Handshake(HandshakeError::PeerError(ErrorCode::InvalidValue))
    ));
}
