//! EdgeBridge wire protocol v1.0.
//!
//! 규범 문서는 `docs/PROTOCOL.md`. 이 crate는 OS와 async runtime에 의존하지 않는다.
//! 단일 message 안의 검증만 담당하고, epoch/ledger/화면 범위 같은 상태 검증은 `edgebridge-core`가 한다.

pub mod frame;
pub mod limits;
pub mod message;
pub mod types;

pub use frame::{
    DecodeError, EncodeError, Frame, FrameDecoder, FrameEncoder, Header, MAGIC, PROTOCOL_VERSION,
    Version, decode_frame, encode_frame,
};
pub use message::*;
pub use types::*;
