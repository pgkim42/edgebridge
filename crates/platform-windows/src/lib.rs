//! Windows backend (M2, capture only). See docs/PLATFORM-WINDOWS.md.
//!
//! * [`model`], [`normalize`], [`queue`] and [`histogram`] are portable and unit-tested on any host.
//! * On Windows, [`CaptureSession`] runs a dedicated control thread with a hidden window,
//!   `WH_KEYBOARD_LL`/`WH_MOUSE_LL` hooks and Raw Input mouse motion, and hands normalized
//!   [`CaptureEvent`]s to the caller through a bounded lock-free queue.
//!
//! Capture never suppresses local input, opens network connections or listens on a port.

pub mod histogram;
pub mod model;
pub mod normalize;
pub mod queue;

#[cfg(windows)]
mod win32;

pub use model::{
    CaptureEvent, CaptureEventKind, CaptureFault, CaptureRecvError, CaptureStats, DisplayConfig,
    DisplayReason, DpiAwareness, DurationSummary, KeyEvent, KeyState, Modifier, MonitorInfo,
    MouseButton, MouseButtonEvent, MouseWheelEvent, RawDeviceChange, RawMotion, RawMouseMotion,
    WheelAxis,
};
#[cfg(windows)]
pub use win32::{CaptureSession, CaptureStop, QUEUE_CAPACITY};
