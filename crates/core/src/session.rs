//! Ready 이후의 순수 client actor. 효과는 반환 순서대로 실행하며 실패하면 abort한다.
//! ledger는 post 완료 후 commit한다. 따라서 실패한 Down을 release 대상으로 오인하지 않는다.
use crate::{
    Millis,
    heartbeat::{Heartbeat, PingResponder},
};
use edgebridge_protocol::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Inject {
    Move {
        position: Position,
        drag: Option<MouseButton>,
    },
    Button {
        position: Position,
        button: MouseButton,
        down: bool,
    },
}
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Inject(Inject),
    Send(Message),
}
#[derive(Debug, thiserror::Error)]
#[error("client session: {detail}")]
pub struct SessionError {
    pub code: ErrorCode,
    pub detail: &'static str,
}
impl SessionError {
    fn state(detail: &'static str) -> Self {
        Self {
            code: ErrorCode::BadState,
            detail,
        }
    }
    fn invalid(detail: &'static str) -> Self {
        Self {
            code: ErrorCode::InvalidValue,
            detail,
        }
    }
    /// 상태 오류를 wire에 best-effort로 알린다. 연결 자체는 항상 폐기한다.
    pub fn to_message(&self) -> Message {
        Message::Error(ErrorMessage {
            code: self.code,
            detail: self.detail.into(),
        })
    }
}

pub fn global_position(screen: &ScreenInfo, p: Position) -> Result<Position, SessionError> {
    screen.validate().map_err(SessionError::invalid)?;
    if !p.is_within(screen) {
        return Err(SessionError::invalid("position outside selected screen"));
    }
    Ok(Position {
        x: screen.origin_x + p.x,
        y: screen.origin_y + p.y,
    })
}

pub struct ClientSession {
    screen: ScreenInfo,
    active: Option<u64>,
    last_epoch: u64,
    closed: bool,
    held: Vec<MouseButton>,
    position: Position,
    source_ms: u64,
    heartbeat: Heartbeat,
    responder: PingResponder,
}
impl ClientSession {
    pub fn new(screen: ScreenInfo, now: Millis) -> Result<Self, SessionError> {
        screen.validate().map_err(SessionError::invalid)?;
        Ok(Self {
            screen,
            active: None,
            last_epoch: 0,
            closed: false,
            held: Vec::new(),
            position: Position {
                x: screen.origin_x,
                y: screen.origin_y,
            },
            source_ms: 0,
            heartbeat: Heartbeat::start(now),
            responder: PingResponder::default(),
        })
    }
    pub fn active_epoch(&self) -> Option<u64> {
        self.active
    }
    pub fn held(&self) -> &[MouseButton] {
        &self.held
    }
    pub fn is_closed(&self) -> bool {
        self.closed
    }
    fn movement(&self, p: Position) -> Result<Effect, SessionError> {
        Ok(Effect::Inject(Inject::Move {
            position: global_position(&self.screen, p)?,
            drag: self.held.first().copied(),
        }))
    }
    /// 호출자는 오류 시 반드시 abort의 release 효과를 실행하고 연결을 닫는다.
    pub fn receive(&mut self, m: Message, now: Millis) -> Result<Vec<Effect>, SessionError> {
        if self.closed || self.heartbeat.is_expired(now) {
            return Err(SessionError::state("closed or expired"));
        }
        m.validate().map_err(SessionError::invalid)?;
        if let Some(epoch) = m.input_epoch()
            && self.active != Some(epoch)
        {
            return Err(SessionError::state("inactive or wrong epoch"));
        }
        Ok(match m {
            Message::Enter(e) => {
                if self.active.is_some()
                    || e.epoch <= self.last_epoch
                    || e.screen_id != self.screen.id
                {
                    return Err(SessionError::state("invalid Enter"));
                }
                let movement = self.movement(e.position)?;
                self.active = Some(e.epoch);
                self.last_epoch = e.epoch;
                vec![
                    movement,
                    Effect::Send(Message::EnterAck(EnterAck { epoch: e.epoch })),
                ]
            }
            Message::MouseMove(m) => vec![self.movement(m.position)?],
            Message::Button(b) => {
                if b.source_ms < self.source_ms
                    || self.held.contains(&b.button) == (b.action == ButtonAction::Down)
                {
                    return Err(SessionError::state("invalid button edge or timestamp"));
                }
                let movement = self.movement(b.position)?;
                self.source_ms = b.source_ms;
                vec![
                    movement,
                    Effect::Inject(Inject::Button {
                        position: global_position(&self.screen, b.position)?,
                        button: b.button,
                        down: b.action == ButtonAction::Down,
                    }),
                ]
            }
            Message::Leave(l) => {
                if self.active != Some(l.epoch) {
                    return Err(SessionError::state("invalid Leave"));
                }
                self.active = None;
                let mut effects = self.releases();
                effects.push(Effect::Send(Message::LeaveAck(LeaveAck { epoch: l.epoch })));
                effects
            }
            Message::Ping(p) => vec![Effect::Send(Message::Pong(
                self.responder
                    .on_ping(p)
                    .map_err(|_| SessionError::state("invalid Ping"))?,
            ))],
            Message::Pong(p) => {
                self.heartbeat
                    .on_pong(p, now)
                    .map_err(|_| SessionError::state("invalid Pong"))?;
                vec![]
            }
            Message::Disconnect(_) => self.abort(),
            Message::Error(_) => return Err(SessionError::state("peer reported error")),
            _ => {
                return Err(SessionError {
                    code: ErrorCode::Unsupported,
                    detail: "unsupported message",
                });
            }
        })
    }
    pub fn tick(&mut self, now: Millis) -> Result<Vec<Effect>, SessionError> {
        if self.closed {
            return Err(SessionError::state("closed"));
        }
        Ok(self
            .heartbeat
            .poll(now)
            .map_err(|_| SessionError::state("lease expired"))?
            .map(|p| Effect::Send(Message::Ping(p)))
            .into_iter()
            .collect())
    }
    /// OS post 호출이 끝난 효과만 기록한다. CGEventPost는 앱 소비 성공을 보고하지 않는다.
    pub fn committed(&mut self, command: Inject) {
        match command {
            Inject::Move { position, .. } => self.position = position,
            Inject::Button {
                position,
                button,
                down,
            } => {
                self.position = position;
                if down {
                    self.held.push(button);
                } else {
                    self.held.retain(|b| *b != button);
                }
            }
        }
    }
    fn releases(&self) -> Vec<Effect> {
        self.held
            .iter()
            .map(|&button| {
                Effect::Inject(Inject::Button {
                    position: self.position,
                    button,
                    down: false,
                })
            })
            .collect()
    }
    pub fn abort(&mut self) -> Vec<Effect> {
        self.closed = true;
        self.active = None;
        self.releases()
    }
}
