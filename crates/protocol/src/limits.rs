//! PROTOCOL.md에 고정된 수치 한계. 변경하면 문서와 golden fixture를 함께 갱신한다.

/// length prefix(u32 BE) 크기. length 값 자체에는 포함되지 않는다.
pub const LENGTH_PREFIX_LEN: usize = 4;
/// magic + major + minor + kind + flags + seq.
pub const HEADER_LEN: usize = 20;
/// length prefix가 가리키는 header+payload 최소/최대 길이.
pub const FRAME_LEN_MIN: u32 = HEADER_LEN as u32;
pub const FRAME_LEN_MAX: u32 = 65_536;

pub const NAME_BYTES_MIN: usize = 1;
pub const NAME_BYTES_MAX: usize = 64;
pub const ERROR_DETAIL_BYTES_MAX: usize = 256;

pub const SCREEN_LOGICAL_MIN: f64 = 16.0;
pub const SCREEN_LOGICAL_MAX: f64 = 65_536.0;
pub const SCREEN_ORIGIN_ABS_MAX: f64 = 1_000_000.0;
pub const SCREEN_PIXEL_MIN: u32 = 16;
pub const SCREEN_PIXEL_MAX: u32 = 131_072;
pub const SCREEN_SCALE_MIN: f64 = 0.25;
pub const SCREEN_SCALE_MAX: f64 = 8.0;
/// pixel/logical 비율과 보고된 scale의 상대 오차 허용치 (1%).
pub const SCREEN_SCALE_TOLERANCE: f64 = 0.01;

pub const SCROLL_ABS_MAX: f64 = 10_000.0;
