use std::path::Path;
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_edgebridge-server"))
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .args(args)
        .output()
        .expect("run server CLI")
}

#[test]
fn configuration_validation_succeeds_without_starting_capture() {
    let output = run(&["--check-config", "--config", "config/server.example.toml"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

#[test]
fn invalid_or_unreadable_configuration_is_an_error() {
    for path in [
        "config/client.example.toml",
        "crates/server/tests/missing-config.toml",
    ] {
        let output = run(&["--check-config", "--config", path]);
        assert_eq!(output.status.code(), Some(78), "{path}: {output:?}");
    }
}

#[test]
fn capture_requires_an_unambiguous_explicit_mode() {
    for args in [
        vec![],
        vec!["--config", "config/server.example.toml"],
        vec!["--duration-secs", "1"],
        vec!["--check-config"],
        vec!["--check-config", "--config"],
        vec![
            "--capture-log",
            "--check-config",
            "--config",
            "config/server.example.toml",
        ],
        vec!["--capture-log", "--config", "config/server.example.toml"],
        vec!["--capture-log", "--allow-insecure-lan"],
        vec!["--capture-log", "--capture-log"],
        vec!["--capture-log", "--unknown"],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(64), "{args:?}: {output:?}");
    }
}

#[test]
fn invalid_duration_cannot_start_capture() {
    for value in ["0", "-1", "1.5", "no", "18446744073709551616", "--help"] {
        let output = run(&["--capture-log", "--duration-secs", value]);
        assert_eq!(output.status.code(), Some(64), "{value}: {output:?}");
    }
    let output = run(&["--capture-log", "--duration-secs"]);
    assert_eq!(output.status.code(), Some(64), "{output:?}");
}

#[cfg(not(windows))]
#[test]
fn non_windows_capture_reports_unavailable_instead_of_succeeding() {
    let output = run(&["--capture-log", "--duration-secs", "1"]);
    assert_eq!(output.status.code(), Some(69), "{output:?}");
}
