//! message registry. PROTOCOL.md §4.
//!
//! payload struct의 필드 선언 순서가 곧 wire 순서다. 순서를 바꾸면 protocol 버전을 올린다.

use serde::{Deserialize, Serialize};

use crate::limits;
use crate::types::{
    ButtonAction, Capabilities, Edge, ErrorCode, HidUsage, KeyAction, MouseButton, Position,
    Reason, Role, ScreenInfo, ScrollUnit,
};

/// header의 `kind` 값. 수동 지정하며 enum 순번과 무관하다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum Kind {
    Hello = 1,
    Ready = 2,
    Enter = 3,
    EnterAck = 4,
    Leave = 5,
    LeaveAck = 6,
    Disconnect = 7,
    Ping = 8,
    Pong = 9,
    Error = 10,
    MouseMove = 16,
    Button = 17,
    Key = 18,
    Scroll = 19,
}

/// clipboard 계열로 예약된 kind 범위. v1.0에서 송수신 금지.
pub const RESERVED_KINDS: std::ops::RangeInclusive<u16> = 32..=35;

impl Kind {
    pub const fn to_wire(self) -> u16 {
        self as u16
    }

    pub const fn from_wire(value: u16) -> Option<Self> {
        Some(match value {
            1 => Self::Hello,
            2 => Self::Ready,
            3 => Self::Enter,
            4 => Self::EnterAck,
            5 => Self::Leave,
            6 => Self::LeaveAck,
            7 => Self::Disconnect,
            8 => Self::Ping,
            9 => Self::Pong,
            10 => Self::Error,
            16 => Self::MouseMove,
            17 => Self::Button,
            18 => Self::Key,
            19 => Self::Scroll,
            _ => return None,
        })
    }

    /// Ready 이전에 허용되는 kind (Hello/Ready/Error/Disconnect).
    pub const fn allowed_before_ready(self) -> bool {
        matches!(
            self,
            Self::Hello | Self::Ready | Self::Error | Self::Disconnect
        )
    }

    /// active epoch에서만 의미가 있는 입력 kind.
    pub const fn is_input(self) -> bool {
        matches!(
            self,
            Self::MouseMove | Self::Button | Self::Key | Self::Scroll
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub role: Role,
    pub name: String,
    pub capabilities: Capabilities,
    pub screens: Vec<ScreenInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ready {
    pub session_id: [u8; 16],
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Enter {
    pub epoch: u64,
    pub screen_id: u32,
    pub edge: Edge,
    pub position: Position,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnterAck {
    pub epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Leave {
    pub epoch: u64,
    pub reason: Reason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaveAck {
    pub epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Disconnect {
    pub reason: Reason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ping {
    pub token: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pong {
    pub token: u64,
}

/// `Error` message payload. std `Error`와 이름이 겹치지 않도록 접미사를 붙였다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorMessage {
    pub code: ErrorCode,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MouseMove {
    pub epoch: u64,
    pub position: Position,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Button {
    pub epoch: u64,
    pub button: MouseButton,
    pub action: ButtonAction,
    pub position: Position,
    /// server monotonic ms. click-count 판단에만 사용한다.
    pub source_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    pub epoch: u64,
    pub hid: HidUsage,
    pub action: KeyAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Scroll {
    pub epoch: u64,
    pub unit: ScrollUnit,
    /// +dx: 내용이 오른쪽으로 이동.
    pub dx: f64,
    /// +dy: 내용이 위쪽으로 이동.
    pub dy: f64,
    pub position: Position,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Hello(Hello),
    Ready(Ready),
    Enter(Enter),
    EnterAck(EnterAck),
    Leave(Leave),
    LeaveAck(LeaveAck),
    Disconnect(Disconnect),
    Ping(Ping),
    Pong(Pong),
    Error(ErrorMessage),
    MouseMove(MouseMove),
    Button(Button),
    Key(Key),
    Scroll(Scroll),
}

impl Message {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Hello(_) => Kind::Hello,
            Self::Ready(_) => Kind::Ready,
            Self::Enter(_) => Kind::Enter,
            Self::EnterAck(_) => Kind::EnterAck,
            Self::Leave(_) => Kind::Leave,
            Self::LeaveAck(_) => Kind::LeaveAck,
            Self::Disconnect(_) => Kind::Disconnect,
            Self::Ping(_) => Kind::Ping,
            Self::Pong(_) => Kind::Pong,
            Self::Error(_) => Kind::Error,
            Self::MouseMove(_) => Kind::MouseMove,
            Self::Button(_) => Kind::Button,
            Self::Key(_) => Kind::Key,
            Self::Scroll(_) => Kind::Scroll,
        }
    }

    /// input message가 속한 epoch.
    pub fn input_epoch(&self) -> Option<u64> {
        match self {
            Self::MouseMove(m) => Some(m.epoch),
            Self::Button(m) => Some(m.epoch),
            Self::Key(m) => Some(m.epoch),
            Self::Scroll(m) => Some(m.epoch),
            _ => None,
        }
    }

    /// 단일 message 안에서 판단 가능한 의미 검증. 상태(epoch/ledger/화면 범위) 검증은 core 담당.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Hello(m) => {
                validate_text(&m.name, limits::NAME_BYTES_MIN, limits::NAME_BYTES_MAX)
                    .map_err(|_| "hello name must be 1..=64 bytes without control characters")?;
                validate_caps(m.capabilities)?;
                match (m.role, m.screens.len()) {
                    (Role::Server, 0) | (Role::Client, 1) => {}
                    (Role::Server, _) => return Err("server hello must not list screens"),
                    (Role::Client, _) => return Err("client hello must list exactly one screen"),
                }
                m.screens.iter().try_for_each(ScreenInfo::validate)
            }
            Self::Ready(m) => validate_caps(m.capabilities),
            Self::Enter(m) => {
                validate_epoch(m.epoch)?;
                m.position.validate()
            }
            Self::EnterAck(EnterAck { epoch })
            | Self::Leave(Leave { epoch, .. })
            | Self::LeaveAck(LeaveAck { epoch }) => validate_epoch(*epoch),
            Self::Disconnect(_) | Self::Ping(_) | Self::Pong(_) => Ok(()),
            Self::Error(m) => validate_text(&m.detail, 0, limits::ERROR_DETAIL_BYTES_MAX)
                .map_err(|_| "error detail must be <=256 bytes without control characters"),
            Self::MouseMove(m) => {
                validate_epoch(m.epoch)?;
                m.position.validate()
            }
            Self::Button(m) => {
                validate_epoch(m.epoch)?;
                m.position.validate()
            }
            Self::Key(m) => validate_epoch(m.epoch),
            Self::Scroll(m) => {
                validate_epoch(m.epoch)?;
                for v in [m.dx, m.dy] {
                    if !v.is_finite() || v.abs() > limits::SCROLL_ABS_MAX {
                        return Err("scroll delta must be finite and |v| <= 10000");
                    }
                }
                m.position.validate()
            }
        }
    }
}

fn validate_epoch(epoch: u64) -> Result<(), &'static str> {
    if epoch == 0 {
        return Err("epoch starts at 1");
    }
    Ok(())
}

fn validate_caps(caps: Capabilities) -> Result<(), &'static str> {
    if caps.has_reserved_bits() {
        return Err("reserved capability bits must be zero");
    }
    Ok(())
}

fn validate_text(s: &str, min: usize, max: usize) -> Result<(), ()> {
    if s.len() < min || s.len() > max || s.chars().any(char::is_control) {
        return Err(());
    }
    Ok(())
}
