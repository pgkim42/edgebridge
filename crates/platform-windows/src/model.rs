//! Portable capture data model. Everything here is `Copy`/plain data so it can travel through the
//! preallocated SPSC queue without allocation, and compiles on every target for tests.

use std::fmt;
use std::time::Duration;

/// Key edge kind. `Repeat` is a Down for a key this session already saw pressed and not released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Down,
    Repeat,
    Up,
}

/// Modifier identity. Left/right variants stay distinct (taken from the LL `vkCode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    LeftShift,
    RightShift,
    LeftCtrl,
    RightCtrl,
    LeftAlt,
    RightAlt,
    LeftWin,
    RightWin,
}

impl Modifier {
    pub fn from_vk(vk: u16) -> Option<Self> {
        Some(match vk {
            0xA0 => Self::LeftShift,
            0xA1 => Self::RightShift,
            0xA2 => Self::LeftCtrl,
            0xA3 => Self::RightCtrl,
            0xA4 => Self::LeftAlt,
            0xA5 => Self::RightAlt,
            0x5B => Self::LeftWin,
            0x5C => Self::RightWin,
            _ => return None,
        })
    }
}

/// Keyboard edge from `WH_KEYBOARD_LL` (physical only; injected events are filtered out).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub vk_code: u16,
    pub scan_code: u16,
    /// `LLKHF_EXTENDED`.
    pub extended: bool,
    pub state: KeyState,
    /// Set when `vk_code` is one of the eight modifier keys.
    pub modifier: Option<Modifier>,
    /// `KBDLLHOOKSTRUCT.time` (ms tick; diagnostic, wraps).
    pub os_time_ms: u32,
}

/// All five standard mouse buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// Button edge from `WH_MOUSE_LL`. `x`/`y` are the OS cursor position, not a virtual position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseButtonEvent {
    pub button: MouseButton,
    pub pressed: bool,
    pub x: i32,
    pub y: i32,
    pub os_time_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelAxis {
    Vertical,
    Horizontal,
}

/// Wheel notch from `WH_MOUSE_LL`. `delta` is the signed `HIWORD(mouseData)` (multiples of 120 for
/// ordinary wheels; positive = away from user / right).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseWheelEvent {
    pub axis: WheelAxis,
    pub delta: i16,
    pub x: i32,
    pub y: i32,
    pub os_time_ms: u32,
}

/// Raw Input mouse movement. Relative counts and absolute positions are never interchangeable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawMotion {
    /// `MOUSE_MOVE_RELATIVE`: counts since the previous report (`lLastX/lLastY`).
    Relative { dx: i32, dy: i32 },
    /// `MOUSE_MOVE_ABSOLUTE` (tablet/RDP/VM): normalized 0..65535 coordinates, mapped to the
    /// virtual desktop when `virtual_desktop` (`MOUSE_VIRTUAL_DESKTOP`) is set, else to the primary.
    Absolute {
        x: i32,
        y: i32,
        virtual_desktop: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawMouseMotion {
    /// `RAWINPUTHEADER.hDevice` handle value (stable while the device is attached).
    pub device: u64,
    pub motion: RawMotion,
}

/// `WM_INPUT_DEVICE_CHANGE` for the mouse class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawDeviceChange {
    pub device: u64,
    pub arrived: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayReason {
    Startup,
    DisplayChange,
    DpiChanged,
}

/// Virtual desktop summary, followed by `monitors` [`MonitorInfo`] events with the same `epoch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayConfig {
    pub reason: DisplayReason,
    pub epoch: u32,
    pub monitors: u32,
    pub virtual_left: i32,
    pub virtual_top: i32,
    pub virtual_width: i32,
    pub virtual_height: i32,
}

/// Full monitor bounds (not work area) in physical pixels when the process is DPI aware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorInfo {
    pub epoch: u32,
    pub index: u32,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub primary: bool,
    /// Effective DPI, or 0 when `GetDpiForMonitor` failed.
    pub dpi_x: u32,
    pub dpi_y: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureEventKind {
    Key(KeyEvent),
    MouseButton(MouseButtonEvent),
    MouseWheel(MouseWheelEvent),
    RawMouseMotion(RawMouseMotion),
    RawDevice(RawDeviceChange),
    Display(DisplayConfig),
    Monitor(MonitorInfo),
}

/// One captured event. `seq` increases by one per produced event in capture order (the order the
/// control thread observed LL callbacks and `WM_INPUT`), so a gap in `seq` is a lost event.
/// `timestamp_ns` is monotonic nanoseconds since the capture session started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureEvent {
    pub seq: u64,
    pub timestamp_ns: u64,
    pub kind: CaptureEventKind,
}

/// Callback/message duration summary. Values are nanoseconds; percentiles have <=12.5% bucket error.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DurationSummary {
    pub count: u64,
    pub mean_ns: u64,
    pub p50_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
    /// Samples above the 100 us p99 budget and the 1 ms observed-max budget.
    pub over_100us: u64,
    pub over_1ms: u64,
}

/// How the process DPI awareness was established before the first window was created.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DpiAwareness {
    /// Not a Windows session.
    #[default]
    NotApplicable,
    /// `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` succeeded.
    Applied,
    /// The call failed with access denied: awareness was already fixed (manifest or earlier call).
    /// The existing level is not verified to be per-monitor v2.
    AlreadySet,
}

/// Runtime/shutdown problem reported by `CaptureSession::stop`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureFault {
    /// The bounded queue was full. Capture was stopped; events from `first_dropped_seq` were lost.
    QueueOverflow {
        first_dropped_seq: u64,
        dropped: u64,
    },
    /// A callback panicked (caught before FFI); capture was stopped.
    CallbackPanicked,
    /// A callback was re-entered while state was in use and its event was dropped.
    ReentrantCallback { count: u64 },
    /// `GetRawInputData` failed or returned inconsistent data; those reports were skipped.
    RawInputReadFailed { count: u64 },
    /// A raw report did not fit the fixed buffer (not a mouse-sized report); skipped.
    RawInputOversized { count: u64 },
    /// `GetMessageW` returned -1; capture stopped.
    MessageLoopFailed { code: u32 },
    /// Monitor/DPI query failed; diagnostics for that epoch are incomplete.
    DisplayQueryFailed { call: &'static str, code: i32 },
    /// A cleanup call failed.
    TeardownFailed { call: &'static str, code: u32 },
    /// Posting the stop message failed (OS error code when available).
    StopRequestFailed { code: Option<i32> },
    /// Signalling or waiting for the consumer wake event failed; capture stopped.
    WakeFailed,
    /// The control thread panicked outside a callback.
    ControlThreadPanicked,
}

impl fmt::Display for CaptureFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QueueOverflow {
                first_dropped_seq,
                dropped,
            } => write!(
                f,
                "capture queue overflowed; {dropped} event(s) lost starting at seq {first_dropped_seq}; capture stopped"
            ),
            Self::CallbackPanicked => write!(f, "a hook/window callback panicked; capture stopped"),
            Self::ReentrantCallback { count } => {
                write!(f, "{count} callback(s) re-entered and were dropped")
            }
            Self::RawInputReadFailed { count } => {
                write!(f, "{count} raw input report(s) could not be read")
            }
            Self::RawInputOversized { count } => {
                write!(f, "{count} raw input report(s) exceeded the buffer")
            }
            Self::MessageLoopFailed { code } => {
                write!(f, "GetMessageW failed (os error {code}); capture stopped")
            }
            Self::DisplayQueryFailed { call, code } => write!(f, "{call} failed (code {code})"),
            Self::TeardownFailed { call, code } => {
                write!(f, "{call} failed during teardown (os error {code})")
            }
            Self::StopRequestFailed { code: Some(code) } => {
                write!(f, "posting stop message failed (os error {code})")
            }
            Self::StopRequestFailed { code: None } => write!(f, "posting stop message failed"),
            Self::WakeFailed => write!(f, "capture wake event failed; capture stopped"),
            Self::ControlThreadPanicked => write!(f, "the control thread panicked"),
        }
    }
}

/// Final session statistics returned by `CaptureSession::stop`.
///
/// `callback_duration` measures each LL hook callback from entry to after `CallNextHookEx` returns,
/// which is what `LowLevelHooksTimeout` charges (it includes other hooks further down the chain).
/// `raw_input_duration` covers the `WM_INPUT` handler including `DefWindowProc`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaptureStats {
    pub queue_capacity: usize,
    pub events_enqueued: u64,
    /// Events the session consumer received through `recv_timeout`.
    pub events_delivered: u64,
    /// Events still queued when stopped (never delivered).
    pub events_undelivered: u64,
    /// Events lost to overflow (counted after capture was declared failed).
    pub events_dropped: u64,
    pub keyboard_callbacks: u64,
    pub mouse_callbacks: u64,
    pub raw_input_messages: u64,
    pub filtered_injected: u64,
    pub filtered_mouse_move: u64,
    pub filtered_unknown: u64,
    pub filtered_no_raw_motion: u64,
    pub raw_read_failures: u64,
    pub raw_oversized: u64,
    pub reentrant_callbacks: u64,
    pub callback_duration: DurationSummary,
    pub raw_input_duration: DurationSummary,
    pub dpi_awareness: DpiAwareness,
    /// Wall time from session start until the control thread finished.
    pub elapsed: Duration,
    pub errors: Vec<CaptureFault>,
}

impl CaptureStats {
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Why `CaptureSession::recv_timeout` returned without an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureRecvError {
    /// No event arrived within the timeout; capture is still running.
    Timeout,
    /// Capture ended (stop request, overflow, or failure) and the queue is drained. Call `stop()`
    /// for the final statistics and any errors.
    Stopped,
}

impl fmt::Display for CaptureRecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => write!(f, "timed out waiting for a capture event"),
            Self::Stopped => write!(f, "capture has stopped"),
        }
    }
}

impl std::error::Error for CaptureRecvError {}
