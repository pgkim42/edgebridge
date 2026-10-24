//! HID Keyboard page(0x07) 이름표. config의 hotkey 해석과 진단 출력용.
//!
//! 전체 Windows scan → HID → mac vk 표는 M4에서 KEYMAP.md의 MVP 행으로 추가한다.

use edgebridge_protocol::HidUsage;

const fn k(usage: u16) -> HidUsage {
    HidUsage::keyboard(usage)
}

pub const LEFT_CTRL: HidUsage = k(0xE0);
pub const LEFT_SHIFT: HidUsage = k(0xE1);
pub const LEFT_ALT: HidUsage = k(0xE2);
pub const LEFT_GUI: HidUsage = k(0xE3);
pub const RIGHT_CTRL: HidUsage = k(0xE4);
pub const RIGHT_SHIFT: HidUsage = k(0xE5);
pub const RIGHT_ALT: HidUsage = k(0xE6);
pub const RIGHT_GUI: HidUsage = k(0xE7);
pub const ESCAPE: HidUsage = k(0x29);

pub fn is_modifier(hid: HidUsage) -> bool {
    hid.page == HidUsage::KEYBOARD_PAGE && (0xE0..=0xE7).contains(&hid.usage)
}

const NAMED: &[(&str, u16)] = &[
    ("LeftCtrl", 0xE0),
    ("LeftShift", 0xE1),
    ("LeftAlt", 0xE2),
    ("LeftGui", 0xE3),
    ("RightCtrl", 0xE4),
    ("RightShift", 0xE5),
    ("RightAlt", 0xE6),
    ("RightGui", 0xE7),
    ("Enter", 0x28),
    ("Escape", 0x29),
    ("Backspace", 0x2A),
    ("Tab", 0x2B),
    ("Space", 0x2C),
];

/// `"A"`, `"7"`, `"F5"`, `"LeftCtrl"` 같은 이름을 HID usage로. 대소문자 구분.
pub fn from_name(name: &str) -> Option<HidUsage> {
    if let Some(&(_, usage)) = NAMED.iter().find(|(n, _)| *n == name) {
        return Some(k(usage));
    }
    let bytes = name.as_bytes();
    match bytes {
        [c @ b'A'..=b'Z'] => Some(k(0x04 + u16::from(c - b'A'))),
        [b'0'] => Some(k(0x27)),
        [c @ b'1'..=b'9'] => Some(k(0x1E + u16::from(c - b'1'))),
        [b'F', rest @ ..] => {
            let n: u16 = std::str::from_utf8(rest).ok()?.parse().ok()?;
            (1..=12).contains(&n).then(|| k(0x3A + n - 1))
        }
        _ => None,
    }
}

/// modifier 1개 이상 + 일반 키 1개. 예: `LeftCtrl+LeftAlt+Escape`. 물리 키(remap 전) 기준.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: Vec<HidUsage>,
    pub key: HidUsage,
}

impl std::str::FromStr for Hotkey {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let (key_name, modifier_names) = parts.split_last().ok_or("empty hotkey")?;
        if modifier_names.is_empty() {
            return Err(format!("hotkey `{s}` needs at least one modifier"));
        }
        let key = from_name(key_name).ok_or_else(|| format!("unknown key `{key_name}`"))?;
        if is_modifier(key) {
            return Err(format!("hotkey `{s}` must end with a non-modifier key"));
        }
        let mut modifiers = Vec::with_capacity(modifier_names.len());
        for name in modifier_names {
            let hid = from_name(name).ok_or_else(|| format!("unknown key `{name}`"))?;
            if !is_modifier(hid) {
                return Err(format!("`{name}` is not a modifier"));
            }
            if modifiers.contains(&hid) {
                return Err(format!("duplicate modifier `{name}`"));
            }
            modifiers.push(hid);
        }
        Ok(Self { modifiers, key })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_map_to_keymap_md_usages() {
        assert_eq!(from_name("A"), Some(k(0x04)));
        assert_eq!(from_name("Z"), Some(k(0x1D)));
        assert_eq!(from_name("1"), Some(k(0x1E)));
        assert_eq!(from_name("9"), Some(k(0x26)));
        assert_eq!(from_name("0"), Some(k(0x27)));
        assert_eq!(from_name("F1"), Some(k(0x3A)));
        assert_eq!(from_name("F12"), Some(k(0x45)));
        assert_eq!(from_name("F13"), None);
        assert_eq!(from_name("Escape"), Some(ESCAPE));
        assert_eq!(from_name("a"), None);
    }

    #[test]
    fn parses_default_escape_hotkey() {
        let hk: Hotkey = "LeftCtrl+LeftAlt+Escape".parse().unwrap();
        assert_eq!(hk.modifiers, vec![LEFT_CTRL, LEFT_ALT]);
        assert_eq!(hk.key, ESCAPE);
    }

    #[test]
    fn rejects_bad_hotkeys() {
        for bad in [
            "",
            "Escape",
            "LeftCtrl",
            "LeftCtrl+LeftAlt",
            "A+Escape",
            "LeftCtrl+LeftCtrl+Escape",
            "LeftCtrl+Nope",
        ] {
            assert!(bad.parse::<Hotkey>().is_err(), "{bad:?} should be rejected");
        }
    }
}
