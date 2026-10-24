//! 네트워크는 Tokio, backend는 호출 스레드의 단일 actor에 격리한다.
use edgebridge_core::{
    Millis,
    handshake::{ClientHandshake, ClientStep, LocalHello},
    session::{ClientSession, Effect, Inject},
};
use edgebridge_protocol::*;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
    time::timeout,
};

pub trait Backend {
    fn check(&mut self) -> Result<(), String>;
    fn inject(&mut self, command: Inject) -> Result<(), String>;
    fn idle(&mut self) {
        std::thread::sleep(Duration::from_millis(5));
    }
}
type Envelope = (Instant, Message);
const AGE: Duration = Duration::from_millis(100);

async fn network(
    addr: std::net::SocketAddr,
    inbound: mpsc::Sender<Envelope>,
    mut outbound: mpsc::Receiver<Envelope>,
    abort: Arc<AtomicBool>,
) -> Result<(), String> {
    let stream = timeout(Duration::from_secs(5), TcpStream::connect(addr))
        .await
        .map_err(|_| "connect timeout")?
        .map_err(|e| e.to_string())?;
    stream.set_nodelay(true).map_err(|e| e.to_string())?;
    let (mut reader, mut writer) = stream.into_split();
    let read = async {
        let mut dec = FrameDecoder::new();
        let mut buffer = [0; 4096];
        let mut partial = None;
        loop {
            let n = if let Some(deadline) = partial {
                tokio::time::timeout_at(deadline, reader.read(&mut buffer))
                    .await
                    .map_err(|_| "frame assembly timeout".to_owned())?
            } else {
                reader.read(&mut buffer).await
            }
            .map_err(|e| e.to_string())?;
            if n == 0 {
                return Err::<(), String>("EOF".into());
            }
            if !dec.has_partial() {
                partial = Some(tokio::time::Instant::now() + Duration::from_secs(1));
            }
            dec.push(&buffer[..n]);
            while let Some(frame) = dec.next_frame().map_err(|e| e.to_string())? {
                inbound
                    .try_send((Instant::now(), frame.message))
                    .map_err(|_| "injection queue full/closed".to_owned())?;
                partial = if dec.has_partial() {
                    Some(tokio::time::Instant::now() + Duration::from_secs(1))
                } else {
                    None
                };
            }
        }
    };
    let write = async {
        let mut enc = FrameEncoder::new();
        while let Some((at, message)) = outbound.recv().await {
            if at.elapsed() > AGE {
                return Err("outbound queue too old".to_owned());
            }
            let bytes = enc.encode(&message).map_err(|e| e.to_string())?;
            timeout(Duration::from_millis(250), writer.write_all(&bytes))
                .await
                .map_err(|_| "write timeout".to_owned())?
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    };
    tokio::select! { result = read => result, result = write => result, _ = tokio::signal::ctrl_c() => { abort.store(true, Ordering::SeqCst); Err("interrupted".into()) } }
}
fn send(out: &mpsc::Sender<Envelope>, m: Message) -> Result<(), String> {
    out.try_send((Instant::now(), m))
        .map_err(|_| "outbound queue full/closed".into())
}
fn effects(
    backend: &mut impl Backend,
    session: &mut ClientSession,
    out: &mpsc::Sender<Envelope>,
    commands: Vec<Effect>,
) -> Result<(), String> {
    for effect in commands {
        match effect {
            Effect::Inject(command) => {
                backend.inject(command)?;
                session.committed(command);
            }
            Effect::Send(message) => send(out, message)?,
        }
    }
    Ok(())
}

/// 실제 backend 대신 recorder를 전달하면 권한/OS 입력 없이 전체 실행 경로를 검사할 수 있다.
pub fn run(
    addr: std::net::SocketAddr,
    name: String,
    screen: ScreenInfo,
    backend: &mut impl Backend,
) -> Result<(), String> {
    screen.validate().map_err(str::to_owned)?;
    backend.check()?;
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let (input_tx, mut input) = mpsc::channel(512);
    let (output, output_rx) = mpsc::channel(512);
    let abort = Arc::new(AtomicBool::new(false));
    let flag = abort.clone();
    let task = runtime.spawn(async move {
        let result = network(addr, input_tx, output_rx, flag.clone()).await;
        flag.store(true, Ordering::SeqCst);
        if let Err(error) = &result {
            eprintln!("network: {error}");
        }
        result
    });
    let clock = Instant::now();
    let now = || Millis(clock.elapsed().as_millis() as u64);
    let mut hs = ClientHandshake::new(
        LocalHello {
            name,
            capabilities: Capabilities::POINTER,
        },
        screen,
    );
    let mut session: Option<ClientSession> = None;
    let mut handshake_started = None;
    let mut last_tick = Instant::now();
    let mut last_health = Instant::now();
    let mut failure_message = Message::Disconnect(Disconnect {
        reason: Reason::Shutdown,
    });
    let result = (|| {
        loop {
            if abort.load(Ordering::SeqCst) {
                return Err("network closed or interrupted".into());
            }
            if session.is_none()
                && (clock.elapsed() > Duration::from_secs(8)
                    || handshake_started
                        .is_some_and(|at: Instant| at.elapsed() >= Duration::from_secs(3)))
            {
                return Err("handshake timeout".into());
            }
            if last_tick.elapsed() >= Duration::from_millis(25) {
                if let Some(s) = session.as_mut() {
                    let commands = s.tick(now()).map_err(|e| e.to_string())?;
                    effects(backend, s, &output, commands)?;
                }
                last_tick = Instant::now();
            }
            if last_health.elapsed() >= Duration::from_secs(1) {
                backend.check()?;
                last_health = Instant::now();
            }
            // bounded batch 대신 매 회 한 명령: timer/abort가 입력 flood에도 굶지 않는다.
            if let Ok((at, message)) = input.try_recv() {
                if at.elapsed() > AGE {
                    return Err("injection queue too old".into());
                }
                if let Some(s) = session.as_mut() {
                    if matches!(message, Message::Enter(_)) {
                        backend.check()?;
                    }
                    let commands = s.receive(message, now()).map_err(|e| {
                        failure_message = e.to_message();
                        e.to_string()
                    })?;
                    effects(backend, s, &output, commands)?;
                    if s.is_closed() {
                        return Ok(());
                    }
                } else {
                    handshake_started.get_or_insert(Instant::now());
                    match hs.on_message(message).map_err(|e| {
                        if let Some(m) = e.to_message() {
                            failure_message = m;
                        }
                        e.to_string()
                    })? {
                        ClientStep::Send(m) => send(&output, m)?,
                        ClientStep::ReadyReceived { echo, .. } => {
                            backend.check()?;
                            send(&output, echo)?;
                            session =
                                Some(ClientSession::new(screen, now()).map_err(|e| e.to_string())?);
                            eprintln!("Ready (inactive)");
                        }
                    }
                }
            } else {
                backend.idle();
            }
        }
    })();
    // 네트워크 상태와 무관하게 먼저 inactive + 자신이 실제 post한 버튼만 best-effort release.
    if let Some(s) = session.as_mut() {
        for effect in s.abort() {
            if let Effect::Inject(command) = effect {
                match backend.inject(command) {
                    Ok(()) => s.committed(command),
                    Err(e) => eprintln!("release failed: {e}"),
                }
            }
        }
        eprintln!("inactive; held ledger={}", s.held().len());
    }
    if result.is_err() && !abort.load(Ordering::SeqCst) {
        // cleanup 이후에만 종료 통지. writer는 독립 실행 중이며 100ms 이상 기다리지 않는다.
        let _ = send(&output, failure_message);
        drop(output);
        runtime.block_on(timeout_task(&task));
    }
    task.abort();
    result
}

async fn timeout_task(task: &tokio::task::JoinHandle<Result<(), String>>) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(100);
    while !task.is_finished() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}
