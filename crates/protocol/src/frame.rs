//! frame codec. PROTOCOL.md §2.
//!
//! ```text
//! u32 length BE | magic "EDGB" | major u16 | minor u16 | kind u16 | flags u16 | seq u64 | payload
//! ```
//!
//! 오류가 나면 연결을 버린다는 전제로, decoder는 첫 오류 이후 poisoned 상태가 된다.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::limits::{FRAME_LEN_MAX, FRAME_LEN_MIN, HEADER_LEN, LENGTH_PREFIX_LEN};
use crate::message::{Kind, Message, RESERVED_KINDS};
use crate::types::ErrorCode;

pub const MAGIC: [u8; 4] = *b"EDGB";
pub const PROTOCOL_VERSION: Version = Version { major: 1, minor: 0 };

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub version: Version,
    pub kind: Kind,
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub header: Header,
    pub message: Message,
}

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("refusing to encode invalid message: {0}")]
    Invalid(&'static str),
    #[error("payload serialization failed: {0}")]
    Payload(postcard::Error),
    #[error("frame length {0} exceeds limit")]
    TooLong(usize),
    #[error("sequence number space exhausted; reconnect")]
    SeqExhausted,
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("bad magic")]
    BadMagic,
    #[error("frame length {0} outside {FRAME_LEN_MIN}..={FRAME_LEN_MAX}")]
    BadLength(u32),
    #[error("unsupported protocol version {0}")]
    Version(Version),
    #[error("flags must be zero, got {0:#06x}")]
    BadFlags(u16),
    #[error("unknown message kind {0}")]
    UnknownKind(u16),
    #[error("reserved message kind {0} is not allowed in v1.0")]
    ReservedKind(u16),
    #[error("sequence mismatch: expected {expected}, got {got}")]
    Seq { expected: u64, got: u64 },
    #[error("payload decode failed: {0}")]
    Payload(postcard::Error),
    #[error("{0} trailing payload bytes")]
    TrailingBytes(usize),
    #[error("invalid value: {0}")]
    Invalid(&'static str),
    #[error("decoder already failed; connection must be closed")]
    Poisoned,
}

impl DecodeError {
    /// 상대에게 best-effort로 보낼 Error code. `None`이면 응답 없이 바로 close한다.
    pub fn wire_code(&self) -> Option<ErrorCode> {
        match self {
            Self::BadMagic | Self::BadLength(_) | Self::Poisoned => None,
            Self::Version(_) => Some(ErrorCode::Version),
            Self::ReservedKind(_) => Some(ErrorCode::Unsupported),
            Self::Invalid(_) => Some(ErrorCode::InvalidValue),
            Self::BadFlags(_)
            | Self::UnknownKind(_)
            | Self::Seq { .. }
            | Self::Payload(_)
            | Self::TrailingBytes(_) => Some(ErrorCode::BadFrame),
        }
    }
}

/// length prefix를 포함한 frame 하나를 만든다. 보내기 전에 message를 검증한다.
pub fn encode_frame(seq: u64, message: &Message) -> Result<Vec<u8>, EncodeError> {
    message.validate().map_err(EncodeError::Invalid)?;

    let mut out = Vec::with_capacity(LENGTH_PREFIX_LEN + HEADER_LEN + 32);
    out.extend_from_slice(&[0; LENGTH_PREFIX_LEN]);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&PROTOCOL_VERSION.major.to_be_bytes());
    out.extend_from_slice(&PROTOCOL_VERSION.minor.to_be_bytes());
    out.extend_from_slice(&message.kind().to_wire().to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&seq.to_be_bytes());
    let mut out = encode_payload(message, out).map_err(EncodeError::Payload)?;

    let len = out.len() - LENGTH_PREFIX_LEN;
    if len > FRAME_LEN_MAX as usize {
        return Err(EncodeError::TooLong(len));
    }
    out[..LENGTH_PREFIX_LEN].copy_from_slice(&(len as u32).to_be_bytes());
    Ok(out)
}

fn encode_payload(message: &Message, out: Vec<u8>) -> postcard::Result<Vec<u8>> {
    fn put<T: Serialize>(value: &T, out: Vec<u8>) -> postcard::Result<Vec<u8>> {
        postcard::to_extend(value, out)
    }
    match message {
        Message::Hello(m) => put(m, out),
        Message::Ready(m) => put(m, out),
        Message::Enter(m) => put(m, out),
        Message::EnterAck(m) => put(m, out),
        Message::Leave(m) => put(m, out),
        Message::LeaveAck(m) => put(m, out),
        Message::Disconnect(m) => put(m, out),
        Message::Ping(m) => put(m, out),
        Message::Pong(m) => put(m, out),
        Message::Error(m) => put(m, out),
        Message::MouseMove(m) => put(m, out),
        Message::Button(m) => put(m, out),
        Message::Key(m) => put(m, out),
        Message::Scroll(m) => put(m, out),
    }
}

/// length prefix를 제외한 frame bytes(header+payload)를 해석한다. seq 연속성은 검사하지 않는다.
pub fn decode_frame(frame: &[u8]) -> Result<Frame, DecodeError> {
    if frame.len() < HEADER_LEN || frame.len() > FRAME_LEN_MAX as usize {
        return Err(DecodeError::BadLength(frame.len() as u32));
    }
    let (header, payload) = frame.split_at(HEADER_LEN);
    if header[0..4] != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let be16 = |i: usize| u16::from_be_bytes([header[i], header[i + 1]]);
    let version = Version {
        major: be16(4),
        minor: be16(6),
    };
    if version != PROTOCOL_VERSION {
        return Err(DecodeError::Version(version));
    }
    let raw_kind = be16(8);
    let flags = be16(10);
    let seq = u64::from_be_bytes(header[12..20].try_into().expect("8-byte slice"));
    if flags != 0 {
        return Err(DecodeError::BadFlags(flags));
    }
    let kind = match Kind::from_wire(raw_kind) {
        Some(kind) => kind,
        None if RESERVED_KINDS.contains(&raw_kind) => {
            return Err(DecodeError::ReservedKind(raw_kind));
        }
        None => return Err(DecodeError::UnknownKind(raw_kind)),
    };

    let message = decode_payload(kind, payload)?;
    message.validate().map_err(DecodeError::Invalid)?;
    Ok(Frame {
        header: Header { version, kind, seq },
        message,
    })
}

fn decode_payload(kind: Kind, payload: &[u8]) -> Result<Message, DecodeError> {
    fn take<T: DeserializeOwned>(payload: &[u8]) -> Result<T, DecodeError> {
        let (value, rest) = postcard::take_from_bytes(payload).map_err(DecodeError::Payload)?;
        if !rest.is_empty() {
            return Err(DecodeError::TrailingBytes(rest.len()));
        }
        Ok(value)
    }
    Ok(match kind {
        Kind::Hello => Message::Hello(take(payload)?),
        Kind::Ready => Message::Ready(take(payload)?),
        Kind::Enter => Message::Enter(take(payload)?),
        Kind::EnterAck => Message::EnterAck(take(payload)?),
        Kind::Leave => Message::Leave(take(payload)?),
        Kind::LeaveAck => Message::LeaveAck(take(payload)?),
        Kind::Disconnect => Message::Disconnect(take(payload)?),
        Kind::Ping => Message::Ping(take(payload)?),
        Kind::Pong => Message::Pong(take(payload)?),
        Kind::Error => Message::Error(take(payload)?),
        Kind::MouseMove => Message::MouseMove(take(payload)?),
        Kind::Button => Message::Button(take(payload)?),
        Kind::Key => Message::Key(take(payload)?),
        Kind::Scroll => Message::Scroll(take(payload)?),
    })
}

/// 송신 방향 seq 관리. 연결마다 새로 만든다 (첫 frame = 1).
#[derive(Debug)]
pub struct FrameEncoder {
    next_seq: u64,
}

impl Default for FrameEncoder {
    fn default() -> Self {
        Self { next_seq: 1 }
    }
}

impl FrameEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn encode(&mut self, message: &Message) -> Result<Vec<u8>, EncodeError> {
        if self.next_seq == u64::MAX {
            return Err(EncodeError::SeqExhausted);
        }
        let bytes = encode_frame(self.next_seq, message)?;
        self.next_seq += 1;
        Ok(bytes)
    }
}

/// 수신 stream 재조립 + seq 검증. TCP read 결과를 `push`하고 `next_frame`이 `None`일 때까지 반복한다.
///
/// 길이 prefix는 4 bytes가 모이는 즉시 검증하므로 큰 길이를 가장한 입력은 할당 전에 거부된다.
#[derive(Debug)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    expected_seq: u64,
    poisoned: bool,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self {
            buf: Vec::new(),
            expected_seq: 1,
            poisoned: false,
        }
    }
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// 아직 완성되지 않은 frame bytes가 남아 있는지. frame 조립 timeout(1000ms) 판단용.
    pub fn has_partial(&self) -> bool {
        !self.buf.is_empty()
    }

    pub fn next_frame(&mut self) -> Result<Option<Frame>, DecodeError> {
        if self.poisoned {
            return Err(DecodeError::Poisoned);
        }
        let result = self.try_next();
        if result.is_err() {
            self.poisoned = true;
            self.buf.clear();
        }
        result
    }

    fn try_next(&mut self) -> Result<Option<Frame>, DecodeError> {
        if self.buf.len() < LENGTH_PREFIX_LEN {
            return Ok(None);
        }
        let len = u32::from_be_bytes(self.buf[..LENGTH_PREFIX_LEN].try_into().expect("4 bytes"));
        if !(FRAME_LEN_MIN..=FRAME_LEN_MAX).contains(&len) {
            return Err(DecodeError::BadLength(len));
        }
        // magic도 frame 전체를 기다리지 않고 도착하는 즉시 검사한다.
        let magic_end = LENGTH_PREFIX_LEN + MAGIC.len();
        if self.buf.len() >= magic_end && self.buf[LENGTH_PREFIX_LEN..magic_end] != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        let total = LENGTH_PREFIX_LEN + len as usize;
        if self.buf.len() < total {
            return Ok(None);
        }
        let frame = decode_frame(&self.buf[LENGTH_PREFIX_LEN..total])?;
        if frame.header.seq != self.expected_seq {
            return Err(DecodeError::Seq {
                expected: self.expected_seq,
                got: frame.header.seq,
            });
        }
        self.expected_seq += 1;
        self.buf.drain(..total);
        Ok(Some(frame))
    }
}
