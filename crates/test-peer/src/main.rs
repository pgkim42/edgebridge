//! usage:
//!   edgebridge-test-peer fake-server [--bind 127.0.0.1:24810] [--pointer-only] [--hold-ms 2000]
//!   edgebridge-test-peer fake-client [--connect 127.0.0.1:24810] [--pointer-only]
//!
//! loopback 전용. 실제 입력을 캡처하거나 주입하지 않는다.

use std::net::{SocketAddr, TcpListener};
use std::process::ExitCode;
use std::time::Duration;

use edgebridge_protocol::Capabilities;
use edgebridge_test_peer::{
    ClientOptions, ServerOptions, run_fake_client, run_fake_server, sample_mac_screen,
};

const DEFAULT_ADDR: &str = "127.0.0.1:24810";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(mode) = args.next() else {
        eprintln!("usage: edgebridge-test-peer <fake-server|fake-client> [options]");
        return ExitCode::from(64);
    };
    let mut addr: SocketAddr = DEFAULT_ADDR.parse().expect("default addr");
    let mut caps = Capabilities::ALL_V1;
    let mut hold = Duration::from_secs(2);
    while let Some(arg) = args.next() {
        let value = |args: &mut dyn Iterator<Item = String>| args.next().unwrap_or_default();
        match arg.as_str() {
            "--bind" | "--connect" => match value(&mut args).parse() {
                Ok(a) => addr = a,
                Err(e) => return usage(&format!("bad address: {e}")),
            },
            "--pointer-only" => caps = Capabilities::POINTER,
            "--hold-ms" => match value(&mut args).parse() {
                Ok(ms) => hold = Duration::from_millis(ms),
                Err(e) => return usage(&format!("bad --hold-ms: {e}")),
            },
            other => return usage(&format!("unknown argument `{other}`")),
        }
    }
    if !addr.ip().is_loopback() {
        return usage("test-peer is loopback-only (plaintext)");
    }

    let result = match mode.as_str() {
        "fake-server" => TcpListener::bind(addr)
            .map_err(Into::into)
            .and_then(|listener| {
                println!("fake-server listening on {addr}");
                let opts = ServerOptions {
                    name: "windows".into(),
                    expected_peer: "mac".into(),
                    capabilities: caps,
                    hold,
                };
                run_fake_server(&listener, &opts)
            }),
        "fake-client" => {
            let opts = ClientOptions {
                name: "mac".into(),
                capabilities: caps,
                screen: sample_mac_screen(),
            };
            run_fake_client(addr, &opts)
        }
        other => return usage(&format!("unknown mode `{other}`")),
    };
    match result {
        Ok(report) => {
            println!(
                "ok: peer `{}`, caps {:#06b}, pongs {}, pings answered {}, {:?}",
                report.established.peer_name,
                report.established.capabilities.0,
                report.pongs_received,
                report.pings_answered,
                report.ended_by
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage(msg: &str) -> ExitCode {
    eprintln!("{msg}");
    ExitCode::from(64)
}
