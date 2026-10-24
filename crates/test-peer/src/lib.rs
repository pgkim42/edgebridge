//! loopback fake peer. 실제 OS 입력 없이 codec·handshake·heartbeat를 socket 위에서 돌려 본다.
//!
//! 단일 thread blocking I/O로 단순하게 만든다. 실제 server/client의 Tokio 구조와는 다르다.
//! M1 script는 실제 client에 이동/버튼/fault를 전송한다. test에서는 recorder backend만 연결한다.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use edgebridge_core::Millis;
use edgebridge_core::handshake::{
    ClientHandshake, ClientStep, Established, HandshakeError, LocalHello, ServerHandshake,
    ServerStep,
};
use edgebridge_core::heartbeat::{Heartbeat, HeartbeatError, PingResponder};
use edgebridge_protocol::{
    Capabilities, DecodeError, Disconnect, EncodeError, FrameDecoder, FrameEncoder, Message,
    Reason, ScreenInfo,
};

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(10);

#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("encode: {0}")]
    Encode(#[from] EncodeError),
    #[error("decode: {0}")]
    Decode(#[from] DecodeError),
    #[error("handshake: {0}")]
    Handshake(#[from] HandshakeError),
    #[error("heartbeat: {0}")]
    Heartbeat(#[from] HeartbeatError),
    #[error("handshake did not finish within 3s")]
    HandshakeTimeout,
    #[error("Enter/Leave acknowledgement did not arrive within 500ms")]
    AckTimeout,
    #[error("connection closed by peer")]
    Eof,
    #[error("unexpected message after ready: {0:?}")]
    Unexpected(Box<Message>),
}

/// 한 TCP 연결의 송수신 상태.
pub struct Conn {
    stream: TcpStream,
    enc: FrameEncoder,
    dec: FrameDecoder,
    clock: Instant,
}

impl Conn {
    pub fn new(stream: TcpStream) -> Result<Self, PeerError> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(POLL))?;
        stream.set_write_timeout(Some(Duration::from_millis(250)))?;
        Ok(Self {
            stream,
            enc: FrameEncoder::new(),
            dec: FrameDecoder::new(),
            clock: Instant::now(),
        })
    }

    pub fn now(&self) -> Millis {
        Millis(self.clock.elapsed().as_millis() as u64)
    }

    pub fn send(&mut self, message: &Message) -> Result<(), PeerError> {
        let bytes = self.enc.encode(message)?;
        self.stream.write_all(&bytes)?;
        Ok(())
    }

    /// 최대 `POLL` 동안 기다려 frame 하나를 받는다.
    pub fn recv(&mut self) -> Result<Option<Message>, PeerError> {
        if let Some(frame) = self.dec.next_frame()? {
            return Ok(Some(frame.message));
        }
        let mut buf = [0u8; 4096];
        match self.stream.read(&mut buf) {
            Ok(0) => Err(PeerError::Eof),
            Ok(n) => {
                self.dec.push(&buf[..n]);
                Ok(self.dec.next_frame()?.map(|f| f.message))
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn recv_before(&mut self, deadline: Instant) -> Result<Message, PeerError> {
        loop {
            if Instant::now() >= deadline {
                return Err(PeerError::HandshakeTimeout);
            }
            if let Some(m) = self.recv()? {
                return Ok(m);
            }
        }
    }

    /// handshake 오류를 상대에게 best-effort로 알리고 그대로 돌려준다.
    fn fail(&mut self, err: HandshakeError) -> PeerError {
        if let Some(m) = err.to_message() {
            let _ = self.send(&m);
        }
        err.into()
    }
}

#[derive(Debug, Clone)]
pub struct Report {
    pub established: Established,
    pub pongs_received: u64,
    pub pings_answered: u64,
    pub ended_by: End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// 정해진 시간 동안 heartbeat를 유지한 뒤 내가 Disconnect를 보냈다.
    SentDisconnect,
    FaultScript,
    /// 상대가 Disconnect를 보냈다.
    PeerDisconnect(Reason),
}

#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub name: String,
    pub expected_peer: String,
    pub capabilities: Capabilities,
    /// Ready 후 heartbeat를 유지할 시간. 끝나면 Disconnect(Shutdown).
    pub hold: Duration,
}

/// 연결 하나를 받아 handshake → heartbeat → Disconnect.
pub fn run_fake_server(listener: &TcpListener, opts: &ServerOptions) -> Result<Report, PeerError> {
    run_script_server(listener, opts, Script::Heartbeat)
}

/// script는 OS 입력을 직접 실행하지 않는다. 연결된 client만 주입한다.
pub fn run_script_server(
    listener: &TcpListener,
    opts: &ServerOptions,
    script: Script,
) -> Result<Report, PeerError> {
    if !listener.local_addr()?.ip().is_loopback() {
        return Err(std::io::Error::other("test-peer is loopback-only").into());
    }
    let (stream, _) = listener.accept()?;
    let mut conn = Conn::new(stream)?;
    let mut session_id = [0u8; 16];
    getrandom::fill(&mut session_id).expect("OS random source");

    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let local = LocalHello {
        name: opts.name.clone(),
        capabilities: opts.capabilities,
    };
    let (mut hs, hello) = ServerHandshake::start(local, opts.expected_peer.clone(), session_id);
    conn.send(&hello)?;
    let established = loop {
        let m = conn.recv_before(deadline)?;
        match hs.on_message(m) {
            Ok(ServerStep::Send(reply)) => conn.send(&reply)?,
            Ok(ServerStep::Established(e)) => break e,
            Err(e) => return Err(conn.fail(e)),
        }
    };
    if script == Script::Heartbeat {
        heartbeat_phase(conn, established, Some(opts.hold))
    } else {
        script_phase(conn, established, script)
    }
}

#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub name: String,
    pub capabilities: Capabilities,
    pub screen: ScreenInfo,
}

/// 접속 → handshake → 상대가 끊을 때까지 heartbeat.
pub fn run_fake_client(
    addr: impl ToSocketAddrs,
    opts: &ClientOptions,
) -> Result<Report, PeerError> {
    let stream = TcpStream::connect(addr)?;
    if !stream.peer_addr()?.ip().is_loopback() {
        return Err(std::io::Error::other("test-peer is loopback-only").into());
    }
    let mut conn = Conn::new(stream)?;
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let local = LocalHello {
        name: opts.name.clone(),
        capabilities: opts.capabilities,
    };
    let mut hs = ClientHandshake::new(local, opts.screen);
    let established = loop {
        let m = conn.recv_before(deadline)?;
        match hs.on_message(m) {
            Ok(ClientStep::Send(reply)) => conn.send(&reply)?,
            Ok(ClientStep::ReadyReceived { echo, established }) => {
                // 실제 client는 여기서 Accessibility 권한과 backend 준비를 확인한다.
                conn.send(&echo)?;
                break established;
            }
            Err(e) => return Err(conn.fail(e)),
        }
    };
    heartbeat_phase(conn, established, None)
}

fn heartbeat_phase(
    mut conn: Conn,
    established: Established,
    hold: Option<Duration>,
) -> Result<Report, PeerError> {
    let started = Instant::now();
    let mut hb = Heartbeat::start(conn.now());
    let mut responder = PingResponder::default();
    let mut pongs_received = 0;
    let mut pings_answered = 0;
    let report = |ended_by, pongs_received, pings_answered| Report {
        established: established.clone(),
        pongs_received,
        pings_answered,
        ended_by,
    };

    loop {
        if hold.is_some_and(|h| started.elapsed() >= h) {
            conn.send(&Message::Disconnect(Disconnect {
                reason: Reason::Shutdown,
            }))?;
            return Ok(report(End::SentDisconnect, pongs_received, pings_answered));
        }
        if let Some(ping) = hb.poll(conn.now())? {
            conn.send(&Message::Ping(ping))?;
        }
        match conn.recv()? {
            None => {}
            Some(Message::Ping(ping)) => {
                let pong = responder.on_ping(ping)?;
                conn.send(&Message::Pong(pong))?;
                pings_answered += 1;
            }
            Some(Message::Pong(pong)) => {
                hb.on_pong(pong, conn.now())?;
                pongs_received += 1;
            }
            Some(Message::Disconnect(d)) => {
                return Ok(report(
                    End::PeerDisconnect(d.reason),
                    pongs_received,
                    pings_answered,
                ));
            }
            Some(other) => return Err(PeerError::Unexpected(Box::new(other))),
        }
    }
}

/// 내장 Retina MacBook 화면을 흉내 낸 값.
pub fn sample_mac_screen() -> ScreenInfo {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    Heartbeat,
    M1,
    Click,
    Eof,
    Blackhole,
}
fn script_phase(
    mut conn: Conn,
    established: Established,
    script: Script,
) -> Result<Report, PeerError> {
    use edgebridge_protocol::{Button, ButtonAction, Edge, Enter, Leave, MouseButton, MouseMove};
    let screen = established.peer_screen.expect("client screen");
    let center = edgebridge_protocol::Position {
        x: screen.width / 2.0,
        y: screen.height / 2.0,
    };
    let positions = [
        center,
        edgebridge_protocol::Position { x: 2.0, y: 2.0 },
        edgebridge_protocol::Position {
            x: screen.width - 3.0,
            y: 2.0,
        },
        edgebridge_protocol::Position {
            x: screen.width - 3.0,
            y: screen.height - 3.0,
        },
        edgebridge_protocol::Position {
            x: 2.0,
            y: screen.height - 3.0,
        },
    ];
    let mut hb = Heartbeat::start(conn.now());
    let mut responder = PingResponder::default();
    let mut pongs = 0;
    let mut pings = 0;
    let mut due = Instant::now() + Duration::from_secs(3);
    let mut step = 0;
    let mut waiting: Option<(bool, Instant)> = None;
    let mut fault_token = None;
    eprintln!("M1: 3 second countdown; script={script:?}");
    loop {
        if waiting.is_some_and(|(_, at)| at.elapsed() >= Duration::from_millis(500)) {
            return Err(PeerError::AckTimeout);
        }
        if let Some(ping) = hb.poll(conn.now())? {
            if step == 8
                && matches!(script, Script::Eof | Script::Blackhole)
                && fault_token.is_none()
            {
                fault_token = Some(ping.token);
            }
            conn.send(&Message::Ping(ping))?;
        }
        if waiting.is_none() && Instant::now() >= due {
            match step {
                0 => {
                    conn.send(&Message::Enter(Enter {
                        epoch: 1,
                        screen_id: screen.id,
                        edge: Edge::Right,
                        position: edgebridge_protocol::Position {
                            x: screen.width - 2.0,
                            y: center.y,
                        },
                    }))?;
                    waiting = Some((true, Instant::now()));
                }
                1..=5 => conn.send(&Message::MouseMove(MouseMove {
                    epoch: 1,
                    position: positions[step - 1],
                }))?,
                6 if script != Script::M1 => conn.send(&Message::Button(Button {
                    epoch: 1,
                    button: MouseButton::Left,
                    action: ButtonAction::Down,
                    position: center,
                    source_ms: conn.now().0,
                }))?,
                7 if script == Script::Click => conn.send(&Message::Button(Button {
                    epoch: 1,
                    button: MouseButton::Left,
                    action: ButtonAction::Up,
                    position: center,
                    source_ms: conn.now().0,
                }))?,
                8 if matches!(script, Script::Eof | Script::Blackhole) => {}
                8 => {
                    conn.send(&Message::Leave(Leave {
                        epoch: 1,
                        reason: Reason::Normal,
                    }))?;
                    waiting = Some((false, Instant::now()));
                }
                9 => {
                    conn.send(&Message::Disconnect(Disconnect {
                        reason: Reason::Shutdown,
                    }))?;
                    std::thread::sleep(Duration::from_millis(50));
                    return Ok(Report {
                        established,
                        pongs_received: pongs,
                        pings_answered: pings,
                        ended_by: End::SentDisconnect,
                    });
                }
                _ => {}
            }
            if step != 8 || !matches!(script, Script::Eof | Script::Blackhole) {
                step += 1;
            }
            due = Instant::now() + Duration::from_millis(500);
        }
        match conn.recv()? {
            Some(Message::Ping(p)) => {
                conn.send(&Message::Pong(responder.on_ping(p)?))?;
                pings += 1;
            }
            Some(Message::Pong(p)) => {
                hb.on_pong(p, conn.now())?;
                pongs += 1;
                if fault_token == Some(p.token) {
                    // Down 다음 actor barrier 완료 후 fault: release를 관측할 held 상태 보장.
                    if script == Script::Blackhole {
                        std::thread::sleep(Duration::from_millis(1500));
                    }
                    return Ok(Report {
                        established,
                        pongs_received: pongs,
                        pings_answered: pings,
                        ended_by: End::FaultScript,
                    });
                }
            }
            Some(Message::EnterAck(a))
                if a.epoch == 1 && waiting.is_some_and(|(enter, _)| enter) =>
            {
                waiting = None
            }
            Some(Message::LeaveAck(a))
                if a.epoch == 1 && waiting.is_some_and(|(enter, _)| !enter) =>
            {
                waiting = None
            }
            None => {}
            Some(other) => return Err(PeerError::Unexpected(Box::new(other))),
        }
    }
}
