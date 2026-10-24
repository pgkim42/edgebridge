//! macOS backend. PLATFORM-MACOS.md.
//!
//! M1: main run loop 위 단일 injection actor, `CGEventPost` 주입, Accessibility(TCC) 확인,
//! `CGDisplayBounds` 기반 화면 정보. 지금은 빈 자리만 잡아 둔다.
#![cfg(target_os = "macos")]

/// backend가 아직 없음을 알리는 자리표시자.
pub const IMPLEMENTED: bool = false;
