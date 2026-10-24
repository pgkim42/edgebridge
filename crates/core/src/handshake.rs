//! application handshake. PROTOCOL.md §4 하단, §7 Connect.
//!
//! ```text
//! S -> C  Hello(Server, screens=[])
//! C -> S  Hello(Client, [screen])
//! S -> C  Ready(session_id, caps = S ∩ C)
//! C -> S  Ready(same)            // client는 permission/backend 준비 후 echo
//! ```
//!
//! Ready 전에는 Hello/Ready/Error/Disconnect만 유효하다. 그 외 kind(특히 input)는 BadState.
//! 전체 3s handshake timeout과 socket 처리는 호출자 책임이다.

use edgebridge_protocol::{
    Capabilities, ErrorCode, ErrorMessage, Hello, Message, Ready, Role, ScreenInfo,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandshakeError {
    /// 상대에게 `Error(code)`를 best-effort로 보내고 close한다.
    #[error("{code:?}: {detail}")]
    Protocol {
        code: ErrorCode,
        detail: &'static str,
    },
    /// 상대가 Error를 보냈다. 응답 없이 close.
    #[error("peer reported {0:?}")]
    PeerError(ErrorCode),
    /// 상대가 Disconnect를 보냈다. 응답 없이 close.
    #[error("peer disconnected")]
    PeerDisconnected,
}

impl HandshakeError {
    const fn protocol(code: ErrorCode, detail: &'static str) -> Self {
        Self::Protocol { code, detail }
    }

    /// 상대에게 보낼 Error message. 상대가 먼저 끊은 경우 `None`.
    pub fn to_message(&self) -> Option<Message> {
        match self {
            Self::Protocol { code, detail } => Some(Message::Error(ErrorMessage {
                code: *code,
                detail: (*detail).to_owned(),
            })),
            Self::PeerError(_) | Self::PeerDisconnected => None,
        }
    }
}

/// handshake 결과. 양쪽 공통.
#[derive(Debug, Clone, PartialEq)]
pub struct Established {
    pub session_id: [u8; 16],
    pub capabilities: Capabilities,
    pub peer_name: String,
    /// server 쪽에서만 `Some`: client가 보고한 선택 화면.
    pub peer_screen: Option<ScreenInfo>,
}

#[derive(Debug, Clone)]
pub struct LocalHello {
    pub name: String,
    pub capabilities: Capabilities,
}

/// S/C capability 교집합. Pointer는 필수다.
pub fn negotiate(
    server: Capabilities,
    client: Capabilities,
) -> Result<Capabilities, HandshakeError> {
    let caps = server.intersection(client);
    if !caps.contains(Capabilities::POINTER) {
        return Err(HandshakeError::protocol(
            ErrorCode::Unsupported,
            "pointer capability is required",
        ));
    }
    Ok(caps)
}

/// Ready 전 공통 처리: 허용되지 않은 kind, 상대의 Error/Disconnect.
fn pre_ready_gate(message: &Message) -> Result<(), HandshakeError> {
    match message {
        Message::Error(e) => Err(HandshakeError::PeerError(e.code)),
        Message::Disconnect(_) => Err(HandshakeError::PeerDisconnected),
        m if !m.kind().allowed_before_ready() => Err(HandshakeError::protocol(
            ErrorCode::BadState,
            "only hello/ready are valid before ready",
        )),
        _ => Ok(()),
    }
}

fn unexpected() -> HandshakeError {
    HandshakeError::protocol(ErrorCode::BadState, "unexpected handshake message")
}

// ---------------------------------------------------------------- server

#[derive(Debug)]
enum ServerState {
    AwaitClientHello,
    AwaitReadyEcho { ready: Ready, peer: Hello },
    Done,
}

#[derive(Debug)]
pub enum ServerStep {
    /// 이 message를 보내고 계속 수신한다.
    Send(Message),
    /// Ready echo 확인 완료. lease를 시작하고 edge를 arm할 수 있다.
    Established(Established),
}

#[derive(Debug)]
pub struct ServerHandshake {
    local: LocalHello,
    expected_peer: String,
    session_id: [u8; 16],
    state: ServerState,
}

impl ServerHandshake {
    /// `expected_peer`: config의 `layout.left` 이름. `session_id`: 호출자가 만든 random 128-bit.
    /// 반환된 Hello를 가장 먼저 보낸다.
    pub fn start(
        local: LocalHello,
        expected_peer: String,
        session_id: [u8; 16],
    ) -> (Self, Message) {
        let hello = Message::Hello(Hello {
            role: Role::Server,
            name: local.name.clone(),
            capabilities: local.capabilities,
            screens: Vec::new(),
        });
        let hs = Self {
            local,
            expected_peer,
            session_id,
            state: ServerState::AwaitClientHello,
        };
        (hs, hello)
    }

    pub fn on_message(&mut self, message: Message) -> Result<ServerStep, HandshakeError> {
        pre_ready_gate(&message)?;
        let state = std::mem::replace(&mut self.state, ServerState::Done);
        match (state, message) {
            (ServerState::AwaitClientHello, Message::Hello(peer)) => {
                if peer.role != Role::Client {
                    return Err(HandshakeError::protocol(
                        ErrorCode::BadState,
                        "peer hello must have client role",
                    ));
                }
                if peer.name != self.expected_peer {
                    return Err(HandshakeError::protocol(
                        ErrorCode::InvalidValue,
                        "client name does not match configured neighbor",
                    ));
                }
                let capabilities = negotiate(self.local.capabilities, peer.capabilities)?;
                let ready = Ready {
                    session_id: self.session_id,
                    capabilities,
                };
                self.state = ServerState::AwaitReadyEcho { ready, peer };
                Ok(ServerStep::Send(Message::Ready(ready)))
            }
            (ServerState::AwaitReadyEcho { ready, peer }, Message::Ready(echo)) => {
                if echo != ready {
                    return Err(HandshakeError::protocol(
                        ErrorCode::InvalidValue,
                        "ready echo does not match",
                    ));
                }
                Ok(ServerStep::Established(Established {
                    session_id: ready.session_id,
                    capabilities: ready.capabilities,
                    peer_screen: peer.screens.first().copied(),
                    peer_name: peer.name,
                }))
            }
            _ => Err(unexpected()),
        }
    }
}

// ---------------------------------------------------------------- client

#[derive(Debug)]
enum ClientState {
    AwaitServerHello,
    AwaitReady { server: Hello },
    Done,
}

#[derive(Debug)]
pub enum ClientStep {
    Send(Message),
    /// server Ready 수신. permission/backend가 준비되면 `echo`를 보낸 뒤 Enter를 받을 수 있다.
    /// 준비에 실패하면 echo 대신 Error(Permission)를 보내고 close한다.
    ReadyReceived {
        echo: Message,
        established: Established,
    },
}

#[derive(Debug)]
pub struct ClientHandshake {
    local: LocalHello,
    screen: ScreenInfo,
    state: ClientState,
}

impl ClientHandshake {
    /// client는 server Hello를 먼저 기다린다.
    pub fn new(local: LocalHello, screen: ScreenInfo) -> Self {
        Self {
            local,
            screen,
            state: ClientState::AwaitServerHello,
        }
    }

    pub fn on_message(&mut self, message: Message) -> Result<ClientStep, HandshakeError> {
        pre_ready_gate(&message)?;
        let state = std::mem::replace(&mut self.state, ClientState::Done);
        match (state, message) {
            (ClientState::AwaitServerHello, Message::Hello(server)) => {
                if server.role != Role::Server {
                    return Err(HandshakeError::protocol(
                        ErrorCode::BadState,
                        "peer hello must have server role",
                    ));
                }
                let hello = Message::Hello(Hello {
                    role: Role::Client,
                    name: self.local.name.clone(),
                    capabilities: self.local.capabilities,
                    screens: vec![self.screen],
                });
                self.state = ClientState::AwaitReady { server };
                Ok(ClientStep::Send(hello))
            }
            (ClientState::AwaitReady { server }, Message::Ready(ready)) => {
                let expected = negotiate(server.capabilities, self.local.capabilities)?;
                if ready.capabilities != expected {
                    return Err(HandshakeError::protocol(
                        ErrorCode::InvalidValue,
                        "ready capabilities must equal hello intersection",
                    ));
                }
                Ok(ClientStep::ReadyReceived {
                    echo: Message::Ready(ready),
                    established: Established {
                        session_id: ready.session_id,
                        capabilities: ready.capabilities,
                        peer_name: server.name,
                        peer_screen: None,
                    },
                })
            }
            _ => Err(unexpected()),
        }
    }
}

// ---------------------------------------------------------------- admission

/// server listener 수준의 연결 수락 정책. Ready client는 하나, pending handshake는 최대 4개.
/// 새 연결이 기존 Ready 연결을 빼앗지 않는다.
#[derive(Debug, Default)]
pub struct Admission {
    ready: bool,
    pending: usize,
}

pub const MAX_PENDING_HANDSHAKES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmitError {
    /// `Error(Busy)`를 보내고 close.
    #[error("a client is already connected")]
    Busy,
    /// 응답 없이 close.
    #[error("too many pending handshakes")]
    TooManyPending,
}

impl Admission {
    pub fn admit(&mut self) -> Result<(), AdmitError> {
        if self.ready {
            return Err(AdmitError::Busy);
        }
        if self.pending >= MAX_PENDING_HANDSHAKES {
            return Err(AdmitError::TooManyPending);
        }
        self.pending += 1;
        Ok(())
    }

    /// pending 연결이 Ready가 되었는지 판단한다. 이미 다른 연결이 Ready면 Busy.
    pub fn promote(&mut self) -> Result<(), AdmitError> {
        debug_assert!(self.pending > 0);
        self.pending -= 1;
        if self.ready {
            return Err(AdmitError::Busy);
        }
        self.ready = true;
        Ok(())
    }

    /// handshake 실패로 pending 연결이 사라짐.
    pub fn abandon(&mut self) {
        debug_assert!(self.pending > 0);
        self.pending = self.pending.saturating_sub(1);
    }

    /// Ready 세션 종료.
    pub fn release(&mut self) {
        self.ready = false;
    }

    pub fn busy_message() -> Message {
        Message::Error(ErrorMessage {
            code: ErrorCode::Busy,
            detail: "another client is connected".to_owned(),
        })
    }
}
