//! Win32 capture backend (capture only: nothing is ever suppressed, no sockets are opened).
//!
//! Thread model: one dedicated control thread owns the hidden window, both low-level hooks and the
//! Raw Input registration, and runs the `GetMessageW` loop. LL hook callbacks and `WM_INPUT` both run
//! on that thread, so there is a single producer and the sequence number is the capture order. The
//! caller thread is the single consumer of a bounded lock-free queue.
//!
//! Callback rules (`keyboard_proc`, `mouse_proc`, `on_raw_input`): no allocation, formatting,
//! logging, locks or IO. The callback copies the OS struct into a small record, normalizes it,
//! try-pushes to the preallocated queue and, only when the consumer is parked, signals an
//! auto-reset event. LL hooks always forward through `CallNextHookEx` (including `nCode < 0` and
//! injected input) and never return nonzero. Queue overflow stops capture and is reported by
//! `CaptureSession::stop`; it is never silent.
//!
//! Limits (not verifiable here, see docs/PLATFORM-WINDOWS.md): Windows silently removes an LL hook
//! whose callback exceeds `LowLevelHooksTimeout`; this cannot be detected, only observed through the
//! duration statistics. Raw Input registration is per process: one capture session per process, and
//! this crate must not be combined with other raw mouse window registrations in the same process.
//! Keys already held when capture starts have unknown state (see `normalize`).

use std::cell::Cell;
use std::io;
use std::mem::{align_of, offset_of, size_of, zeroed};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{self, null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering, fence};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, GetLastError, HANDLE, HMODULE, HWND, LPARAM, LRESULT, RECT,
    WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, SetEvent, WaitForSingleObject,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RAWMOUSE, RID_INPUT,
    RIDEV_DEVNOTIFY, RIDEV_INPUTSINK, RIDEV_REMOVE, RIM_TYPEMOUSE, RegisterRawInputDevices,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GIDC_ARRIVAL,
    GetMessageW, GetSystemMetrics, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, MONITORINFOF_PRIMARY, MSG,
    MSLLHOOKSTRUCT, PostMessageW, RegisterClassExW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SetWindowsHookExW, UnhookWindowsHookEx, UnregisterClassW,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_INPUT,
    WM_INPUT_DEVICE_CHANGE, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::histogram::DurationHistogram;
use crate::model::{
    CaptureEvent, CaptureEventKind, CaptureFault, CaptureRecvError, CaptureStats, DisplayConfig,
    DisplayReason, DpiAwareness, MonitorInfo, RawDeviceChange,
};
use crate::normalize::{
    FilterReason, KeyboardRecord, MouseRecord, Normalizer, Outcome, RawMouseRecord,
};
use crate::queue::{self, Consumer, Producer};

/// Events the capture queue can hold before overflow stops capture (rounded to a power of two).
pub const QUEUE_CAPACITY: usize = 2048;

const WM_STOP: u32 = WM_APP + 1;
const CLASS_NAME: &str = "EdgeBridgeCaptureWindow\0";

const RAW_BUF_WORDS: usize = 32;
const RAW_HEADER_SIZE: u32 = size_of::<RAWINPUTHEADER>() as u32;
const RAW_DATA_OFFSET: usize = offset_of!(RAWINPUT, data);
const _: () = {
    // The u64 buffer is at least as aligned as RAWINPUT (>= DWORD as documented), and the mouse
    // payload sits at an offset aligned for RAWMOUSE, so typed reads below are aligned.
    assert!(align_of::<RAWINPUT>() <= align_of::<u64>());
    assert!(RAW_DATA_OFFSET.is_multiple_of(align_of::<RAWMOUSE>()));
    assert!(RAW_DATA_OFFSET + size_of::<RAWMOUSE>() <= RAW_BUF_WORDS * size_of::<u64>());
    assert!(align_of::<RAWINPUTHEADER>() <= align_of::<u64>());
};

/// Callbacks dropped because they re-entered while capture state was in use.
static REENTRANT_DROPS: AtomicU64 = AtomicU64::new(0);
/// One capture session per process (hooks and the raw-input window registration are process-wide).
static ACTIVE: AtomicBool = AtomicBool::new(false);

thread_local! {
    static STATE: Cell<*mut CallbackState> = const { Cell::new(null_mut()) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

// ---------------------------------------------------------------------------------------------
// Shared cross-thread state
// ---------------------------------------------------------------------------------------------

struct EventHandle(HANDLE);

// SAFETY: a Win32 event handle may be signalled and waited on from any thread.
unsafe impl Send for EventHandle {}
unsafe impl Sync for EventHandle {}

impl EventHandle {
    /// Auto-reset, initially non-signalled.
    fn new() -> io::Result<Self> {
        // SAFETY: null attributes/name are valid arguments.
        let handle = unsafe { CreateEventW(null(), 0, 0, null()) };
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }

    fn set(&self) -> bool {
        // SAFETY: valid event handle owned by `self`.
        unsafe { SetEvent(self.0) != 0 }
    }

    fn wait(&self, millis: u32) -> u32 {
        // SAFETY: valid event handle owned by `self`.
        unsafe { WaitForSingleObject(self.0, millis) }
    }
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        // SAFETY: handle was created by CreateEventW and is closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

struct Shared {
    stop_requested: AtomicBool,
    /// Set (after the last queue push) when the control thread is done; consumer then drains.
    finished: AtomicBool,
    /// Consumer is parked on `wake` (or about to be).
    waiting: AtomicBool,
    wake_failed: AtomicBool,
    /// Hidden window handle while it is alive. Cleared under the lock before `DestroyWindow`, so a
    /// stop request can never post to a destroyed or recycled handle.
    hwnd: Mutex<Option<usize>>,
    wake: EventHandle,
}

impl Shared {
    /// Called by the producer after a push. Wait-free when the consumer is not parked.
    fn wake_consumer(&self) -> bool {
        fence(Ordering::SeqCst);
        !self.waiting.swap(false, Ordering::SeqCst) || self.signal()
    }

    fn signal(&self) -> bool {
        let ok = self.wake.set();
        if !ok {
            self.wake_failed.store(true, Ordering::Release);
        }
        ok
    }
}

struct ActiveGuard;

impl ActiveGuard {
    fn acquire() -> io::Result<Self> {
        ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "a capture session is already active in this process",
                )
            })
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}

struct FinishGuard(Arc<Shared>);

impl Drop for FinishGuard {
    fn drop(&mut self) {
        self.0.finished.store(true, Ordering::Release);
        self.0.signal();
    }
}

// ---------------------------------------------------------------------------------------------
// Callback state (control thread only; reached through thread-local pointer)
// ---------------------------------------------------------------------------------------------

struct CallbackState {
    normalizer: Normalizer,
    producer: Producer<CaptureEvent>,
    shared: Arc<Shared>,
    base: Instant,
    hwnd: HWND,
    stats: CaptureStats,
    callback_hist: DurationHistogram,
    raw_hist: DurationHistogram,
    /// First lost sequence number once the queue overflowed.
    overflow_first_seq: Option<u64>,
    panicked: bool,
    display_epoch: u32,
    faults: Vec<CaptureFault>,
}

enum RawRead {
    Record(RawMouseRecord),
    NotMouse,
    Failed,
    Oversized,
}

struct DisplaySnapshot {
    virtual_left: i32,
    virtual_top: i32,
    virtual_width: i32,
    virtual_height: i32,
    monitors: Vec<MonitorInfo>,
    errors: Vec<(&'static str, i32)>,
}

impl CallbackState {
    fn timestamp_ns(&self, at: Instant) -> u64 {
        u64::try_from(at.saturating_duration_since(self.base).as_nanos()).unwrap_or(u64::MAX)
    }

    fn request_stop_from_thread(&self) {
        self.shared.stop_requested.store(true, Ordering::Release);
        if !self.hwnd.is_null() {
            // SAFETY: our own live window (cleared from state before it is destroyed). If the post
            // fails the loop still observes `stop_requested` after the next message it handles.
            unsafe { PostMessageW(self.hwnd, WM_STOP, 0, 0) };
        }
    }

    fn enqueue(&mut self, event: CaptureEvent) {
        if self.overflow_first_seq.is_some() {
            self.stats.events_dropped += 1;
            return;
        }
        match self.producer.push(event) {
            Ok(()) => {
                self.stats.events_enqueued += 1;
            }
            Err(lost) => {
                self.overflow_first_seq = Some(lost.seq);
                self.stats.events_dropped += 1;
                self.request_stop_from_thread();
            }
        }
        if !self.shared.wake_consumer() {
            self.request_stop_from_thread();
        }
    }

    fn submit(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Event(event) => self.enqueue(event),
            Outcome::Filtered(FilterReason::Injected) => self.stats.filtered_injected += 1,
            Outcome::Filtered(FilterReason::MouseMove) => self.stats.filtered_mouse_move += 1,
            Outcome::Filtered(FilterReason::UnknownMessage) => self.stats.filtered_unknown += 1,
            Outcome::Filtered(FilterReason::NoRawMotion) => self.stats.filtered_no_raw_motion += 1,
        }
    }

    fn handle_keyboard(&mut self, record: KeyboardRecord, at: Instant) {
        self.stats.keyboard_callbacks += 1;
        if self.panicked {
            return;
        }
        let outcome = self.normalizer.keyboard(&record, self.timestamp_ns(at));
        self.submit(outcome);
    }

    fn handle_mouse(&mut self, record: MouseRecord, at: Instant) {
        self.stats.mouse_callbacks += 1;
        if self.panicked {
            return;
        }
        let outcome = self.normalizer.mouse(&record, self.timestamp_ns(at));
        self.submit(outcome);
    }

    fn handle_raw(&mut self, read: RawRead, at: Instant) {
        self.stats.raw_input_messages += 1;
        if self.panicked {
            return;
        }
        match read {
            RawRead::Record(record) => {
                let outcome = self.normalizer.raw_mouse(&record, self.timestamp_ns(at));
                self.submit(outcome);
            }
            RawRead::NotMouse => self.stats.filtered_unknown += 1,
            RawRead::Failed => self.stats.raw_read_failures += 1,
            RawRead::Oversized => self.stats.raw_oversized += 1,
        }
    }

    fn handle_device_change(&mut self, device: u64, arrived: bool, at: Instant) {
        if self.panicked {
            return;
        }
        let kind = CaptureEventKind::RawDevice(RawDeviceChange { device, arrived });
        let event = self.normalizer.stamp(kind, self.timestamp_ns(at));
        self.enqueue(event);
    }

    fn handle_displays(&mut self, reason: DisplayReason, snapshot: DisplaySnapshot, at: Instant) {
        if self.panicked {
            return;
        }
        self.display_epoch += 1;
        let epoch = self.display_epoch;
        let timestamp_ns = self.timestamp_ns(at);
        self.faults.extend(
            snapshot
                .errors
                .into_iter()
                .map(|(call, code)| CaptureFault::DisplayQueryFailed { call, code }),
        );
        let config = DisplayConfig {
            reason,
            epoch,
            monitors: snapshot.monitors.len() as u32,
            virtual_left: snapshot.virtual_left,
            virtual_top: snapshot.virtual_top,
            virtual_width: snapshot.virtual_width,
            virtual_height: snapshot.virtual_height,
        };
        let event = self
            .normalizer
            .stamp(CaptureEventKind::Display(config), timestamp_ns);
        self.enqueue(event);
        for mut monitor in snapshot.monitors {
            monitor.epoch = epoch;
            let event = self
                .normalizer
                .stamp(CaptureEventKind::Monitor(monitor), timestamp_ns);
            self.enqueue(event);
        }
    }

    fn into_stats(self: Box<Self>, teardown_faults: Vec<CaptureFault>) -> CaptureStats {
        let Self {
            callback_hist,
            raw_hist,
            overflow_first_seq,
            panicked,
            faults,
            mut stats,
            ..
        } = *self;
        let mut errors = faults;
        stats.callback_duration = callback_hist.summary();
        stats.raw_input_duration = raw_hist.summary();
        stats.reentrant_callbacks = REENTRANT_DROPS.swap(0, Ordering::Relaxed);
        if let Some(first_dropped_seq) = overflow_first_seq {
            errors.push(CaptureFault::QueueOverflow {
                first_dropped_seq,
                dropped: stats.events_dropped,
            });
        }
        if panicked {
            errors.push(CaptureFault::CallbackPanicked);
        }
        if stats.reentrant_callbacks > 0 {
            errors.push(CaptureFault::ReentrantCallback {
                count: stats.reentrant_callbacks,
            });
        }
        if stats.raw_read_failures > 0 {
            errors.push(CaptureFault::RawInputReadFailed {
                count: stats.raw_read_failures,
            });
        }
        if stats.raw_oversized > 0 {
            errors.push(CaptureFault::RawInputOversized {
                count: stats.raw_oversized,
            });
        }
        errors.extend(teardown_faults);
        stats.errors = errors;
        stats
    }
}

/// Runs `f` on the thread's capture state. Returns `None` (counting a re-entrancy drop when it was
/// busy) instead of aliasing the state. `f` must not call Win32 functions that can dispatch hooks.
fn with_state<R>(f: impl FnOnce(&mut CallbackState) -> R) -> Option<R> {
    let ptr = STATE.with(Cell::get);
    if ptr.is_null() {
        return None;
    }
    if BUSY.with(|busy| busy.replace(true)) {
        REENTRANT_DROPS.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            BUSY.with(|busy| busy.set(false));
        }
    }
    let _release = Release;
    // SAFETY: STATE is set by `StateOwner` to a live Box for the lifetime of the control thread's
    // capture, and BUSY guarantees this is the only live reference on this thread.
    Some(f(unsafe { &mut *ptr }))
}

/// Never lets a panic unwind into Windows; a panic marks capture failed and requests stop.
fn guard(f: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(f)).is_err() {
        with_state(|state| {
            state.panicked = true;
            state.request_stop_from_thread();
        });
    }
}

fn elapsed_ns(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

// ---------------------------------------------------------------------------------------------
// Callbacks
// ---------------------------------------------------------------------------------------------

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let capture = code == HC_ACTION as i32 && lparam != 0;
    let start = capture.then(Instant::now);
    if let Some(start) = start {
        guard(|| {
            // SAFETY: for HC_ACTION `lparam` points to a KBDLLHOOKSTRUCT valid for this call.
            let kb = unsafe { ptr::read(lparam as *const KBDLLHOOKSTRUCT) };
            let record = KeyboardRecord {
                message: wparam as u32,
                vk_code: kb.vkCode,
                scan_code: kb.scanCode,
                flags: kb.flags,
                time: kb.time,
            };
            with_state(|state| state.handle_keyboard(record, start));
        });
    }
    // No state borrow is held here. Always forward (also nCode < 0 and injected input).
    // SAFETY: arguments are forwarded unchanged; the hook handle is ignored by CallNextHookEx.
    let result = unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) };
    if let Some(start) = start {
        guard(|| {
            with_state(|state| state.callback_hist.record(elapsed_ns(start)));
        });
    }
    result
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let capture = code == HC_ACTION as i32 && lparam != 0;
    let start = capture.then(Instant::now);
    if let Some(start) = start {
        guard(|| {
            // SAFETY: for HC_ACTION `lparam` points to an MSLLHOOKSTRUCT valid for this call.
            let ms = unsafe { ptr::read(lparam as *const MSLLHOOKSTRUCT) };
            let record = MouseRecord {
                message: wparam as u32,
                x: ms.pt.x,
                y: ms.pt.y,
                mouse_data: ms.mouseData,
                flags: ms.flags,
                time: ms.time,
            };
            with_state(|state| state.handle_mouse(record, start));
        });
    }
    // SAFETY: see keyboard_proc.
    let result = unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) };
    if let Some(start) = start {
        guard(|| {
            with_state(|state| state.callback_hist.record(elapsed_ns(start)));
        });
    }
    result
}

/// Reads one `WM_INPUT` packet into a fixed, 8-byte aligned stack buffer with the documented
/// size-query-then-read sequence. Never reads past the bytes the OS reports.
fn read_raw_mouse(handle: HRAWINPUT) -> RawRead {
    let mut buf = [0u64; RAW_BUF_WORDS];
    let capacity = size_of_val(&buf);
    let mut size = 0u32;
    // SAFETY: null data pointer asks only for the required size.
    let rc = unsafe { GetRawInputData(handle, RID_INPUT, null_mut(), &mut size, RAW_HEADER_SIZE) };
    if rc != 0 {
        return RawRead::Failed;
    }
    if size as usize > capacity {
        return RawRead::Oversized;
    }
    // SAFETY: `buf` is writable for `capacity >= size` bytes and aligned to at least a DWORD.
    let copied = unsafe {
        GetRawInputData(
            handle,
            RID_INPUT,
            buf.as_mut_ptr().cast(),
            &mut size,
            RAW_HEADER_SIZE,
        )
    };
    let copied = copied as usize;
    if copied == u32::MAX as usize || copied > capacity || copied < size_of::<RAWINPUTHEADER>() {
        return RawRead::Failed;
    }
    let base = buf.as_ptr().cast::<u8>();
    // SAFETY: at least a full header was copied; the buffer is aligned for RAWINPUTHEADER.
    let header = unsafe { ptr::read(base.cast::<RAWINPUTHEADER>()) };
    if header.dwType != RIM_TYPEMOUSE {
        return RawRead::NotMouse;
    }
    if copied < RAW_DATA_OFFSET + size_of::<RAWMOUSE>() {
        return RawRead::Failed;
    }
    // SAFETY: the mouse payload lies fully inside the copied bytes and is aligned (const-asserted).
    let mouse = unsafe { ptr::read(base.add(RAW_DATA_OFFSET).cast::<RAWMOUSE>()) };
    RawRead::Record(RawMouseRecord {
        device: header.hDevice as usize as u64,
        flags: mouse.usFlags,
        last_x: mouse.lLastX,
        last_y: mouse.lLastY,
    })
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_INPUT => {
            let start = Instant::now();
            // The packet must be read before DefWindowProc; DefWindowProc then performs the system
            // cleanup required for foreground (RIM_INPUT) input. Done for sink input as well.
            guard(|| {
                let read = read_raw_mouse(lparam as HRAWINPUT);
                with_state(|state| state.handle_raw(read, start));
            });
            // SAFETY: forwarded unchanged on the window's own thread; no state borrow is held.
            let result = unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            guard(|| {
                with_state(|state| state.raw_hist.record(elapsed_ns(start)));
            });
            result
        }
        WM_INPUT_DEVICE_CHANGE => {
            let at = Instant::now();
            guard(|| {
                let arrived = wparam == GIDC_ARRIVAL as WPARAM;
                with_state(|state| state.handle_device_change(lparam as usize as u64, arrived, at));
            });
            0
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED => {
            let reason = if msg == WM_DISPLAYCHANGE {
                DisplayReason::DisplayChange
            } else {
                DisplayReason::DpiChanged
            };
            guard(|| refresh_displays(reason));
            // SAFETY: forwarded unchanged on the window's own thread.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        // SAFETY: forwarded unchanged on the window's own thread.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

// ---------------------------------------------------------------------------------------------
// Monitor / DPI diagnostics (control thread, outside hooks; allocation is fine here)
// ---------------------------------------------------------------------------------------------

struct MonitorScan {
    monitors: Vec<MonitorInfo>,
    errors: Vec<(&'static str, i32)>,
}

unsafe extern "system" fn monitor_proc(
    monitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `data` is the `&mut MonitorScan` passed by `query_displays` for this enumeration.
        let scan = unsafe { &mut *(data as *mut MonitorScan) };
        // SAFETY: MONITORINFO is plain data; all-zero is a valid initial value.
        let mut info: MONITORINFO = unsafe { zeroed() };
        info.cbSize = size_of::<MONITORINFO>() as u32;
        // SAFETY: valid monitor handle from the enumeration and a correctly sized out struct.
        if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            // SAFETY: trivially safe.
            scan.errors
                .push(("GetMonitorInfoW", unsafe { GetLastError() } as i32));
            return 1;
        }
        let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
        // SAFETY: valid monitor handle and out pointers.
        let hr = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        if hr < 0 {
            scan.errors.push(("GetDpiForMonitor", hr));
            (dpi_x, dpi_y) = (0, 0);
        }
        scan.monitors.push(MonitorInfo {
            epoch: 0,
            index: scan.monitors.len() as u32,
            left: info.rcMonitor.left,
            top: info.rcMonitor.top,
            right: info.rcMonitor.right,
            bottom: info.rcMonitor.bottom,
            primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
            dpi_x,
            dpi_y,
        });
        1
    }))
    .unwrap_or(0)
}

fn query_displays() -> DisplaySnapshot {
    let mut scan = MonitorScan {
        monitors: Vec::new(),
        errors: Vec::new(),
    };
    // SAFETY: `scan` outlives the synchronous enumeration; the callback only runs inside this call.
    let ok = unsafe {
        EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(monitor_proc),
            &mut scan as *mut MonitorScan as LPARAM,
        )
    };
    if ok == 0 {
        // SAFETY: trivially safe.
        scan.errors
            .push(("EnumDisplayMonitors", unsafe { GetLastError() } as i32));
    }
    // SAFETY: GetSystemMetrics has no preconditions.
    let (left, top, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    DisplaySnapshot {
        virtual_left: left,
        virtual_top: top,
        virtual_width: width,
        virtual_height: height,
        monitors: scan.monitors,
        errors: scan.errors,
    }
}

fn refresh_displays(reason: DisplayReason) {
    let at = Instant::now();
    // The Win32 queries run with no state borrow held; only the enqueue borrows state.
    let snapshot = query_displays();
    with_state(|state| state.handle_displays(reason, snapshot, at));
}

// ---------------------------------------------------------------------------------------------
// Control thread
// ---------------------------------------------------------------------------------------------

/// Owns the heap `CallbackState` and the thread-local pointer to it.
struct StateOwner {
    ptr: *mut CallbackState,
}

impl StateOwner {
    fn install(state: Box<CallbackState>) -> Self {
        let ptr = Box::into_raw(state);
        STATE.with(|cell| cell.set(ptr));
        Self { ptr }
    }

    fn release(&mut self) -> Option<Box<CallbackState>> {
        STATE.with(|cell| cell.set(null_mut()));
        let ptr = std::mem::replace(&mut self.ptr, null_mut());
        // SAFETY: `ptr` came from Box::into_raw in `install` and is reclaimed exactly once; the
        // thread-local pointer was cleared above, so no callback can still reach it.
        (!ptr.is_null()).then(|| unsafe { Box::from_raw(ptr) })
    }
}

impl Drop for StateOwner {
    fn drop(&mut self) {
        drop(self.release());
    }
}

/// Win32 objects created by the control thread; `teardown` is idempotent and also runs on drop.
struct Resources {
    shared: Arc<Shared>,
    class_name: Vec<u16>,
    hinstance: HMODULE,
    class_registered: bool,
    hwnd: HWND,
    raw_registered: bool,
    keyboard_hook: HHOOK,
    mouse_hook: HHOOK,
}

impl Resources {
    fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            class_name: CLASS_NAME.encode_utf16().collect(),
            hinstance: null_mut(),
            class_registered: false,
            hwnd: null_mut(),
            raw_registered: false,
            keyboard_hook: null_mut(),
            mouse_hook: null_mut(),
        }
    }

    fn mouse_device(flags: u32, target: HWND) -> RAWINPUTDEVICE {
        RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x02,
            dwFlags: flags,
            hwndTarget: target,
        }
    }

    /// Everything that can dispatch callbacks into this thread is called here with no state borrow.
    fn setup(&mut self) -> io::Result<()> {
        // SAFETY: null module name returns the executable's module handle.
        self.hinstance = unsafe { GetModuleHandleW(null()) };
        if self.hinstance.is_null() {
            return Err(io::Error::last_os_error());
        }
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: self.hinstance,
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: self.class_name.as_ptr(),
            hIconSm: null_mut(),
        };
        // SAFETY: `class` is fully initialised and the class name buffer outlives registration.
        if unsafe { RegisterClassExW(&class) } == 0 {
            return Err(io::Error::last_os_error());
        }
        self.class_registered = true;

        // A hidden, non-message-only top-level window: message-only windows do not receive the
        // broadcast display-change notifications used for diagnostics.
        // SAFETY: valid registered class name; all handles are null/owned.
        self.hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                self.class_name.as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                self.hinstance,
                null(),
            )
        };
        if self.hwnd.is_null() {
            return Err(io::Error::last_os_error());
        }
        let hwnd = self.hwnd;
        with_state(|state| state.hwnd = hwnd);

        let device = Self::mouse_device(RIDEV_INPUTSINK | RIDEV_DEVNOTIFY, self.hwnd);
        // SAFETY: one fully initialised RAWINPUTDEVICE with the correct element size.
        if unsafe { RegisterRawInputDevices(&device, 1, size_of::<RAWINPUTDEVICE>() as u32) } == 0 {
            return Err(io::Error::last_os_error());
        }
        self.raw_registered = true;

        refresh_displays(DisplayReason::Startup);

        // SAFETY: the callback is `extern "system"` and lives for the whole process.
        self.keyboard_hook =
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), self.hinstance, 0) };
        if self.keyboard_hook.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: as above.
        self.mouse_hook =
            unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), self.hinstance, 0) };
        if self.mouse_hook.is_null() {
            return Err(io::Error::last_os_error());
        }
        *self
            .shared
            .hwnd
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(self.hwnd as usize);
        Ok(())
    }

    fn teardown(&mut self) -> Vec<CaptureFault> {
        let mut faults = Vec::new();
        let mut fail = |call: &'static str| {
            // SAFETY: trivially safe.
            faults.push(CaptureFault::TeardownFailed {
                call,
                code: unsafe { GetLastError() },
            });
        };
        // No stop request may target the window past this point.
        *self
            .shared
            .hwnd
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        with_state(|state| state.hwnd = null_mut());

        for hook in [&mut self.keyboard_hook, &mut self.mouse_hook] {
            if !hook.is_null() {
                // SAFETY: handle returned by SetWindowsHookExW on this thread, unhooked once.
                if unsafe { UnhookWindowsHookEx(*hook) } == 0 {
                    fail("UnhookWindowsHookEx");
                }
                *hook = null_mut();
            }
        }
        if self.raw_registered {
            // RIDEV_REMOVE requires a NULL target window.
            let device = Self::mouse_device(RIDEV_REMOVE, null_mut());
            // SAFETY: one fully initialised RAWINPUTDEVICE with the correct element size.
            if unsafe { RegisterRawInputDevices(&device, 1, size_of::<RAWINPUTDEVICE>() as u32) }
                == 0
            {
                fail("RegisterRawInputDevices(RIDEV_REMOVE)");
            }
            self.raw_registered = false;
        }
        if !self.hwnd.is_null() {
            // SAFETY: window created by this thread, destroyed once on the same thread.
            if unsafe { DestroyWindow(self.hwnd) } == 0 {
                fail("DestroyWindow");
            }
            self.hwnd = null_mut();
        }
        if self.class_registered {
            // SAFETY: class registered by `setup` with this name and instance; its window is gone.
            if unsafe { UnregisterClassW(self.class_name.as_ptr(), self.hinstance) } == 0 {
                fail("UnregisterClassW");
            }
            self.class_registered = false;
        }
        faults
    }
}

impl Drop for Resources {
    fn drop(&mut self) {
        let _ = self.teardown();
    }
}

fn message_loop(shared: &Shared) -> Option<CaptureFault> {
    // SAFETY: MSG is plain data; all-zero is a valid initial value.
    let mut msg: MSG = unsafe { zeroed() };
    loop {
        if shared.stop_requested.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: valid out pointer; null hwnd retrieves messages for this thread's windows.
        let result = unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) };
        if result == -1 {
            // SAFETY: trivially safe.
            return Some(CaptureFault::MessageLoopFailed {
                code: unsafe { GetLastError() },
            });
        }
        if result == 0 || msg.message == WM_STOP {
            return None; // WM_QUIT or our stop message
        }
        // SAFETY: `msg` was filled by GetMessageW.
        unsafe { DispatchMessageW(&msg) };
    }
}

fn control_main(
    shared: Arc<Shared>,
    producer: Producer<CaptureEvent>,
    base: Instant,
    dpi_awareness: DpiAwareness,
    ready: mpsc::Sender<io::Result<()>>,
    _active: ActiveGuard,
) -> CaptureStats {
    // Drop order (reverse): Resources (hooks, window), StateOwner (state), FinishGuard (finished flag).
    let _finish = FinishGuard(Arc::clone(&shared));
    REENTRANT_DROPS.store(0, Ordering::Relaxed);
    let stats = CaptureStats {
        queue_capacity: producer.capacity(),
        dpi_awareness,
        ..CaptureStats::default()
    };
    let mut owner = StateOwner::install(Box::new(CallbackState {
        normalizer: Normalizer::new(),
        producer,
        shared: Arc::clone(&shared),
        base,
        hwnd: null_mut(),
        stats,
        callback_hist: DurationHistogram::new(),
        raw_hist: DurationHistogram::new(),
        overflow_first_seq: None,
        panicked: false,
        display_epoch: 0,
        faults: Vec::new(),
    }));
    let mut resources = Resources::new(Arc::clone(&shared));

    let setup = resources.setup();
    let loop_fault = match setup {
        Ok(()) => {
            let _ = ready.send(Ok(()));
            message_loop(&shared)
        }
        Err(error) => {
            let teardown = resources.teardown();
            let mut stats = owner
                .release()
                .map(|state| state.into_stats(teardown))
                .unwrap_or_default();
            stats.elapsed = base.elapsed();
            let _ = ready.send(Err(error));
            return stats;
        }
    };

    let mut teardown = resources.teardown();
    teardown.extend(loop_fault);
    let mut stats = owner
        .release()
        .map(|state| state.into_stats(teardown))
        .unwrap_or_default();
    stats.elapsed = base.elapsed();
    stats
}

fn apply_dpi_awareness() -> io::Result<DpiAwareness> {
    // SAFETY: no preconditions; must precede HWND creation, which `start` guarantees.
    if unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } != 0 {
        return Ok(DpiAwareness::Applied);
    }
    // SAFETY: trivially safe.
    let code = unsafe { GetLastError() };
    if code == ERROR_ACCESS_DENIED {
        Ok(DpiAwareness::AlreadySet)
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}

// ---------------------------------------------------------------------------------------------
// Public lifecycle API
// ---------------------------------------------------------------------------------------------

/// Cloneable, `Send` handle that asks a running session to stop (e.g. from a Ctrl-C handler).
#[derive(Clone)]
pub struct CaptureStop {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for CaptureStop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureStop").finish_non_exhaustive()
    }
}

impl CaptureStop {
    /// Non-blocking. Idempotent; `Ok` if capture already ended. After this the session's
    /// `recv_timeout` drains remaining events and then reports `Stopped`.
    pub fn request_stop(&self) -> io::Result<()> {
        self.shared.stop_requested.store(true, Ordering::Release);
        // The lock is held across the post so the window cannot be destroyed (and its handle
        // recycled) in between; PostMessageW never waits for the control thread.
        let guard = self
            .shared
            .hwnd
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match *guard {
            // SAFETY: the handle is live while it is stored in `Shared`.
            Some(hwnd) => {
                if unsafe { PostMessageW(hwnd as HWND, WM_STOP, 0, 0) } == 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            }
            None => Ok(()),
        }
    }
}

/// A running capture-only session. Capture never suppresses input, connects out or listens.
pub struct CaptureSession {
    shared: Arc<Shared>,
    consumer: Consumer<CaptureEvent>,
    thread: Option<JoinHandle<CaptureStats>>,
    delivered: u64,
    queue_capacity: usize,
}

impl std::fmt::Debug for CaptureSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureSession")
            .field("delivered", &self.delivered)
            .finish_non_exhaustive()
    }
}

impl CaptureSession {
    /// Starts the control thread and returns once the hooks and raw input are installed. Fails (with
    /// everything cleaned up) if any Win32 setup step fails or another session is already active.
    pub fn start() -> io::Result<Self> {
        let active = ActiveGuard::acquire()?;
        let dpi_awareness = apply_dpi_awareness()?;
        let (producer, consumer) = queue::channel::<CaptureEvent>(QUEUE_CAPACITY);
        let queue_capacity = producer.capacity();
        let shared = Arc::new(Shared {
            stop_requested: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            waiting: AtomicBool::new(false),
            wake_failed: AtomicBool::new(false),
            hwnd: Mutex::new(None),
            wake: EventHandle::new()?,
        });
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread_shared = Arc::clone(&shared);
        let base = Instant::now();
        let handle = thread::Builder::new()
            .name("edgebridge-capture".into())
            .spawn(move || {
                control_main(
                    thread_shared,
                    producer,
                    base,
                    dpi_awareness,
                    ready_tx,
                    active,
                )
            })?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                shared,
                consumer,
                thread: Some(handle),
                delivered: 0,
                queue_capacity,
            }),
            Ok(Err(error)) => {
                let _ = handle.join();
                Err(error)
            }
            Err(_) => {
                let _ = handle.join();
                Err(io::Error::other(
                    "capture control thread ended during startup",
                ))
            }
        }
    }

    pub fn stop_handle(&self) -> CaptureStop {
        CaptureStop {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Waits up to `timeout` for the next event in capture order. After capture ends, remaining
    /// queued events are still returned before `Stopped`.
    pub fn recv_timeout(&mut self, timeout: Duration) -> Result<CaptureEvent, CaptureRecvError> {
        let started = Instant::now();
        loop {
            if let Some(event) = self.consumer.pop() {
                self.delivered += 1;
                return Ok(event);
            }
            if self.shared.finished.load(Ordering::Acquire) {
                // `finished` is set after the last push, so one more pop is conclusive.
                return match self.consumer.pop() {
                    Some(event) => {
                        self.delivered += 1;
                        Ok(event)
                    }
                    None => Err(CaptureRecvError::Stopped),
                };
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(CaptureRecvError::Timeout);
            }
            // Park: publish intent, then re-check so a push racing with us cannot be missed.
            self.shared.waiting.store(true, Ordering::SeqCst);
            fence(Ordering::SeqCst);
            if self.consumer.is_empty() && !self.shared.finished.load(Ordering::Acquire) {
                let millis = remaining
                    .as_nanos()
                    .div_ceil(1_000_000)
                    .min(u128::from(INFINITE - 1)) as u32;
                match self.shared.wake.wait(millis) {
                    WAIT_OBJECT_0 | WAIT_TIMEOUT => {}
                    _ => {
                        self.shared.wake_failed.store(true, Ordering::Release);
                        self.shared.waiting.store(false, Ordering::SeqCst);
                        let _ = self.stop_handle().request_stop();
                        return Err(CaptureRecvError::Stopped);
                    }
                }
            }
            self.shared.waiting.store(false, Ordering::SeqCst);
        }
    }

    /// Stops capture, joins the control thread and returns final statistics including any runtime,
    /// overflow or teardown errors in `errors`.
    pub fn stop(mut self) -> io::Result<CaptureStats> {
        self.finish()
            .ok_or_else(|| io::Error::other("capture session already stopped"))
    }

    fn finish(&mut self) -> Option<CaptureStats> {
        let handle = self.thread.take()?;
        let stop_error = self.stop_handle().request_stop().err();
        let mut stats = handle.join().unwrap_or_else(|_| CaptureStats {
            queue_capacity: self.queue_capacity,
            errors: vec![CaptureFault::ControlThreadPanicked],
            ..CaptureStats::default()
        });
        if let Some(error) = stop_error {
            stats.errors.push(CaptureFault::StopRequestFailed {
                code: error.raw_os_error(),
            });
        }
        if self.shared.wake_failed.load(Ordering::Acquire) {
            stats.errors.push(CaptureFault::WakeFailed);
        }
        while self.consumer.pop().is_some() {
            stats.events_undelivered += 1;
        }
        stats.events_delivered = self.delivered;
        Some(stats)
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}
