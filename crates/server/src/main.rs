//! Windows capture diagnostics and offline server configuration validation.

use std::process::ExitCode;
use std::time::Duration;

use edgebridge_core::config::ServerConfig;

const USAGE: &str = "usage:
  edgebridge-server --capture-log [--duration-secs <positive integer>]
  edgebridge-server --check-config --config <path> [--allow-insecure-lan]
  edgebridge-server --help

--capture-log explicitly logs keyboard and mouse events on Windows.
Local input is not blocked. No network listener or Mac forwarding is started.
Use only test input while capturing. Stop with Ctrl-C or --duration-secs.
--check-config validates the future network configuration without capturing input.";

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Help,
    CheckConfig {
        path: String,
        allow_insecure_lan: bool,
    },
    Capture {
        duration: Option<Duration>,
    },
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Action, String> {
    let mut args = args.into_iter();
    let mut check_config = false;
    let mut capture_log = false;
    let mut config_path = None;
    let mut allow_insecure_lan = false;
    let mut duration = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(Action::Help),
            "--check-config" if !check_config => check_config = true,
            "--capture-log" if !capture_log => capture_log = true,
            "--allow-insecure-lan" if !allow_insecure_lan => allow_insecure_lan = true,
            "--config" if config_path.is_none() => {
                let path = args.next().ok_or("--config requires a path")?;
                if path.is_empty() || path.starts_with("--") {
                    return Err("--config requires a path".into());
                }
                config_path = Some(path);
            }
            "--duration-secs" if duration.is_none() => {
                let value = args
                    .next()
                    .ok_or("--duration-secs requires a positive integer")?;
                let seconds = value
                    .parse::<u64>()
                    .ok()
                    .filter(|seconds| *seconds > 0)
                    .ok_or("--duration-secs requires a positive integer")?;
                duration = Some(Duration::from_secs(seconds));
            }
            other => return Err(format!("unknown or repeated argument `{other}`")),
        }
    }
    match (check_config, capture_log) {
        (true, false) if duration.is_none() => Ok(Action::CheckConfig {
            path: config_path.ok_or("--check-config requires --config <path>")?,
            allow_insecure_lan,
        }),
        (false, true) if config_path.is_none() && !allow_insecure_lan => {
            Ok(Action::Capture { duration })
        }
        _ => Err("choose --capture-log (with optional --duration-secs), or --check-config --config <path> (with optional --allow-insecure-lan)".into()),
    }
}

fn main() -> ExitCode {
    let action = match parse(std::env::args().skip(1)) {
        Ok(action) => action,
        Err(error) => {
            eprintln!("{error}\n{USAGE}");
            return ExitCode::from(64);
        }
    };
    match action {
        Action::Help => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Action::CheckConfig {
            path,
            allow_insecure_lan,
        } => {
            match std::fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|text| ServerConfig::from_toml(&text).map_err(|error| error.to_string()))
                .and_then(|config| {
                    config
                        .validate(allow_insecure_lan)
                        .map_err(|error| error.to_string())
                }) {
                Ok(config) => {
                    println!(
                        "config ok: bind {} ({:?}), {} <- left of {}",
                        config.raw.network.bind,
                        config.raw.network.security,
                        config.raw.layout.left,
                        config.raw.layout.local
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{path}: {error}");
                    ExitCode::from(78)
                }
            }
        }
        Action::Capture { duration } => capture(duration),
    }
}

#[cfg(not(windows))]
fn capture(_duration: Option<Duration>) -> ExitCode {
    eprintln!("--capture-log requires Windows; no input was captured");
    ExitCode::from(69)
}

#[cfg(windows)]
fn capture(duration: Option<Duration>) -> ExitCode {
    match run_capture(duration) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("capture failed: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn run_capture(duration: Option<Duration>) -> std::io::Result<()> {
    use edgebridge_platform_windows::{CaptureRecvError, CaptureSession};
    use std::io::{self, Write};
    use std::time::Instant;

    eprintln!("capture-log: keyboard and mouse events will be printed; use test input only");
    eprintln!("local input passes through; no network listener; Ctrl-C stops capture");
    let mut session = CaptureSession::start()?;
    let stop = session.stop_handle();
    ctrlc::set_handler(move || {
        if let Err(error) = stop.request_stop() {
            eprintln!("could not stop capture: {error}");
        }
    })
    .map_err(io::Error::other)?;

    let started = Instant::now();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let result = (|| -> io::Result<()> {
        loop {
            let timeout = match duration {
                Some(limit) => match limit.checked_sub(started.elapsed()) {
                    Some(remaining) if !remaining.is_zero() => {
                        remaining.min(Duration::from_millis(250))
                    }
                    _ => break,
                },
                None => Duration::from_millis(250),
            };
            match session.recv_timeout(timeout) {
                Ok(event) => writeln!(output, "{event:?}")?,
                Err(CaptureRecvError::Timeout) => {}
                Err(CaptureRecvError::Stopped) => break,
            }
        }
        output.flush()
    })();
    let stopped = session.stop();
    match (result, stopped) {
        (result, Ok(stats)) => {
            eprintln!("capture stopped: {stats:?}");
            result?;
            if stats.has_errors() {
                return Err(io::Error::other(
                    "capture reported errors; see final statistics",
                ));
            }
            Ok(())
        }
        (Err(output_error), Err(stop_error)) => Err(io::Error::other(format!(
            "output: {output_error}; cleanup: {stop_error}"
        ))),
        (Ok(()), Err(error)) => Err(error),
    }
}
