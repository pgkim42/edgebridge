# 로드맵과 수락 기준

상태: M0 완료, M1 구현·자동 검증 완료 / 실기기 수락 대기 (2026-10-24). 이 문서는 완료 보고서가 아니다. milestone 완료는 코드 존재가 아니라 아래 관측 가능한 조건으로 판정한다. 개발 순서는 **Mac client → Windows capture → 연결 → keyboard → edge → fault → 보안 → clipboard**다.

## M0 — workspace skeleton / 계약 고정

Cargo workspace, 공통 DTO/config/error 경계와 fake peer. **2026-10-24 Mac에서 구현** (Windows 실기기 build는 미확인).

- [x] ARCHITECTURE §2의 crate 이름으로 skeleton 생성. Mac에서 전체 build/test 통과.
  - [ ] Windows 실기기에서 server build. 현재는 Mac에서 `cargo check --target x86_64-pc-windows-msvc`만 통과 (link/실행 미검증).
- [x] codec golden fixture 6개 (Ping, server/client Hello, MouseMove, Key, Scroll). Rust encoder와 독립적으로 Python `struct`로 계산한 bytes. fragmentation/concatenation, invalid length/magic/version/flags/kind/reserved kind/trailing/truncated/seq/enum 값/NaN 거부 — `crates/protocol/tests/`.
- [x] caps 축소 협상(Pointer-only), Ready 전 input 차단(BadState), Busy 거부, 늦은 handshake가 Ready 세션을 빼앗지 않음 — `crates/core/tests/handshake.rs`, loopback TCP — `crates/test-peer/tests/loopback.rs`.
- [x] TOML unknown field/잘못된 role/자기 자신 neighbor/빈 이름/잘못된 sensitivity/escape hotkey/LAN 평문 오류 — `crates/core/tests/config.rs`. "중복 screen 이름"은 v1 schema(local/left 두 이름)에서 자기 자신 neighbor 검사로 대체.
- [x] fake clock(`Millis`) lease/heartbeat/epoch 테스트 — `crates/core/src/heartbeat.rs`. `unsafe_code = "deny"`는 platform crate를 제외한 workspace 전체에 적용.
- [x] toolchain 1.96.0 고정(`rust-toolchain.toml`), `rust-version = 1.96`, `Cargo.lock` 커밋. build 명령은 README.

M0는 hook이나 permission을 실제로 시험한 단계가 아니다.

## M1 — fake server → Mac mouse injection

Mac loopback fake server가 listen, Mac client가 접속한다. 선택 화면·권한 handshake, 명시 Enter, 절대 이동, Leave를 먼저 구현한다.

- [ ] Accessibility 거부 시 Ready/주입 없음. 허용 후 정상 연결. Input Monitoring 요청 없음.
- [ ] center와 네 corner의 안쪽 좌표로 이동하고 관측 위치 오차 <=1 logical unit. Retina 기본/다른 scaled mode에서 각각 확인.
- [x] 화면 local/global origin 변환을 fake fixture로 검증. invalid screen/NaN/out-of-range는 OS post 전에 거부.
- [x] Enter 이전 move, 다른 epoch move가 주입되지 않고 session이 종료됨.
- [ ] fake server EOF/blackhole 뒤 inactive로 복귀. 100회 Enter/Leave에도 held ledger가 비어 있음.
- [ ] 실행 주체/TCC, OS/SDK/CPU, display logical/pixel 정보를 기록. 테스트 중 client를 안전하게 종료할 수 있음.

자동 검증: `core/tests/session.rs`의 100회 epoch/ledger·fake clock·범위 검사와 `client/tests/actor.rs`의 실제 TCP + recorder backend가 정상/클릭/EOF/blackhole/잘못된 epoch/권한 거부/actor barrier/오래된 큐를 확인한다. 위 체크된 항목은 순수·메모리 backend 증거이며 실제 Quartz 성공을 뜻하지 않는다. EOF/blackhole의 메모리 release는 통과했지만 해당 실기기 항목은 체크하지 않았다. OS/SDK/CPU는 [M1 검증 기록](M1-VERIFICATION.md)에 기록했고 TCC 귀속·display 정보·위치 오차·실제 종료 복구는 사용자 실험 대기다.

## M2 — Windows capture-only logging + Raw Input spike

이 단계에서는 normal run이 **입력을 억제하지 않는다**. 별도의 짧은 opt-in suppression 실험만 5초 자동 해제 watchdog을 붙여 수행한다.

- [ ] 일반 키/좌우 modifiers, buttons, signed wheel, relative motion을 관측. local 앱 입력은 그대로 동작.
- [ ] callback에서 할당/로그/IO/lock 대기가 없고 측정 ring으로 duration 수집. p99 <100µs 목표/최대·부하 함께 보고.
- [ ] hook thread message loop, uninstall/종료, injected filtering, keyboard repeat normalization 확인.
- [ ] Raw+LL 조합으로 억제 중에도 relative count가 지속됨. 빠른 move+click의 ordering 오차를 계측.
- [ ] relative/absolute device 구분, device unplug, foreground 전환, 125/1000Hz(가능하면 8000Hz) 검증.
- [ ] raw 방식 승인 또는 ADR-004 수정. 양쪽 mouse 이벤트 중복이 0임.

## M3 — end-to-end mouse

edge 대신 명시적 테스트 제어로 Enter/Leave한다. Windows→Mac 이동, click/drag/double-click/scroll 구현.

- [ ] Windows Local이면 Mac 입력 없음; Remote이면 일반 Windows editor/browser에서 클릭·wheel이 새지 않음.
- [ ] Left/Right/Middle, 지원 장치의 Back/Forward down/up이 균형. 100회 click/drag 후 ledger 0.
- [ ] TextEdit 선택 drag, Finder drag 취소, double-click 단어 선택, triple-click 동작을 확인.
- [ ] vertical/horizontal wheel의 방향과 fractional 누적 보존; line/pixel fake trace 각각 검증.
- [ ] 빠른 이동 직후 click target 오차 <=4 Mac logical units를 초기 목표로 측정. 초과 시 Raw 경로 통합 검토.
- [ ] 같은 LAN에서 RTT p95 <=20ms, queue age p99 <10ms를 초기 목표로 측정. 유/무선 조건·표본 10분 기록; one-way 지연으로 오표기하지 않음.
- [ ] 30분 연속 이동/클릭에도 메모리·큐가 증가하지 않음. TCP_NODELAY와 coalescing on/off 비교 기록.

## M4 — keyboard

- [ ] KEYMAP의 MVP 행 전부 fixture로 확인, text/physical 차이 문서화. 좌우 modifiers 각각 테스트.
- [ ] TextEdit/Terminal/browser에서 Shift+A, 복사/붙여넣기, navigation, shortcut 확인. physical/ctrl-cmd-swap 둘 다 시험.
- [ ] 5초 key hold: down 1개, repeat 다수, up 1개; 이중 autorepeat 없음. Repeat가 refcount를 늘리지 않음.
- [ ] Mac 한국어 input source에서 조합/삭제/enter 동작. 한영 physical 이벤트를 기록하고 LANG mapping 지원 여부 결정.
- [ ] modifier held 중 disconnect에서 1s lease + 정상 scheduling 여유 250ms 이내 release 관측. 실패 조건도 기록.
- [ ] Caps/IME/media/Fn의 지원·미지원이 UI/로그 진단에서 구분됨. 미지원이 임의 키로 주입되지 않음.

## M5 — 양방향 edge switching

- [ ] Windows 왼쪽 → Mac 오른쪽, Mac virtual 오른쪽 밖 → Windows 왼쪽 안쪽. 왼쪽/위/아래의 미설정 edge는 clamp.
- [ ] 서로 다른 높이에서 y 비례 mapping; Windows 100/150% DPI, Mac Retina scaled mode 확인.
- [ ] 100회 왕복 시 bounce/영구 suppression/held 누락 0. entry/leave ACK 지연 중 input이 local과 remote에 중복되지 않음.
- [ ] key/button hold 중 정상 전환 차단. 놓고 새 outward move 후 전환. emergency는 hold 여부와 무관하게 동작.
- [ ] edge에 놓고 시작, 매우 큰 delta, corner 대각선, warp-generated move, client 미연결/permission 없음에서 잘못 진입하지 않음.
- [ ] 다중 모니터가 선택 edge를 가리면 설정 오류. 지원하지 않는 모니터 배치를 조용히 잘못 처리하지 않음.

## M6 — robustness / stuck input / reconnect

- [ ] 네트워크 EOF/RST/blackhole/partial frame/trickle frame, input queue full, writer stall, actor stall을 주입하여 cleanup 관측.
- [ ] fault 인지 시 server latch 즉시 off. blackhole은 마지막 valid Pong 후 1s 목표, 정상 scheduling 환경의 실기기 관측 <=1.25s.
- [ ] LeftCtrl+LeftAlt+Escape를 50회 시도: network 응답 없이 local 복귀, escape 재진입 loop 없음.
- [ ] key/button down 중 각각 server/client 정상 종료 및 강제 kill 테스트. 보장할 수 없는 crash release를 결과에서 분리.
- [ ] Mac/Windows sleep/wake, lock/unlock, permission 철회, display detach/scale 변경 후 새 handshake. 옛 queue 재생 0.
- [ ] reconnect backoff, session/epoch 분리, 중복 client 거절. 재접속만으로 Remote 재개하지 않음.
- [ ] parser fuzz/모델 기반 상태 전이로 “Ready 아닌데 억제”, “inactive에서 inject”, “다른 epoch 입력 적용”이 없음.
- [ ] crash 후 modifier가 남는다면 별도 helper 또는 일상 사용 제한을 ADR로 결정. 복구가 불명확한 상태를 완료라고 부르지 않음.

## M7 — security / pairing

- [ ] TLS 1.3 상호 인증, 양쪽 고정 identity와 fingerprint pinning. 양 장비에서 지문을 대조한 pair만 Ready 가능.
- [ ] unknown/mismatched/revoked peer, 잘못된 key, expired identity 정책, MITM 테스트 실패 시 주입·억제 0.
- [ ] TLS 실패→plaintext fallback 없음. 인증 전 이름/화면만으로 연결 허용하지 않음. 0-RTT 비활성화.
- [ ] packet capture에서 input/clipboard 내용이 평문으로 보이지 않음. 트래픽 타이밍 정보까지 숨긴다고 주장하지 않음.
- [ ] key file 권한, 재-pair/폐기, 인증 오류 retry 제한, 로그 secret redaction 확인.
- [ ] native Windows/Mac crypto provider 빌드 확인. cargo-xwin이 실패해도 native Windows 경로 유지.
- [ ] 정상 모드 기본 TLS, plaintext-dev는 명시 opt-in·자동 시작 불가. 일상 사용 기준은 M6+M7 통과.

## M8 — clipboard (보안 이후)

- [ ] v1.1 schema/협상과 최대 크기를 먼저 확정. v1.0 peer에 clipboard 전송 금지.
- [ ] 명시 enable한 UTF-8 text만 양방향 동기화, NUL/invalid UTF-8/1MiB 초과 정책 테스트.
- [ ] Windows `AddClipboardFormatListener`/`WM_CLIPBOARDUPDATE`, `OpenClipboard`/`CF_UNICODETEXT`; Mac `NSPasteboard.changeCount` polling/main-thread 작업의 경쟁 처리.
- [ ] origin/change_id + 로컬 write echo 억제로 무한 ping-pong 없음. 동시 변경의 우선순위와 오래된 전송 취소 시험.
- [ ] 활성 화면의 변경만 기본 전파. 비활성 화면 변경은 덮어쓰지 않으며 다음 활성화 시 자동 publish 여부를 명시.
- [ ] disconnect/timeout 시 partial data 폐기, 사용자 clipboard를 부분 값으로 바꾸지 않음.
- [ ] bulk 전송 중 keyup/heartbeat 지연으로 timeout이 발생하지 않도록 chunk scheduling 또는 별도 인증 stream 검증.
- [ ] rich text/image/file/자동 secret 판별은 지원하지 않음. clipboard 내용 로그 없음.

## Stretch goals

1. Barrier compatibility adapter: 원문 완수에 필요하지만 MVP와 별도. 아래 대응표의 gap 전체를 구현하고 실제 Barrier 버전으로 검증.
2. 여러 monitor/computer, 비정형 layout, physical-height alignment, edge segment, 회전 화면.
3. held modifier/drag를 화면 사이로 넘기는 transfer transaction. 두 OS가 서로 다른 앱이므로 파일 drag-and-drop와 구분.
4. mDNS discovery(인증을 대체하지 않음), tray/menu-bar UI, 안정된 code signing/TCC identity, login agent.
5. Caps/NumLock sync, 한국어 dedicated toggle, media/Fn 가능한 subset, Windows 가속과 유사한 pointer curve.
6. screensaver notification, file transfer, Mac 로컬 입력 takeover. 추가 권한과 wire version 재검토.

## 원문 challenge 대응

[원문](https://codingchallenges.substack.com/p/coding-challenge-142-software-kvm)은 2026-10-24 HTTP 200으로 전문 확인했다. 아래는 단계 목적을 짧게 대응한 것이며 원문 프로토콜을 구현했다는 뜻이 아니다. 원문은 server를 먼저 만들고 실제 Barrier를 fixture로 쓰지만 우리는 개발 환경 때문에 client/fake peer부터 한다.

| 원문 단계 | EdgeBridge milestone | 차이/누락 |
|---|---|---|
| Step Zero: 환경·참고 구현 | M0 | 실제 Barrier capture는 호환 track에서 수행 |
| 1: 연결 handshake | M0–M1 | 자체 Hello/Ready, 24810; Barrier handshake 아님 |
| 2: 버전 거부 | M0 | 자체 Error Version 사용 |
| 3: 화면 배치 | M0, M5 | Windows↔왼쪽 Mac 한 쌍 |
| 4: 화면 정보 교환 | M1 | Hello ScreenInfo/Ready로 대체 |
| 5: 커서 모델 | M2–M3 | server virtual cursor |
| 6: edge 감지 | M5 | 왼쪽/오른쪽만 |
| 7: 제어권 이동 | M5 | Enter/Leave + ACK |
| 8: 키·repeat | M4 | HID; Barrier key model과 다름 |
| 9: 버튼·wheel | M3 | 자체 Button/Scroll |
| 10: 상대 이동 wire | M3 | 의도적 차이: absolute MouseMove |
| 11: 연결 유지·종료 | M1, M6 | 자체 Ping/Pong 250ms/1s; 원문 3s/9s와 다름 |
| 12: clipboard | M8 | UTF-8만; 호환 없음 |
| 13: screensaver | stretch | MVP 제외 |
| 14: 다중 화면 | stretch | MVP 제외 |
| 15: Barrier 상호운용 | 별도 compatibility track | 자체 protocol만으로 통과할 수 없음 |
| 16: client 구현 | M1–M6 | 순서 앞당김; Barrier server 연결은 별도 |
| Going Further | M5–M8 + stretch | DPI/재접속/TLS 등을 본 계획에 포함 |

### Barrier 호환을 원하면 필요한 별도 track

[Barrier protocol_types.h](https://github.com/debauchee/barrier/blob/master/src/lib/barrier/protocol_types.h)를 구현 시 특정 commit으로 고정해 읽고, 실제 Barrier server/client와 양방향 fixture를 만든다. 외부 message code를 내부 DTO 이름만 바꾸어 대체할 수 없다. handshake/version, key identifier/modifier/repeat 의미, screen info/enter sequence, relative motion, keepalive, clipboard ownership, SSL 설정을 각각 adapter로 번역해야 한다. EdgeBridge TLS/pairing과 Barrier SSL 인증도 별개다. 이 track을 선택하면 ADR-012를 수정하고 호환 테스트를 독립 gate로 추가한다.
