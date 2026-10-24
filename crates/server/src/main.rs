//! EdgeBridge Windows server. 현재(M0)는 config 검증만 수행한다.
//!
//! usage: edgebridge-server --config <path> [--allow-insecure-lan]

use std::process::ExitCode;

use edgebridge_core::config::ServerConfig;

fn main() -> ExitCode {
    let mut config_path = None;
    let mut allow_insecure_lan = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => config_path = args.next(),
            "--allow-insecure-lan" => allow_insecure_lan = true,
            other => {
                eprintln!("unknown argument `{other}`");
                return ExitCode::from(64);
            }
        }
    }
    let Some(path) = config_path else {
        eprintln!("usage: edgebridge-server --config <path> [--allow-insecure-lan]");
        return ExitCode::from(64);
    };

    let config = match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|t| ServerConfig::from_toml(&t).map_err(|e| e.to_string()))
        .and_then(|c| c.validate(allow_insecure_lan).map_err(|e| e.to_string()))
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::from(78);
        }
    };
    println!(
        "config ok: bind {} ({:?}), {} <- left of {}",
        config.raw.network.bind,
        config.raw.network.security,
        config.raw.layout.left,
        config.raw.layout.local
    );

    if !cfg!(windows) {
        eprintln!("edgebridge-server runs on Windows only; config was validated");
        return ExitCode::from(69);
    }
    eprintln!("capture backend is not implemented yet (M2)");
    ExitCode::from(69)
}
