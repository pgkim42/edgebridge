//! Windows backend. PLATFORM-WINDOWS.md.
//!
//! M2: 전용 control thread의 `WH_KEYBOARD_LL`/`WH_MOUSE_LL` + Raw Input(`WM_INPUT`) capture,
//! suppression latch, lease 직접 검사. 지금은 빈 자리만 잡아 둔다.
#![cfg(windows)]

/// backend가 아직 없음을 알리는 자리표시자.
pub const IMPLEMENTED: bool = false;
