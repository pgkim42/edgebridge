# 아키텍처

상태: Design v0.1 / 2026-10-24. 규범적 wire 정의는 [PROTOCOL](PROTOCOL.md), 선택의 근거는 [DECISIONS](DECISIONS.md). `Local`, `Entering`, `Remote`, `Leaving`은 제어 상태이며 TCP 연결 상태와 별개다.

## 1. 범위와 불변식

Windows = 입력 제공 server/listener, Mac = 입력 수신 client/connector. MVP는 로그인된 interactive desktop, 일반 GUI 앱, 선택된 모니터 하나씩을 대상으로 한다. Mac 내장 키보드/트랙패드는 비상 조작에 남겨 둔다. 두 사용자가 동시에 입력하는 협업 모드는 지원하지 않는다. 게임의 raw/exclusive input, UAC secure desktop, 로그인 화면, 영상·오디오·파일 전송은 MVP 밖이다.

1. 연결·권한·화면 정보가 준비되지 않으면 server는 `Local`이다.
2. 물리 입력의 소유권 전환을 결정하는 곳은 Windows control thread 하나다. Mac은 edge crossing을 판단하지 않는다.
3. 정상 Enter/Leave는 물리 key/button held set이 비어 있을 때만 시작한다. 화면 사이에 drag나 modifier hold를 넘기지 않는다.
4. 실패하면 연결 유지보다 로컬 제어권 복원을 우선한다. 옛 입력을 reconnect 후 재생하지 않는다.
5. client는 연결+epoch별로 자신이 주입한 key/button ledger를 갖는다. release는 이 ledger만 대상으로 하고 Mac 전체 키 상태를 임의 초기화하지 않는다.
6. key/button edge를 조용히 버리지 않는다. 큐 포화·잘못된 순서·오래된 입력은 세션 중단으로 처리한다.
7. 일반 로그에는 키 값, 입력 문자열, clipboard 내용, pairing 비밀을 남기지 않는다. M2의 명시적 진단 모드에서만 테스트 키를 기록한다.

## 2. workspace (M0에서 생성)

```text
crates/
  protocol/          package edgebridge-protocol: frame codec, DTO, 제한, golden bytes
  core/              package edgebridge-core: 순수 상태 머신, 좌표, HID, config 의미 검증
  platform-windows/  package edgebridge-platform-windows: hooks, Raw Input, 화면/세션
  platform-macos/    package edgebridge-platform-macos: Quartz inject, TCC, 화면/세션
  server/            binary edgebridge-server: Windows 조립, TCP listen, config
  client/            binary edgebridge-client: macOS 조립, TCP connect, config
  test-peer/         binary edgebridge-test-peer: fake server/client, fault script
```

`protocol`은 OS/Tokio에 의존하지 않는다. `core`는 `protocol`의 값 타입을 사용할 수 있으나 FFI를 호출하지 않는다. OS backend는 `core`의 이벤트/효과 타입을 받아 실행한다. server/client만 network와 platform을 조립한다. `test-peer`는 공통 codec을 사용하지만 독립 golden byte fixture도 둬 양쪽의 같은 버그가 상쇄되지 않게 한다. OS dependency는 Cargo target dependency 및 `#[cfg(target_os = "...")]`로 제한한다. Mac에서 Windows backend를 compile한다고 hook 동작이 검증된 것은 아니다.

작은 경계 스케치:

```rust
// 설계용 타입: 실행 가능한 구현이 아님
struct HidUsage { page: u16, usage: u16 }
struct Position { x: f64, y: f64 } // 선택된 Mac 화면 내부, Quartz 좌표 단위
// core: (State, Event) -> (State, Effects)
// platform: CaptureEvent 생성 / InjectCommand 실행 / 완료·오류 보고
```

## 3. 실행 모델과 순서

```text
Windows physical input
  | WH_KEYBOARD_LL / WH_MOUSE_LL       | WM_INPUT (motion only)
  v                                   v
전용 Win32 control thread: hidden window + GetMessage/DispatchMessage
  callback: preallocated try_enqueue + held bits + suppression latch 판정
  drain: core 상태 머신, 가상 커서, modifier 전송 전 물리 상태
  | bounded outbound                         ^ inbound command
  v                                          |
Tokio: single writer + single reader + connection supervisor
  | TCP_NODELAY / M7 TLS                      | ACK / heartbeat
  +---------------------- LAN ---------------+
                                             |
Mac Tokio reader --> bounded command queue --> main run loop / injection actor
Mac Tokio writer <-- 결과·Pong·ACK queue <----- ledger + CGEventPost
```

hook callback에서 상태 머신 전체·네트워크·로그·파일 IO·동적 할당·mutex 대기를 실행하지 않는다. `nCode < 0`는 바로 다음 hook. 꼭 필요한 고정 크기 held bit update, injected 판별, escape chord 판별, lease 검사와 try-enqueue만 한다. 큐 포화 시 suppression latch를 즉시 내려 놓고 abort flag를 세운다. drain은 한 번에 제한된 개수만 처리하여 message loop를 굶기지 않는다.

Local edge에서 클릭이 새기지 않도록 callback에 미리 계산된 edge 조건/ready snapshot을 제공한다. 적격한 왼쪽 edge 이벤트를 본 callback이 provisional suppression latch를 먼저 올리고 `BeginEnter`를 큐에 넣는다. 이후 이벤트는 Entering 버퍼로 간다. drain이 조건을 재검증하고 Enter를 전송한다. 이 좁은 원자적 gate 없이 Tokio가 뒤늦게 Remote로 바꾸면 화면을 넘은 직후 클릭이 Windows에 들어갈 수 있다.

Raw movement와 LL button 이벤트의 OS 간 전역 순서는 보장된 것으로 가정하지 않는다. 같은 control thread에서 수신한 순서를 capture order로 사용한다. LL button의 물리 위치는 Remote에서 가상 위치가 아니다. M2/M3에서 빠른 이동 직후 클릭의 오차를 측정한다. 허용 오차를 넘으면 mouse movement/button/wheel을 Raw Input의 단일 경로로 모으는 재설계가 필요하다. 이 경우 LL은 억제만 하며 이중 전송하지 않는다.

Mac은 main run loop 위의 단일 injection actor로 순서를 직렬화한다. AppKit 사용 시 main-thread 제약과 permission UI를 수용하기 위한 선택이며 CGEventPost 자체가 반드시 main-thread라는 뜻은 아니다. network task는 OS 객체를 공유하지 않고 소유권이 명확한 값만 전달한다. actor가 처리한 후에 EnterAck/LeaveAck/Pong을 만든다. 네트워크만 살아 있는 상태를 정상으로 보지 않는다.

## 4. 좌표계와 edge 규칙

### 단위

- Windows local: Per-Monitor V2 DPI-aware process에서 얻는 screen physical pixel 좌표. 선택 모니터 rect `(L,T,Ww,Hw)`; 음수 origin 가능.
- Mac: `CGDisplayBounds`와 CGEvent 위치가 사용하는 global Quartz 좌표. wire에서는 선택 화면 origin을 뺀 local 좌표 `(x,y)`. backing pixel과 다르며 Retina 배율을 두 번 적용하지 않는다.
- 화면 정보는 logical rect와 pixel 크기, x/y scale을 함께 보고한다. server는 이동에 logical 크기만 쓴다. scale은 진단/검증용이다. `NSScreen.frame`의 AppKit 좌표를 그대로 섞지 않는다.
- Raw delta는 device count다. pixel도 point도 아니다. `dx * sensitivity`, `dy * sensitivity`로 Mac logical unit에 변환한다. MVP sensitivity=1.0, 가속 없음. Windows의 pointer acceleration을 복제하지 않는다. 실제 mouse DPI에 맞춘 보정은 M3.

### Windows → Mac

선택 Windows 화면의 **외부 왼쪽 경계**에 Mac을 둔다. MVP는 Windows 모니터 하나. 다중 모니터가 그 경계를 가리는 경우 설정을 거부한다. Local에서 OS 위치가 `x <= L+1`이고 직전 위치보다 왼쪽으로 이동한 이벤트에만 진입한다. edge에 정지했거나 programmatic warp한 이벤트는 진입하지 않는다. 최초 위치가 이미 edge면 먼저 안으로 이동해야 한다.

Mac 크기를 `(Wm,Hm)`이라 하면 `u=clamp((win_y-T)/(Hw-1),0,1)`, entry=`(Wm-2, u*(Hm-1))`. 모든 화면 dimension은 16 이상이어야 한다. 높이가 다르면 전체 높이를 비례 대응한다. 물리 높이 정렬은 stretch goal이다. Entering 동안 이동 delta는 bounded buffer에 두고 ACK 이후 entry부터 누적한다. edge 이전 Raw delta를 재사용하지 않는다.

### Mac → Windows

서버가 `candidate = virtual + converted_delta`를 계산한다. 왼쪽/위/아래는 `[0,Wm-1] × [0,Hm-1]`에 clamp. 오른쪽 `candidate.x >= Wm`이고 모든 키/버튼이 up이면 Leave. held가 있으면 `x=Wm-1`로 clamp하고 drag/typing을 Mac에서 계속한다. release 후 **새로운 오른쪽 이동**이 있어야 복귀한다. 큰 delta로 여러 화면을 한 번에 건너뛰지 않는다.

복귀 위치는 `win_x=L+2`, `win_y=T+clamp(virtual_y/(Hm-1),0,1)*(Hw-1)`. Windows cursor warp 후 Local. 재진입은 커서가 `L+8` 이상 안으로 한번 이동한 뒤 다시 왼쪽으로 갈 때만 arm한다. 이동·warp로 인한 bounce를 막는다. 강제 복귀도 같은 re-arm 규칙을 쓴다.

Mac에 보이는 실제 cursor를 Mac 사용자가 옮겨도 server virtual cursor를 갱신하지 않는다. 다음 remote movement가 가상 위치로 되돌린다. passive cursor readback을 관측 목적으로 추가할 수 있으나 로컬 사용자 동시 조작은 지원하지 않는다.

## 5. server 상태 머신

연결 상태는 `Disconnected → Handshaking → Ready`, 실패 시 `Disconnected`. 아래 상태는 `Ready`가 아니면 반드시 `Local`로 수렴한다.

```text
Local --eligible left edge / Enter(epoch)--> Entering
Entering --EnterAck / drain buffer--------> Remote
Remote --right crossing, all up / Leave---> Leaving
Leaving --LeaveAck / warp, unlatch--------> Local
Entering/Remote/Leaving --fault/escape----> Local + close connection
```

| 상태 | Windows 전달 | Mac 입력 | 종료 조건 |
|---|---|---|---|
| Local | 원래대로 통과 | 없음 | ready + armed edge + all up |
| Entering | 억제; epoch buffer | Enter만 | 500ms 이내 ACK; 아니면 abort |
| Remote | 억제 | epoch input | 정상 right edge 또는 fault |
| Leaving | 억제; 새 입력은 버리지 않고 abort 원인 | Leave만; 이전 입력 drain | ACK 후 복귀; 500ms 초과 abort |

Entering buffer는 최대 256개, oldest 100ms 제한. 500ms ACK 한도와 별도로 입력이 쌓인 채 100ms가 지나면 즉시 abort한다. Leaving 중 새 물리 key/button/motion이 오면 기다린 입력을 재생하지 않고 연결을 닫고 Local로 복귀한다. 그 이벤트는 가능하면 현재 callback부터 local 통과시키며 이전에 억제한 down을 합성하지 않는다. 따라서 매우 빠른 전환 중 짧은 입력이 사라질 수 있다. 오작동할 수 있는 클릭을 나중에 재생하는 것보다 이 동작을 택한다.

정상 Enter는 client가 권한 확인·old ledger release·entry 위치 post를 마친 후 ACK한다. 정상 Leave는 client가 앞선 입력을 처리하고 ledger release 후 ACK한다. ACK는 OS post 호출 완료이지 대상 앱이 이벤트를 소비했다는 보장은 아니다.

## 6. 실패와 복구

| 원인 | Windows | Mac |
|---|---|---|
| EOF/RST, malformed frame, write failure | 즉시 latch off, Local, old queues 폐기 | 즉시 release ledger, inactive, socket close |
| 케이블/방화벽 blackhole | heartbeat lease 만료(1s 목표 상한) 후 Local | 자기 peer lease 만료 후 release |
| ctrl+alt+escape | callback에서 latch off, epoch 폐기, 연결 close | EOF 또는 lease 만료 시 release |
| slow consumer/queue full/oldest >100ms | key edge를 버리지 않고 abort | pending injection 폐기 후 release |
| sleep/lock/display 변경 | suspend 전에 Local, 연결 종료 | release 시도, 연결 종료; wake 후 재-handshake |
| client 재시작 | old TCP/epoch 폐기, 자동 remote 재개 금지 | 새 session, 빈 ledger, permission 재확인 |
| hook callback hang/삭제 | 완전 탐지 불가. lease/전용 thread로 위험 축소 | peer timeout으로 release |
| client crash/kill -9 | peer failure 시 Local | cleanup 실행 불가; 아래 한계 참조 |

Windows는 억제 판단마다 monotonic lease deadline을 직접 검사한다. Tokio가 멈춰도 새 callback이 만료된 lease를 보고 통과시킨다. 별도 watchdog은 25ms 간격으로 thread progress/lease를 보고 latch를 내린다. OS scheduling 지연 때문에 hard real-time 상한은 보장하지 않는다. watchdog이 큐로만 해제 요청을 보내는 설계는 stalled hook thread를 구제하지 못한다.

Mac injection actor는 25ms timer로 lease를 검사한다. Ping/Pong은 actor/control thread 경유라 그 thread가 막히면 갱신되지 않는다. graceful 종료에서는 best-effort Disconnect를 최대 100ms 시도하되 local release/unlatch를 먼저 한다. reconnect backoff는 0.5, 1, 2, 4, 8초 + ±20% jitter, Ready 10초 유지 후 reset. 인증/권한/config 오류는 무한 재시도하지 않고 사용자 조치를 기다린다.

**release 한계:** CGEventPost는 성공 여부를 반환하지 않고 OS 권한 철회·프로세스 강제 종료에서는 release가 실행/적용되지 않을 수 있다. 다음 실행 시 과거 key를 무조건 up하는 것도 Mac 로컬 입력을 손상할 수 있다. ledger journaling을 복구 보증으로 취급하지 않는다. M6에서 강제 종료 후 실제 OS 상태를 확인하고, 필요하면 별도 watchdog helper를 설계한다. Mac 내장 입력으로 해당 modifier/button을 눌렀다 놓기, 앱 종료, 사용자 세션 재로그인을 수동 복구 수단으로 안내한다.

정상 전환에서는 all-up 정책으로 Windows에 내려간 down의 up이 Mac으로 넘어가는 문제를 막는다. emergency는 all-up을 기다리지 않는다. Mac release 후 Windows에 누른 상태를 재주입하지 않으며 사용자는 키를 놓고 다시 누른다. 이후 고아 up이 Local에 통과할 수 있으나 down replay는 하지 않는다.

## 7. 큐·관측·테스트

capture ring 2048 slots, network outbound 512 frames, injection queue 512 commands를 초기 상한으로 둔다. 단순 개수보다 oldest age 100ms가 우선이다. 숫자는 M3 측정 후 조정한다. MouseMove는 **연속 이동끼리만** 마지막 절대 위치로 coalesce한다. Key/Button/Scroll/Enter/Leave/Ping은 barrier다. click 앞의 이동을 뒤로 넘기지 않는다. coalescing은 wire seq 부여 전에 수행한다. 고속 마우스는 최대 250Hz move 송신으로 시작하되 버튼 전에는 마지막 위치를 즉시 flush한다. 아주 세밀한 drag path가 필요한 앱은 coalescing 감소 옵션을 추후 검토한다.

관측: state transition reason, queue depth/oldest age, hook duration histogram, RTT, frame count, release count, unsupported usage count. wall-clock으로 양쪽 one-way latency를 계산하지 않는다. RTT와 각 기기의 monotonic 구간을 따로 보고, end-to-end는 외부 카메라/실기기 측정으로 확인한다.

- 순수 테스트: edge 수식, 음수 origin, 불균일 DPI, hysteresis, held 차단, escape, epoch, 모든 fault → Local 및 release 효과. fake clock으로 timeout 경계 전후 확인.
- codec: chunked read, 여러 frame 합쳐 읽기, 최대 길이, invalid bool/NaN/tag/version/seq, EOF 중간, fuzz. OS injection 없이 수행.
- fake server: Mac 이동/클릭/drag/keyboard/EOF/partial frame/blackhole script. fake client: Windows ACK 지연·queue stop·화면 invalid 재현.
- Windows+Mac 실기기: 100회 왕복, Ctrl/Shift held 중 끊기, 마우스 button held 중 끊기, sleep/wake, permission 철회, 프로세스 crash. OS 상태까지 확인한다.
- loopback은 framing/순서 테스트이며 네트워크 지연·OS hook·Retina 검증의 대체가 아니다.

## 8. TOML 제안

아래는 문서 예시이며 현재 parser나 config 파일은 없다. OS별 `--config` 경로를 명시하는 것으로 시작하고 자동 검색은 뒤로 미룬다.

```toml
# Windows 예시
role = "server"
[network]
bind = "127.0.0.1:24810" # 실기기 실험 시 해당 private NIC IP로 명시 변경
security = "plaintext-dev" # M7 이후 기본 tls-pinned; 자동 fallback 없음
[layout]
local = "windows"
left = "mac" # 역방향 mac.right=windows는 자동 도출
[screen]
selector = "primary" # stable ID 지원은 후속; 세션 내 실제 ID를 고정
[input]
sensitivity = 1.0
escape_hotkey = "LeftCtrl+LeftAlt+Escape" # remap 전 물리 키
```

```toml
# Mac 예시
role = "client"
name = "mac"
[network]
server = "192.168.1.10:24810" # 예시 주소
security = "plaintext-dev"
[screen]
selector = "builtin"
[modifiers]
profile = "physical" # 또는 ctrl-cmd-swap; KEYMAP 참조
[scroll]
invert_x = false
invert_y = false
```

unknown field, 중복 screen name, 자기 자신을 neighbor로 지정, invalid hotkey, 비유한 sensitivity 또는 0 이하, 선택 화면 미발견은 시작 오류다. runtime hot reload는 MVP에서 금지한다. mapping/화면/보안 설정 변경은 release + reconnect 후 적용한다. plaintext-dev는 loopback 외 bind/connect에서 별도 `--allow-insecure-lan` 명시가 필요하다. 서비스 자동 시작과 clipboard는 이 모드에서 비활성화한다.
