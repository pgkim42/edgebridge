//! Quartz 전용 backend. OS 객체는 생성한 main actor 안에서만 사용한다.
#![cfg(target_os = "macos")]
use core_graphics::{
    display::CGDisplay,
    event::{CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField},
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::CGPoint,
};
use edgebridge_core::{config::ClientScreenSelector, session::Inject};
use edgebridge_protocol::{MouseButton, ScreenInfo};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightPostEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
}

#[derive(Debug, thiserror::Error)]
#[error("macOS backend: {0}")]
pub struct BackendError(pub &'static str);

pub fn permission(prompt: bool) -> bool {
    // 인자/소유 포인터가 없는 OS 조회. 요청은 명시적 flag에서만 실행한다.
    unsafe {
        if prompt {
            CGRequestPostEventAccess();
        }
        AXIsProcessTrusted() && CGPreflightPostEventAccess()
    }
}
pub fn screen(selector: ClientScreenSelector) -> Result<ScreenInfo, BackendError> {
    let ids =
        CGDisplay::active_displays().map_err(|_| BackendError("display enumeration failed"))?;
    if ids.len() != 1 {
        return Err(BackendError("M1 requires one active built-in display"));
    }
    let display = match selector {
        ClientScreenSelector::Main => CGDisplay::main(),
        ClientScreenSelector::Builtin => CGDisplay::new(ids[0]),
    };
    if !display.is_builtin()
        || !display.is_active()
        || display.is_asleep()
        || display.is_in_mirror_set()
        || display.rotation() != 0.0
    {
        return Err(BackendError("unsupported display configuration"));
    }
    let bounds = display.bounds();
    let mode = display
        .display_mode()
        .ok_or(BackendError("no display mode"))?;
    let pixel_width =
        u32::try_from(mode.pixel_width()).map_err(|_| BackendError("pixel width overflow"))?;
    let pixel_height =
        u32::try_from(mode.pixel_height()).map_err(|_| BackendError("pixel height overflow"))?;
    let s = ScreenInfo {
        id: display.id,
        origin_x: bounds.origin.x,
        origin_y: bounds.origin.y,
        width: bounds.size.width,
        height: bounds.size.height,
        pixel_width,
        pixel_height,
        scale_x: f64::from(pixel_width) / bounds.size.width,
        scale_y: f64::from(pixel_height) / bounds.size.height,
    };
    s.validate().map_err(BackendError)?;
    Ok(s)
}
pub struct Quartz {
    source: CGEventSource,
}
impl Quartz {
    pub fn new() -> Result<Self, BackendError> {
        Ok(Self {
            source: CGEventSource::new(CGEventSourceStateID::Private)
                .map_err(|_| BackendError("event source allocation failed"))?,
        })
    }
    pub fn inject(&mut self, command: Inject) -> Result<(), BackendError> {
        use CGEventType::*;
        let (position, button, kind, click) = match command {
            Inject::Move { position, drag } => (
                position,
                drag.unwrap_or(MouseButton::Left),
                match drag {
                    None => MouseMoved,
                    Some(MouseButton::Left) => LeftMouseDragged,
                    Some(MouseButton::Right) => RightMouseDragged,
                    _ => OtherMouseDragged,
                },
                0,
            ),
            Inject::Button {
                position,
                button,
                down,
            } => (
                position,
                button,
                match (button, down) {
                    (MouseButton::Left, true) => LeftMouseDown,
                    (MouseButton::Left, false) => LeftMouseUp,
                    (MouseButton::Right, true) => RightMouseDown,
                    (MouseButton::Right, false) => RightMouseUp,
                    (_, true) => OtherMouseDown,
                    (_, false) => OtherMouseUp,
                },
                1,
            ),
        };
        if !position.x.is_finite() || !position.y.is_finite() {
            return Err(BackendError("non-finite position"));
        }
        let quartz_button = match button {
            MouseButton::Left => CGMouseButton::Left,
            MouseButton::Right => CGMouseButton::Right,
            _ => CGMouseButton::Center,
        };
        let event = CGEvent::new_mouse_event(
            self.source.clone(),
            kind,
            CGPoint::new(position.x, position.y),
            quartz_button,
        )
        .map_err(|_| BackendError("event allocation failed"))?;
        event.set_integer_value_field(
            EventField::MOUSE_EVENT_BUTTON_NUMBER,
            i64::from(button.to_wire()),
        );
        event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, click);
        event.set_flags(CGEventFlags::empty());
        event.post(CGEventTapLocation::HID);
        Ok(())
    }
    /// 수동 진단 전용 readback. event tap/listen 권한을 요청하지 않는다.
    pub fn cursor_position(&self) -> Result<edgebridge_protocol::Position, BackendError> {
        let event = CGEvent::new(self.source.clone())
            .map_err(|_| BackendError("cursor readback failed"))?;
        let point = event.location();
        Ok(edgebridge_protocol::Position {
            x: point.x,
            y: point.y,
        })
    }
    pub fn idle() {
        // main run loop를 짧게 pump한다. AppKit 객체/알림은 M6에서 추가한다.
        core_foundation::runloop::CFRunLoop::run_in_mode(
            unsafe { core_foundation::runloop::kCFRunLoopDefaultMode },
            std::time::Duration::from_millis(5),
            true,
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
