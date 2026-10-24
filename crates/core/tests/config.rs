use edgebridge_core::config::{ClientConfig, ConfigError, ModifierProfile, ServerConfig};
use edgebridge_core::hid;

const SERVER_EXAMPLE: &str = include_str!("../../../config/server.example.toml");
const CLIENT_EXAMPLE: &str = include_str!("../../../config/client.example.toml");

fn server(text: &str, allow_lan: bool) -> Result<(), ConfigError> {
    ServerConfig::from_toml(text)?
        .validate(allow_lan)
        .map(|_| ())
}

fn client(text: &str, allow_lan: bool) -> Result<(), ConfigError> {
    ClientConfig::from_toml(text)?
        .validate(allow_lan)
        .map(|_| ())
}

fn assert_parse_err(r: Result<(), ConfigError>) {
    assert!(
        matches!(r, Err(ConfigError::Parse(_))),
        "expected parse error, got {r:?}"
    );
}

fn assert_invalid(r: Result<(), ConfigError>, needle: &str) {
    match r {
        Err(ConfigError::Invalid(msg)) => assert!(msg.contains(needle), "{msg:?} lacks {needle:?}"),
        other => panic!("expected invalid({needle}), got {other:?}"),
    }
}

#[test]
fn examples_are_valid() {
    let s = ServerConfig::from_toml(SERVER_EXAMPLE)
        .unwrap()
        .validate(false)
        .unwrap();
    assert_eq!(s.raw.layout.left, "mac");
    assert_eq!(s.escape_hotkey.key, hid::ESCAPE);

    let c = ClientConfig::from_toml(CLIENT_EXAMPLE)
        .unwrap()
        .validate(false)
        .unwrap();
    assert_eq!(c.name, "mac");
    assert_eq!(c.modifiers.profile, ModifierProfile::Physical);
}

#[test]
fn unknown_fields_are_rejected() {
    assert_parse_err(server(&format!("{SERVER_EXAMPLE}\nextra = 1\n"), false));
    assert_parse_err(server(
        &SERVER_EXAMPLE.replace("sensitivity", "sensitivty"),
        false,
    ));
    assert_parse_err(client(
        &CLIENT_EXAMPLE.replace("invert_x", "invert_z"),
        false,
    ));
}

#[test]
fn wrong_role_is_rejected() {
    assert_parse_err(server(CLIENT_EXAMPLE, false));
    assert_parse_err(client(SERVER_EXAMPLE, false));
}

#[test]
fn self_neighbor_is_rejected() {
    let text = SERVER_EXAMPLE.replace(r#"left = "mac""#, r#"left = "windows""#);
    assert_invalid(server(&text, false), "layout.left");
}

#[test]
fn empty_neighbor_name_is_rejected() {
    let text = SERVER_EXAMPLE.replace(r#"left = "mac""#, r#"left = """#);
    assert_invalid(server(&text, false), "layout.left");
}

#[test]
fn bad_sensitivity_is_rejected() {
    for bad in ["0.0", "-1.0", "nan", "inf"] {
        let text = SERVER_EXAMPLE.replace("sensitivity = 1.0", &format!("sensitivity = {bad}"));
        assert_invalid(server(&text, false), "sensitivity");
    }
}

#[test]
fn bad_escape_hotkey_is_rejected() {
    let text = SERVER_EXAMPLE.replace("LeftCtrl+LeftAlt+Escape", "Escape");
    assert_invalid(server(&text, false), "escape_hotkey");
}

#[test]
fn plaintext_on_lan_requires_explicit_flag() {
    let text = SERVER_EXAMPLE.replace("127.0.0.1:24810", "192.168.1.10:24810");
    assert_invalid(server(&text, false), "--allow-insecure-lan");
    server(&text, true).unwrap();

    let text = CLIENT_EXAMPLE.replace("127.0.0.1:24810", "192.168.1.10:24810");
    assert_invalid(client(&text, false), "--allow-insecure-lan");
    client(&text, true).unwrap();
}
