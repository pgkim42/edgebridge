//! Portable normalization of raw Win32 records into [`CaptureEvent`]s.
//!
//! The Win32 backend copies the minimal OS struct fields into the plain records below and calls the
//! [`Normalizer`] inside the hook callback, so this code must stay allocation-free, non-blocking and
//! small. It has no OS dependency, so it is tested on every host.
//!
//! Policy:
//! * Injected input (`LLKHF_INJECTED`/`LLKHF_LOWER_IL_INJECTED`, `LLMHF_INJECTED`/
//!   `LLMHF_LOWER_IL_INJECTED`) is filtered out of the captured stream.
//! * LL mouse movement is filtered; movement comes only from Raw Input. LL buttons and wheels are
//!   the only source of button/wheel events (Raw buttons/wheels are not read), so nothing is doubled.
//! * Key repeat is derived from the session's own held-key bits. Keys already held when capture
//!   started have no known state: their first observed Down (possibly an auto-repeat) is reported
//!   as `Down`, and an Up for a key never seen down is still reported as `Up`.
//! * `seq` is consumed only by events that are produced, in the order records reach the normalizer.

use crate::model::{
    CaptureEvent, CaptureEventKind, KeyEvent, KeyState, Modifier, MouseButton, MouseButtonEvent,
    MouseWheelEvent, RawMotion, RawMouseMotion, WheelAxis,
};

// Window message ids delivered as `wParam` of the LL hooks (winuser.h).
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_SYSKEYUP: u32 = 0x0105;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_MBUTTONDOWN: u32 = 0x0207;
pub const WM_MBUTTONUP: u32 = 0x0208;
pub const WM_MOUSEWHEEL: u32 = 0x020A;
pub const WM_XBUTTONDOWN: u32 = 0x020B;
pub const WM_XBUTTONUP: u32 = 0x020C;
pub const WM_MOUSEHWHEEL: u32 = 0x020E;

pub const LLKHF_EXTENDED: u32 = 0x01;
pub const LLKHF_LOWER_IL_INJECTED: u32 = 0x02;
pub const LLKHF_INJECTED: u32 = 0x10;
pub const LLMHF_INJECTED: u32 = 0x01;
pub const LLMHF_LOWER_IL_INJECTED: u32 = 0x02;

pub const MOUSE_MOVE_ABSOLUTE: u16 = 0x01;
pub const MOUSE_VIRTUAL_DESKTOP: u16 = 0x02;

/// Minimal copy of `KBDLLHOOKSTRUCT` plus the hook `wParam` message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardRecord {
    pub message: u32,
    pub vk_code: u32,
    pub scan_code: u32,
    pub flags: u32,
    pub time: u32,
}

/// Minimal copy of `MSLLHOOKSTRUCT` plus the hook `wParam` message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseRecord {
    pub message: u32,
    pub x: i32,
    pub y: i32,
    pub mouse_data: u32,
    pub flags: u32,
    pub time: u32,
}

/// Minimal copy of a mouse `RAWINPUT`: header device handle plus `RAWMOUSE.usFlags/lLastX/lLastY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawMouseRecord {
    pub device: u64,
    pub flags: u16,
    pub last_x: i32,
    pub last_y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterReason {
    /// Injected (synthetic) input; passes through to Windows but is not captured.
    Injected,
    /// LL mouse move; movement is captured from Raw Input only.
    MouseMove,
    /// Message or button not part of the capture contract.
    UnknownMessage,
    /// Raw record carried no movement (button/wheel-only report, zero delta, repeated absolute).
    NoRawMotion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Event(CaptureEvent),
    Filtered(FilterReason),
}

#[derive(Debug, Clone)]
pub struct Normalizer {
    next_seq: u64,
    /// Physical scan code plus extended bit; scan-less records use a separate virtual-key range.
    held: [u64; 12],
    last_absolute: Option<(u64, i32, i32, bool)>,
}

impl Default for Normalizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Normalizer {
    pub const fn new() -> Self {
        Self {
            next_seq: 0,
            held: [0; 12],
            last_absolute: None,
        }
    }

    /// Sequence number the next produced event will receive.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Wraps a non-input event (device change, display diagnostics) with the next seq.
    pub fn stamp(&mut self, kind: CaptureEventKind, timestamp_ns: u64) -> CaptureEvent {
        if let CaptureEventKind::RawDevice(change) = kind
            && self
                .last_absolute
                .is_some_and(|(device, ..)| device == change.device)
        {
            self.last_absolute = None;
        }
        let seq = self.next_seq;
        self.next_seq = seq.wrapping_add(1);
        CaptureEvent {
            seq,
            timestamp_ns,
            kind,
        }
    }

    pub fn keyboard(&mut self, rec: &KeyboardRecord, timestamp_ns: u64) -> Outcome {
        if rec.flags & (LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED) != 0 {
            return Outcome::Filtered(FilterReason::Injected);
        }
        let down = match rec.message {
            WM_KEYDOWN | WM_SYSKEYDOWN => true,
            WM_KEYUP | WM_SYSKEYUP => false,
            _ => return Outcome::Filtered(FilterReason::UnknownMessage),
        };
        let slot = if rec.scan_code == 0 {
            512 + (rec.vk_code & 0xFF) as usize
        } else {
            (rec.scan_code & 0xFF) as usize + usize::from(rec.flags & LLKHF_EXTENDED != 0) * 256
        };
        let (word, bit) = (slot / 64, 1u64 << (slot % 64));
        let was_held = self.held[word] & bit != 0;
        let state = if down {
            self.held[word] |= bit;
            if was_held {
                KeyState::Repeat
            } else {
                KeyState::Down
            }
        } else {
            self.held[word] &= !bit;
            KeyState::Up
        };
        let vk_code = rec.vk_code as u16;
        let key = KeyEvent {
            vk_code,
            scan_code: rec.scan_code as u16,
            extended: rec.flags & LLKHF_EXTENDED != 0,
            state,
            modifier: Modifier::from_vk(vk_code),
            os_time_ms: rec.time,
        };
        Outcome::Event(self.stamp(CaptureEventKind::Key(key), timestamp_ns))
    }

    pub fn mouse(&mut self, rec: &MouseRecord, timestamp_ns: u64) -> Outcome {
        if rec.flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) != 0 {
            return Outcome::Filtered(FilterReason::Injected);
        }
        let high_word = (rec.mouse_data >> 16) as u16;
        let button = |button, pressed| {
            CaptureEventKind::MouseButton(MouseButtonEvent {
                button,
                pressed,
                x: rec.x,
                y: rec.y,
                os_time_ms: rec.time,
            })
        };
        let wheel = |axis| {
            CaptureEventKind::MouseWheel(MouseWheelEvent {
                axis,
                delta: high_word as i16,
                x: rec.x,
                y: rec.y,
                os_time_ms: rec.time,
            })
        };
        let kind = match rec.message {
            WM_MOUSEMOVE => return Outcome::Filtered(FilterReason::MouseMove),
            WM_LBUTTONDOWN => button(MouseButton::Left, true),
            WM_LBUTTONUP => button(MouseButton::Left, false),
            WM_RBUTTONDOWN => button(MouseButton::Right, true),
            WM_RBUTTONUP => button(MouseButton::Right, false),
            WM_MBUTTONDOWN => button(MouseButton::Middle, true),
            WM_MBUTTONUP => button(MouseButton::Middle, false),
            WM_XBUTTONDOWN | WM_XBUTTONUP => {
                let pressed = rec.message == WM_XBUTTONDOWN;
                match high_word {
                    1 => button(MouseButton::X1, pressed),
                    2 => button(MouseButton::X2, pressed),
                    _ => return Outcome::Filtered(FilterReason::UnknownMessage),
                }
            }
            WM_MOUSEWHEEL => wheel(WheelAxis::Vertical),
            WM_MOUSEHWHEEL => wheel(WheelAxis::Horizontal),
            _ => return Outcome::Filtered(FilterReason::UnknownMessage),
        };
        Outcome::Event(self.stamp(kind, timestamp_ns))
    }

    pub fn raw_mouse(&mut self, rec: &RawMouseRecord, timestamp_ns: u64) -> Outcome {
        let motion = if rec.flags & MOUSE_MOVE_ABSOLUTE != 0 {
            let current = (
                rec.device,
                rec.last_x,
                rec.last_y,
                rec.flags & MOUSE_VIRTUAL_DESKTOP != 0,
            );
            if self.last_absolute == Some(current) {
                return Outcome::Filtered(FilterReason::NoRawMotion);
            }
            self.last_absolute = Some(current);
            RawMotion::Absolute {
                x: rec.last_x,
                y: rec.last_y,
                virtual_desktop: rec.flags & MOUSE_VIRTUAL_DESKTOP != 0,
            }
        } else {
            self.last_absolute = None;
            if rec.last_x == 0 && rec.last_y == 0 {
                return Outcome::Filtered(FilterReason::NoRawMotion);
            }
            RawMotion::Relative {
                dx: rec.last_x,
                dy: rec.last_y,
            }
        };
        let kind = CaptureEventKind::RawMouseMotion(RawMouseMotion {
            device: rec.device,
            motion,
        });
        Outcome::Event(self.stamp(kind, timestamp_ns))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(message: u32, vk: u32, scan: u32, flags: u32) -> KeyboardRecord {
        KeyboardRecord {
            message,
            vk_code: vk,
            scan_code: scan,
            flags,
            time: 7,
        }
    }

    fn mouse(message: u32, mouse_data: u32, flags: u32) -> MouseRecord {
        MouseRecord {
            message,
            x: -5,
            y: 9,
            mouse_data,
            flags,
            time: 11,
        }
    }

    fn event(outcome: Outcome) -> CaptureEvent {
        match outcome {
            Outcome::Event(event) => event,
            other => panic!("expected an event, got {other:?}"),
        }
    }

    fn key_event(outcome: Outcome) -> KeyEvent {
        match event(outcome).kind {
            CaptureEventKind::Key(key) => key,
            other => panic!("expected key, got {other:?}"),
        }
    }

    #[test]
    fn injected_keyboard_is_filtered_and_does_not_touch_held_state() {
        let mut n = Normalizer::new();
        for flags in [
            LLKHF_INJECTED,
            LLKHF_LOWER_IL_INJECTED,
            LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED,
        ] {
            assert_eq!(
                n.keyboard(&key(WM_KEYDOWN, 0x41, 0x1E, flags), 1),
                Outcome::Filtered(FilterReason::Injected)
            );
        }
        // The injected downs above must not turn the first physical Down into a Repeat.
        assert_eq!(
            key_event(n.keyboard(&key(WM_KEYDOWN, 0x41, 0x1E, 0), 2)).state,
            KeyState::Down
        );
        assert_eq!(
            n.next_seq(),
            1,
            "filtered records do not consume sequence numbers"
        );
    }

    #[test]
    fn key_down_repeat_up_cycle_and_syskey_variants() {
        let mut n = Normalizer::new();
        let states: Vec<KeyState> = [
            (WM_KEYDOWN, 0x41),
            (WM_KEYDOWN, 0x41),
            (WM_KEYDOWN, 0x41),
            (WM_KEYUP, 0x41),
            (WM_KEYDOWN, 0x41),
            (WM_SYSKEYDOWN, 0x41),
            (WM_SYSKEYUP, 0x41),
        ]
        .into_iter()
        .map(|(message, vk)| key_event(n.keyboard(&key(message, vk, 0x1E, 0), 0)).state)
        .collect();
        assert_eq!(
            states,
            [
                KeyState::Down,
                KeyState::Repeat,
                KeyState::Repeat,
                KeyState::Up,
                KeyState::Down,
                KeyState::Repeat,
                KeyState::Up
            ]
        );
    }

    #[test]
    fn up_for_a_key_never_seen_down_is_still_reported() {
        let mut n = Normalizer::new();
        assert_eq!(
            key_event(n.keyboard(&key(WM_KEYUP, 0x10, 0x2A, 0), 0)).state,
            KeyState::Up
        );
        assert_eq!(
            key_event(n.keyboard(&key(WM_KEYDOWN, 0x10, 0x2A, 0), 0)).state,
            KeyState::Down
        );
    }

    #[test]
    fn left_and_right_modifiers_are_independent_identities() {
        let mut n = Normalizer::new();
        let lshift = key_event(n.keyboard(&key(WM_KEYDOWN, 0xA0, 0x2A, 0), 0));
        let rshift = key_event(n.keyboard(&key(WM_KEYDOWN, 0xA1, 0x36, 0), 0));
        assert_eq!(lshift.state, KeyState::Down);
        assert_eq!(
            rshift.state,
            KeyState::Down,
            "right shift is not a repeat of left shift"
        );
        assert_eq!(lshift.modifier, Some(Modifier::LeftShift));
        assert_eq!(rshift.modifier, Some(Modifier::RightShift));
        // Releasing one side leaves the other side held.
        assert_eq!(
            key_event(n.keyboard(&key(WM_KEYUP, 0xA0, 0x2A, 0), 0)).state,
            KeyState::Up
        );
        assert_eq!(
            key_event(n.keyboard(&key(WM_KEYDOWN, 0xA1, 0x36, 0), 0)).state,
            KeyState::Repeat
        );

        let expected = [
            (0xA2, Modifier::LeftCtrl),
            (0xA3, Modifier::RightCtrl),
            (0xA4, Modifier::LeftAlt),
            (0xA5, Modifier::RightAlt),
            (0x5B, Modifier::LeftWin),
            (0x5C, Modifier::RightWin),
        ];
        for (vk, modifier) in expected {
            assert_eq!(
                key_event(n.keyboard(&key(WM_KEYDOWN, vk, 1, 0), 0)).modifier,
                Some(modifier)
            );
        }
        assert_eq!(
            key_event(n.keyboard(&key(WM_KEYDOWN, 0x41, 0x1E, 0), 0)).modifier,
            None
        );
    }

    #[test]
    fn physical_keys_sharing_a_virtual_key_have_independent_repeat_state() {
        let mut n = Normalizer::new();
        let plain = key(WM_KEYDOWN, 0x25, 0x4B, 0);
        let extended = key(WM_KEYDOWN, 0x25, 0x4B, LLKHF_EXTENDED);
        assert_eq!(key_event(n.keyboard(&plain, 0)).state, KeyState::Down);
        assert_eq!(key_event(n.keyboard(&extended, 1)).state, KeyState::Down);
        n.keyboard(
            &KeyboardRecord {
                message: WM_KEYUP,
                ..plain
            },
            2,
        );
        assert_eq!(key_event(n.keyboard(&extended, 3)).state, KeyState::Repeat);
        assert_eq!(key_event(n.keyboard(&plain, 4)).state, KeyState::Down);
    }

    #[test]
    fn unknown_keyboard_message_is_filtered() {
        let mut n = Normalizer::new();
        assert_eq!(
            n.keyboard(&key(0x0102, 0x41, 0, 0), 0),
            Outcome::Filtered(FilterReason::UnknownMessage)
        );
    }

    #[test]
    fn all_five_buttons_report_both_edges() {
        let mut n = Normalizer::new();
        let cases = [
            (WM_LBUTTONDOWN, 0u32, MouseButton::Left, true),
            (WM_LBUTTONUP, 0, MouseButton::Left, false),
            (WM_RBUTTONDOWN, 0, MouseButton::Right, true),
            (WM_RBUTTONUP, 0, MouseButton::Right, false),
            (WM_MBUTTONDOWN, 0, MouseButton::Middle, true),
            (WM_MBUTTONUP, 0, MouseButton::Middle, false),
            (WM_XBUTTONDOWN, 1 << 16, MouseButton::X1, true),
            (WM_XBUTTONUP, 1 << 16, MouseButton::X1, false),
            (WM_XBUTTONDOWN, 2 << 16, MouseButton::X2, true),
            (WM_XBUTTONUP, 2 << 16, MouseButton::X2, false),
        ];
        for (message, data, button, pressed) in cases {
            match event(n.mouse(&mouse(message, data, 0), 0)).kind {
                CaptureEventKind::MouseButton(e) => {
                    assert_eq!(
                        (e.button, e.pressed, e.x, e.y, e.os_time_ms),
                        (button, pressed, -5, 9, 11)
                    );
                }
                other => panic!("expected button, got {other:?}"),
            }
        }
        assert_eq!(
            n.mouse(&mouse(WM_XBUTTONDOWN, 3 << 16, 0), 0),
            Outcome::Filtered(FilterReason::UnknownMessage),
            "an unknown X button id is not guessed"
        );
    }

    #[test]
    fn wheel_delta_keeps_its_sign_and_axis() {
        let mut n = Normalizer::new();
        let delta = |event: CaptureEvent| match event.kind {
            CaptureEventKind::MouseWheel(w) => (w.axis, w.delta),
            other => panic!("expected wheel, got {other:?}"),
        };
        // mouseData high word is a signed short; the low word is noise that must not leak in.
        let up = (120u32 << 16) | 0x00AB;
        let down = ((-120i16 as u16 as u32) << 16) | 0x00AB;
        let fine = ((-1i16 as u16 as u32) << 16) | 0xFFFF;
        assert_eq!(
            delta(event(n.mouse(&mouse(WM_MOUSEWHEEL, up, 0), 0))),
            (WheelAxis::Vertical, 120)
        );
        assert_eq!(
            delta(event(n.mouse(&mouse(WM_MOUSEWHEEL, down, 0), 0))),
            (WheelAxis::Vertical, -120)
        );
        assert_eq!(
            delta(event(n.mouse(&mouse(WM_MOUSEHWHEEL, down, 0), 0))),
            (WheelAxis::Horizontal, -120)
        );
        assert_eq!(
            delta(event(n.mouse(&mouse(WM_MOUSEWHEEL, fine, 0), 0))),
            (WheelAxis::Vertical, -1)
        );
        assert_eq!(
            delta(event(n.mouse(
                &mouse(WM_MOUSEWHEEL, (i16::MIN as u16 as u32) << 16, 0),
                0
            ))),
            (WheelAxis::Vertical, i16::MIN)
        );
    }

    #[test]
    fn ll_mouse_move_and_injected_mouse_are_not_captured() {
        let mut n = Normalizer::new();
        assert_eq!(
            n.mouse(&mouse(WM_MOUSEMOVE, 0, 0), 0),
            Outcome::Filtered(FilterReason::MouseMove)
        );
        for flags in [LLMHF_INJECTED, LLMHF_LOWER_IL_INJECTED] {
            assert_eq!(
                n.mouse(&mouse(WM_LBUTTONDOWN, 0, flags), 0),
                Outcome::Filtered(FilterReason::Injected)
            );
            assert_eq!(
                n.mouse(&mouse(WM_MOUSEWHEEL, 120 << 16, flags), 0),
                Outcome::Filtered(FilterReason::Injected)
            );
        }
        assert_eq!(n.next_seq(), 0);
    }

    #[test]
    fn raw_relative_and_absolute_are_distinct_and_signed() {
        let mut n = Normalizer::new();
        let motion = |event: CaptureEvent| match event.kind {
            CaptureEventKind::RawMouseMotion(m) => m,
            other => panic!("expected raw motion, got {other:?}"),
        };
        let relative = RawMouseRecord {
            device: 0xD1,
            flags: 0,
            last_x: -7,
            last_y: 3,
        };
        let absolute = RawMouseRecord {
            device: 0xD2,
            flags: MOUSE_MOVE_ABSOLUTE,
            last_x: 32_000,
            last_y: 100,
        };
        let virtual_abs = RawMouseRecord {
            device: 0xD2,
            flags: MOUSE_MOVE_ABSOLUTE | MOUSE_VIRTUAL_DESKTOP,
            last_x: 1,
            last_y: 2,
        };
        assert_eq!(
            motion(event(n.raw_mouse(&relative, 0))),
            RawMouseMotion {
                device: 0xD1,
                motion: RawMotion::Relative { dx: -7, dy: 3 }
            }
        );
        assert_eq!(
            motion(event(n.raw_mouse(&absolute, 0))).motion,
            RawMotion::Absolute {
                x: 32_000,
                y: 100,
                virtual_desktop: false
            }
        );
        assert_eq!(
            motion(event(n.raw_mouse(&virtual_abs, 0))).motion,
            RawMotion::Absolute {
                x: 1,
                y: 2,
                virtual_desktop: true
            }
        );
    }

    #[test]
    fn raw_reports_without_motion_are_filtered() {
        let mut n = Normalizer::new();
        let button_only = RawMouseRecord {
            device: 1,
            flags: 0,
            last_x: 0,
            last_y: 0,
        };
        assert_eq!(
            n.raw_mouse(&button_only, 0),
            Outcome::Filtered(FilterReason::NoRawMotion)
        );
        let abs = RawMouseRecord {
            device: 1,
            flags: MOUSE_MOVE_ABSOLUTE,
            last_x: 10,
            last_y: 10,
        };
        assert!(matches!(n.raw_mouse(&abs, 0), Outcome::Event(_)));
        assert_eq!(
            n.raw_mouse(&abs, 0),
            Outcome::Filtered(FilterReason::NoRawMotion)
        );
        let other_device = RawMouseRecord { device: 2, ..abs };
        assert!(
            matches!(n.raw_mouse(&other_device, 0), Outcome::Event(_)),
            "per-device position memory"
        );
    }

    #[test]
    fn absolute_mapping_changes_and_device_rearrival_are_not_deduplicated() {
        let mut n = Normalizer::new();
        let record = RawMouseRecord {
            device: 1,
            flags: MOUSE_MOVE_ABSOLUTE,
            last_x: 10,
            last_y: 10,
        };
        event(n.raw_mouse(&record, 0));
        let changed = RawMouseRecord {
            flags: MOUSE_MOVE_ABSOLUTE | MOUSE_VIRTUAL_DESKTOP,
            ..record
        };
        assert!(matches!(
            event(n.raw_mouse(&changed, 1)).kind,
            CaptureEventKind::RawMouseMotion(RawMouseMotion {
                motion: RawMotion::Absolute {
                    virtual_desktop: true,
                    ..
                },
                ..
            })
        ));
        n.stamp(
            CaptureEventKind::RawDevice(crate::model::RawDeviceChange {
                device: 1,
                arrived: false,
            }),
            2,
        );
        n.stamp(
            CaptureEventKind::RawDevice(crate::model::RawDeviceChange {
                device: 1,
                arrived: true,
            }),
            3,
        );
        assert!(matches!(n.raw_mouse(&changed, 4), Outcome::Event(_)));
    }

    #[test]
    fn sequence_and_timestamps_follow_observation_order_across_kinds() {
        let mut n = Normalizer::new();
        let a = event(n.keyboard(&key(WM_KEYDOWN, 0x41, 0x1E, 0), 100));
        assert_eq!(
            n.mouse(&mouse(WM_MOUSEMOVE, 0, 0), 150),
            Outcome::Filtered(FilterReason::MouseMove)
        );
        let b = event(n.raw_mouse(
            &RawMouseRecord {
                device: 1,
                flags: 0,
                last_x: 4,
                last_y: 0,
            },
            200,
        ));
        let c = event(n.mouse(&mouse(WM_LBUTTONDOWN, 0, 0), 300));
        let d = n.stamp(
            CaptureEventKind::RawDevice(crate::model::RawDeviceChange {
                device: 1,
                arrived: false,
            }),
            400,
        );
        assert_eq!([a.seq, b.seq, c.seq, d.seq], [0, 1, 2, 3]);
        assert_eq!(
            [
                a.timestamp_ns,
                b.timestamp_ns,
                c.timestamp_ns,
                d.timestamp_ns
            ],
            [100, 200, 300, 400]
        );
    }
}
