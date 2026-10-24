//! EdgeBridge macOS client. M1 pointer-only client.
//!
//! usage: edgebridge-client --config <path> [--allow-insecure-lan] [--request-permission] [--observe-pointer]

use std::process::ExitCode;

use edgebridge_core::config::ClientConfig;

fn main() -> ExitCode {
    let mut config_path = None;
    let mut allow_insecure_lan = false;
    let mut prompt_permission = false;
    let mut observe_pointer = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => config_path = args.next(),
            "--allow-insecure-lan" => allow_insecure_lan = true,
            "--request-permission" => prompt_permission = true,
            "--observe-pointer" => observe_pointer = true,
            other => {
                eprintln!("unknown argument `{other}`");
                return ExitCode::from(64);
            }
        }
    }
    let Some(path) = config_path else {
        eprintln!(
            "usage: edgebridge-client --config <path> [--allow-insecure-lan] [--request-permission] [--observe-pointer]"
        );
        return ExitCode::from(64);
    };

    let config = match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|t| ClientConfig::from_toml(&t).map_err(|e| e.to_string()))
        .and_then(|c| c.validate(allow_insecure_lan).map_err(|e| e.to_string()))
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::from(78);
        }
    };
    println!(
        "config ok: `{}` -> {} ({:?}), modifiers {:?}",
        config.name, config.network.server, config.network.security, config.modifiers.profile
    );

    #[cfg(target_os = "macos")]
    {
        if config.network.security != edgebridge_core::config::Security::PlaintextDev {
            eprintln!("TLS is not implemented");
            return ExitCode::from(69);
        }
        match mac::run(config, prompt_permission, observe_pointer) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (prompt_permission, observe_pointer);
        eprintln!("client requires macOS");
        ExitCode::from(69)
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use edgebridge_platform_macos::{self as platform, Quartz};
    struct Backend {
        quartz: Quartz,
        observe: bool,
        pending: Option<(std::time::Instant, edgebridge_protocol::Position)>,
        selector: edgebridge_core::config::ClientScreenSelector,
        screen: edgebridge_protocol::ScreenInfo,
    }
    impl edgebridge_client::Backend for Backend {
        fn check(&mut self) -> Result<(), String> {
            if !platform::permission(false) {
                return Err("Accessibility/post-event access denied; no Ready/injection".into());
            }
            if platform::screen(self.selector).map_err(|e| e.to_string())? != self.screen {
                return Err("display changed".into());
            }
            Ok(())
        }
        fn inject(&mut self, command: edgebridge_core::session::Inject) -> Result<(), String> {
            self.quartz.inject(command).map_err(|e| e.to_string())?;
            if self.observe
                && let edgebridge_core::session::Inject::Move { position, .. } = command
            {
                self.pending = Some((std::time::Instant::now(), position));
            }
            Ok(())
        }
        fn idle(&mut self) {
            Quartz::idle();
            if let Some((at, target)) = self.pending
                && at.elapsed() >= std::time::Duration::from_millis(50)
            {
                self.pending = None;
                match self.quartz.cursor_position() {
                    Ok(actual) => eprintln!(
                        "pointer target={target:?} actual={actual:?} error={:.3}",
                        (target.x - actual.x).hypot(target.y - actual.y)
                    ),
                    Err(e) => eprintln!("{e}"),
                }
            }
        }
    }
    pub fn run(
        config: edgebridge_core::config::ClientConfig,
        prompt: bool,
        observe: bool,
    ) -> Result<(), String> {
        if !platform::permission(prompt) {
            return Err("Accessibility/post-event access denied; allow terminal or edgebridge-client in System Settings, then restart".into());
        }
        let screen = platform::screen(config.screen.selector).map_err(|e| e.to_string())?;
        eprintln!(
            "executable={:?}; screen={screen:?}",
            std::env::current_exe()
        );
        let mut backend = Backend {
            observe,
            pending: None,
            quartz: Quartz::new().map_err(|e| e.to_string())?,
            selector: config.screen.selector,
            screen,
        };
        edgebridge_client::run(config.network.server, config.name, screen, &mut backend)
    }
}
