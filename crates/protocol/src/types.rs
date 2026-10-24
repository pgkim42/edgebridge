//! 공통 wire 값 타입. PROTOCOL.md §3.
//!
//! 정수 enum은 Rust 선언 순번이 아니라 명시된 값으로 직렬화한다.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::limits;

/// 명시 숫자를 wire 값으로 쓰는 enum. 미정의 숫자는 deserialize 단계에서 거부한다.
macro_rules! wire_enum {
    (
        $(#[$meta:meta])*
        $name:ident: $repr:ty, $ser:ident {
            $($(#[$vmeta:meta])* $variant:ident = $value:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[repr($repr)]
        pub enum $name {
            $($(#[$vmeta])* $variant = $value),+
        }

        impl $name {
            pub const fn to_wire(self) -> $repr {
                self as $repr
            }

            pub const fn from_wire(value: $repr) -> Option<Self> {
                match value {
                    $($value => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.$ser(self.to_wire())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let raw = <$repr>::deserialize(d)?;
                Self::from_wire(raw).ok_or_else(|| {
                    serde::de::Error::custom(concat!("undefined ", stringify!($name), " value"))
                })
            }
        }
    };
}

wire_enum! {
    Role: u8, serialize_u8 {
        Server = 0,
        Client = 1,
    }
}

wire_enum! {
    /// Mac 화면에 진입하는 edge. v1에서는 Right만 존재한다 (Mac이 Windows 왼쪽).
    Edge: u8, serialize_u8 {
        Right = 0,
    }
}

wire_enum! {
    KeyAction: u8, serialize_u8 {
        Down = 0,
        Up = 1,
        Repeat = 2,
    }
}

wire_enum! {
    /// Button은 Repeat가 없으므로 KeyAction과 타입을 분리한다.
    ButtonAction: u8, serialize_u8 {
        Down = 0,
        Up = 1,
    }
}

wire_enum! {
    MouseButton: u8, serialize_u8 {
        Left = 0,
        Right = 1,
        Middle = 2,
        Back = 3,
        Forward = 4,
    }
}

wire_enum! {
    ScrollUnit: u8, serialize_u8 {
        Lines = 0,
        Pixels = 1,
    }
}

wire_enum! {
    Reason: u8, serialize_u8 {
        Normal = 0,
        Escape = 1,
        Timeout = 2,
        QueueOverflow = 3,
        Permission = 4,
        DisplayChanged = 5,
        SleepOrLock = 6,
        ProtocolError = 7,
        Shutdown = 8,
        BackendFailure = 9,
    }
}

wire_enum! {
    ErrorCode: u16, serialize_u16 {
        Version = 1,
        BadFrame = 2,
        BadState = 3,
        InvalidValue = 4,
        Unsupported = 5,
        Busy = 6,
        Permission = 7,
    }
}

/// 물리 USB HID usage. KEYMAP.md 참조.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HidUsage {
    pub page: u16,
    pub usage: u16,
}

impl HidUsage {
    pub const KEYBOARD_PAGE: u16 = 0x07;

    pub const fn keyboard(usage: u16) -> Self {
        Self {
            page: Self::KEYBOARD_PAGE,
            usage,
        }
    }
}

/// 선택된 Mac 화면 local 좌표 (Quartz logical 단위).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

impl Position {
    /// wire 수준 검증: 유한수, 음수 아님. 화면 크기 상한은 session에서 검사한다.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.x.is_finite() || !self.y.is_finite() {
            return Err("position must be finite");
        }
        if self.x < 0.0 || self.y < 0.0 {
            return Err("position must be non-negative");
        }
        Ok(())
    }

    /// `0 <= x <= width-1`, `0 <= y <= height-1`.
    pub fn is_within(&self, screen: &ScreenInfo) -> bool {
        self.validate().is_ok() && self.x <= screen.width - 1.0 && self.y <= screen.height - 1.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScreenInfo {
    pub id: u32,
    pub origin_x: f64,
    pub origin_y: f64,
    pub width: f64,
    pub height: f64,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub scale_x: f64,
    pub scale_y: f64,
}

impl ScreenInfo {
    pub fn validate(&self) -> Result<(), &'static str> {
        let floats = [
            self.origin_x,
            self.origin_y,
            self.width,
            self.height,
            self.scale_x,
            self.scale_y,
        ];
        if floats.iter().any(|v| !v.is_finite()) {
            return Err("screen values must be finite");
        }
        if self.origin_x.abs() > limits::SCREEN_ORIGIN_ABS_MAX
            || self.origin_y.abs() > limits::SCREEN_ORIGIN_ABS_MAX
        {
            return Err("screen origin out of range");
        }
        let logical = limits::SCREEN_LOGICAL_MIN..=limits::SCREEN_LOGICAL_MAX;
        if !logical.contains(&self.width) || !logical.contains(&self.height) {
            return Err("screen logical size out of range");
        }
        let pixel = limits::SCREEN_PIXEL_MIN..=limits::SCREEN_PIXEL_MAX;
        if !pixel.contains(&self.pixel_width) || !pixel.contains(&self.pixel_height) {
            return Err("screen pixel size out of range");
        }
        let scale = limits::SCREEN_SCALE_MIN..=limits::SCREEN_SCALE_MAX;
        if !scale.contains(&self.scale_x) || !scale.contains(&self.scale_y) {
            return Err("screen scale out of range");
        }
        let ratio_ok = |pixels: u32, logical: f64, scale: f64| {
            let ratio = f64::from(pixels) / logical;
            ((ratio - scale) / scale).abs() <= limits::SCREEN_SCALE_TOLERANCE
        };
        if !ratio_ok(self.pixel_width, self.width, self.scale_x)
            || !ratio_ok(self.pixel_height, self.height, self.scale_y)
        {
            return Err("screen pixel/logical ratio disagrees with scale");
        }
        Ok(())
    }
}

/// capability bit 집합. 정의되지 않은 bit는 0이어야 한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Capabilities(pub u64);

impl Capabilities {
    pub const POINTER: Self = Self(1 << 0);
    pub const KEYBOARD: Self = Self(1 << 1);
    pub const SCROLL_LINES: Self = Self(1 << 2);
    pub const SCROLL_PIXELS: Self = Self(1 << 3);
    pub const ALL_V1: Self = Self(0b1111);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub const fn has_reserved_bits(self) -> bool {
        self.0 & !Self::ALL_V1.0 != 0
    }

    /// 이 scroll 단위를 쓰려면 필요한 capability.
    pub const fn for_scroll(unit: ScrollUnit) -> Self {
        match unit {
            ScrollUnit::Lines => Self::SCROLL_LINES,
            ScrollUnit::Pixels => Self::SCROLL_PIXELS,
        }
    }
}

impl std::ops::BitOr for Capabilities {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}
