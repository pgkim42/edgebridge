//! TOML 설정. ARCHITECTURE.md §8.
//!
//! unknown field는 parse 오류, 의미 오류는 [`ConfigError::Invalid`]. 둘 다 시작 실패로 처리한다.
//! runtime hot reload는 없다.

use std::net::SocketAddr;

use serde::Deserialize;

use crate::hid::Hotkey;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid config: {0}")]
    Invalid(String),
}

fn invalid<T>(msg: impl Into<String>) -> Result<T, ConfigError> {
    Err(ConfigError::Invalid(msg.into()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Security {
    /// M1–M6 개발용 평문. loopback 외에서는 `--allow-insecure-lan`이 필요하다.
    PlaintextDev,
    /// M7 TLS 1.3 상호 인증 + pinning. 아직 구현되지 않았다.
    TlsPinned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModifierProfile {
    Physical,
    CtrlCmdSwap,
}

// ---------------------------------------------------------------- server

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ServerRole {
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServerScreenSelector {
    Primary,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(rename = "role")]
    _role: ServerRole,
    pub network: ServerNetwork,
    pub layout: Layout,
    pub screen: ServerScreen,
    #[serde(default)]
    pub input: Input,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerNetwork {
    pub bind: SocketAddr,
    pub security: Security,
}

/// v1은 Windows 한 대의 왼쪽에 Mac 한 대. 역방향(mac.right = windows)은 자동 도출한다.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub local: String,
    pub left: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerScreen {
    pub selector: ServerScreenSelector,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Input {
    pub sensitivity: f64,
    pub escape_hotkey: String,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            sensitivity: 1.0,
            escape_hotkey: "LeftCtrl+LeftAlt+Escape".to_owned(),
        }
    }
}

/// 검증을 마친 server 설정.
#[derive(Debug, Clone)]
pub struct ValidServerConfig {
    pub raw: ServerConfig,
    pub escape_hotkey: Hotkey,
}

impl ServerConfig {
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(text)?)
    }

    pub fn validate(self, allow_insecure_lan: bool) -> Result<ValidServerConfig, ConfigError> {
        validate_name("layout.local", &self.layout.local)?;
        validate_name("layout.left", &self.layout.left)?;
        if self.layout.local == self.layout.left {
            return invalid("layout.left must not be the local screen itself");
        }
        let s = self.input.sensitivity;
        if !s.is_finite() || s <= 0.0 || s > 100.0 {
            return invalid("input.sensitivity must be finite and in (0, 100]");
        }
        let escape_hotkey = self
            .input
            .escape_hotkey
            .parse()
            .map_err(|e| ConfigError::Invalid(format!("input.escape_hotkey: {e}")))?;
        check_insecure(
            self.network.security,
            self.network.bind,
            "network.bind",
            allow_insecure_lan,
        )?;
        Ok(ValidServerConfig {
            raw: self,
            escape_hotkey,
        })
    }
}

// ---------------------------------------------------------------- client

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ClientRole {
    Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientScreenSelector {
    /// 내장 디스플레이.
    Builtin,
    /// CGMainDisplayID.
    Main,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientConfig {
    #[serde(rename = "role")]
    _role: ClientRole,
    pub name: String,
    pub network: ClientNetwork,
    pub screen: ClientScreen,
    #[serde(default)]
    pub modifiers: Modifiers,
    #[serde(default)]
    pub scroll: ScrollConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientNetwork {
    pub server: SocketAddr,
    pub security: Security,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientScreen {
    pub selector: ClientScreenSelector,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modifiers {
    pub profile: ModifierProfile,
}

impl Default for Modifiers {
    fn default() -> Self {
        Self {
            profile: ModifierProfile::Physical,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScrollConfig {
    pub invert_x: bool,
    pub invert_y: bool,
}

impl ClientConfig {
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(text)?)
    }

    pub fn validate(self, allow_insecure_lan: bool) -> Result<ClientConfig, ConfigError> {
        validate_name("name", &self.name)?;
        check_insecure(
            self.network.security,
            self.network.server,
            "network.server",
            allow_insecure_lan,
        )?;
        Ok(self)
    }
}

// ---------------------------------------------------------------- shared

/// Hello name과 같은 규칙: 1..=64 bytes, 제어 문자 없음.
fn validate_name(field: &str, name: &str) -> Result<(), ConfigError> {
    use edgebridge_protocol::limits::{NAME_BYTES_MAX, NAME_BYTES_MIN};
    if name.len() < NAME_BYTES_MIN
        || name.len() > NAME_BYTES_MAX
        || name.chars().any(char::is_control)
    {
        return invalid(format!(
            "{field} must be 1..=64 bytes without control characters"
        ));
    }
    Ok(())
}

fn check_insecure(
    security: Security,
    addr: SocketAddr,
    field: &str,
    allow_insecure_lan: bool,
) -> Result<(), ConfigError> {
    if security == Security::PlaintextDev && !addr.ip().is_loopback() && !allow_insecure_lan {
        return invalid(format!(
            "{field} {addr} is not loopback; plaintext-dev on LAN requires --allow-insecure-lan"
        ));
    }
    Ok(())
}
