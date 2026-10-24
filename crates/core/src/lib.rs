//! EdgeBridge 순수 로직. FFI·socket·async runtime을 호출하지 않는다.
//!
//! 시간은 호출자가 [`Millis`]로 넘긴다. 테스트는 fake clock 값을 그대로 넣으면 된다.

pub mod config;
pub mod epoch;
pub mod handshake;
pub mod heartbeat;
pub mod hid;
pub mod session;
pub mod time;

pub use time::Millis;
