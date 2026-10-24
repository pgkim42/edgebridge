# 결정 기록 (ADR)

2026-10-24 작성. `Accepted / 설계 채택`은 구현이 검증되었다는 뜻이 아니다. `Proposed / 미정`은 실험 결과에 따라 변경할 후보이며 아래 gate를 통과하기 전 확정하지 않는다. 각 결정은 context/options/decision/consequences를 함께 남긴다.

## ADR-001 — Rust / Accepted

**상황:** Windows hook과 Mac Quartz FFI가 필요하며, 이벤트 처리 경로의 예측 가능성과 cross-platform 공통 로직이 중요하다.

**대안:** C++(기존 KVM 생태계/FFI 자연스러움), Go(개발 편의/GC·FFI 경계), Rust(메모리 안전성/명시적 unsafe/소유권).

**결정:** Rust. `#[cfg(target_os)]`와 target dependency, Windows `windows`, Mac Quartz/objc2 계열 bindings를 사용한다. wire/state는 순수 Rust로 분리한다. Tokio/serde 생태계를 활용한다.

**결과:** GC pause는 없지만 allocation, OS scheduling, locks, callback panic 위험은 남는다. Rust가 hard real-time을 보장하지 않는다. “단일 binary”는 OS별 실행 파일 하나라는 배포 목표이며 시스템 DLL/framework까지 제거한다는 뜻이 아니다. FFI unsafe 면적과 lifetime 검토가 필요하다. [windows-rs](https://github.com/microsoft/windows-rs), [objc2](https://github.com/madsmtm/objc2).

## ADR-002 — Windows server / Mac client / Accepted

**상황:** 물리 keyboard/mouse가 Windows에 연결되어 있고 Mac이 왼쪽에 있다. 개발은 Mac에서 한다.

**대안:** 역할 자동 전환, 양방향 capture/inject, 고정 역할.

**결정:** Windows는 capture/suppress/listen, Mac은 connect/inject. Mac client + fake server를 먼저 개발한다. 실제 역할과 구현 순서를 혼동하지 않는다.

**결과:** Mac Input Monitoring 없이 시작할 수 있다. Mac→Windows 입력 전달은 없지만 커서 제어권 복귀는 server virtual cursor로 구현한다. 역할 교환은 후속 별도 backend가 필요하다.

## ADR-003 — 상대 capture / 절대 wire / Accepted

**상황:** Windows cursor는 화면 edge에서 clamp된다. 원격 화면에서 언제 돌아올지 server가 알아야 한다.

**대안:** relative delta를 wire로 보내 client가 누적; absolute position을 wire로 보내 server가 단일 좌표를 소유; relative+periodic correction 혼합.

**결정:** raw counts→server sensitivity→Mac logical virtual position→absolute MouseMove. Button/Scroll에도 position 포함. Mac 화면 global origin은 client만 더한다.

**결과:** 위치 드리프트와 move coalescing이 단순해진다. TCP가 모든 frame을 보장해도 양쪽 가속/좌표 적용이 다르면 relative 모델은 달라질 수 있으므로 단일 권위를 택했다. Mac 사용자 동시 trackpad 조작은 지원하지 않는다. macOS 고유 pointer acceleration은 재현하지 않는다. 게임 relative-mouselook은 범위 밖. 원문 Step 10 상대 이동 wire 요구와 의도적으로 다르다.

## ADR-004 — Raw Input + low-level hooks / Proposed, M2 gate

**상황:** LL hook만으로 cursor position delta를 얻으면 edge에서 더 이상 움직임을 측정하기 어렵다.

**대안:** center pin/recenter, Raw Input relative motion + LL 억제, 전용 driver.

**결정 후보:** Raw Input은 motion, LL은 key/button/wheel과 suppression. center pin과 ClipCursor는 기본 경로에서 제외한다.

**결과:** device counts라 sensitivity가 필요하고 두 이벤트 소스의 ordering 오차가 있다. Raw Input은 억제 도구가 아니다. [Microsoft RAWMOUSE](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-rawmouse), [LowLevelMouseProc](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelmouseproc).

**확정 조건:** 억제 중 raw delivery, edge 지속 이동, 고속 polling, 빠른 move+click, foreground 변경을 M2/M3에서 통과. 실패하면 Raw mouse 버튼/휠 통합 또는 center pin으로 ADR을 갱신한다. 자동 fallback은 금지.

## ADR-005 — TCP 하나 + TCP_NODELAY / Accepted

**상황:** keyup/buttonup 손실·순서 변경은 stuck input을 만든다. 장비 두 대와 작은 traffic부터 시작한다.

**대안:** TCP, UDP+ACK/reorder/retry, TCP discrete+UDP motion, QUIC.

**결정:** 한 TCP stream, 양쪽 TCP_NODELAY, single writer, bounded queue/age timeout. heartbeat는 application layer.

**결과:** loss 시 head-of-line blocking은 남는다. UDP로 바꾸기 전에 유/무선 RTT/queue age/실제 drag를 측정한다. 복구 상태 머신을 분산시키지 않는 장점이 크다. M8 bulk가 keyup을 막으면 별도 authenticated stream을 재검토한다. [Tokio TcpStream](https://docs.rs/tokio/latest/tokio/net/struct.TcpStream.html).

## ADR-006 — fixed header + serde/postcard payload / Accepted

**상황:** Rust 양끝에서 작은 typed event를 교환하되 wire 규칙을 문서로 고정해야 한다.

**대안:** JSON(관측 쉬움/큰 payload), bincode(설정·버전 명시 필요), postcard(문서화된 작은 binary), 완전 수동 codec(명확하지만 작성량 증가).

**결정:** u32 BE length + 20-byte fixed header + kind별 postcard struct. kind tag는 수동 고정, enum ordinal 직렬화 금지, exact 1.0 version만 수용. protocol crate와 golden bytes를 함께 관리한다.

**결과:** postcard는 self-describing이 아니므로 필드 추가가 자동 호환되지 않는다. 배열/숫자/길이 제한을 명세대로 검증한다. dependency 버전은 M0 lockfile로 고정한다. [Postcard specification](https://postcard.jamesmunns.com/wire-format.html).

## ADR-007 — USB HID physical keys / Accepted (특수 키는 Proposed)

**상황:** Windows VK와 Mac vk는 같은 숫자 공간이 아니며 Unicode 문자만 보내면 shortcuts/IME가 깨진다.

**대안:** OS keycode 그대로, Unicode text, USB HID usage pair.

**결정:** `{page:u16,usage:u16}` physical HID. server scan normalization, client keymap/remap. 기본 physical modifier profile, ctrl-cmd-swap 선택 가능.

**결과:** 문자는 Mac input source에서 결정한다. 한영·Caps·media·Fn은 단순 표 변환으로 완결되지 않아 MVP 지원을 제한한다. 보유 중 mapping 변경 금지. many-to-one은 target refcount 필요. [USB-IF Usage Tables](https://www.usb.org/document-library/hid-usage-tables-17).

## ADR-008 — 단계적 보안, TLS 1.3 mutual authentication / Accepted 방향, pairing 세부 Proposed

**상황:** 평문 LAN input은 입력 내용 도청뿐 아니라 active MITM의 임의 입력 주입에 노출된다. private IP는 인증이 아니다.

**대안:** 평문 계속 사용, TLS 서버 인증만, mutual TLS + pinning, Noise + out-of-band PSK/pairing.

**결정:** M1–M6 실험은 loopback 기본의 명시 plaintext-dev. M7은 `rustls` + `tokio-rustls`, TLS 1.3, 양쪽 device identity를 수동 비교·pin하는 상호 인증. Mac은 server를, server는 Mac을 각각 인증한다. 서버만 인증하면 임의 client가 입력을 수신할 수 있으므로 불충분하다.

**결과:** 인증서/키 생성·보관·갱신·폐기 UX가 필요하다. Noise의 작은 handshake도 유력하지만 TCP stream 결합과 rustls 생태계를 택한다. PSK를 일반 TOML/chat/log에 넣지 않는다. LAN discovery는 identity verification을 대체하지 않는다. TLS는 입력 타이밍/traffic volume을 숨기지 않는다. [rustls](https://docs.rs/rustls/latest/rustls/).

**pairing 제안:** 양쪽 로컬에서 key/cert 생성, 공개 cert/fingerprint를 상대 장비로 수동 전달하고 직접 대조, 허용 목록에 저장. 첫 네트워크 연결의 cert를 묻지도 않고 신뢰하는 TOFU는 하지 않는다. pinning은 cert chain/name/유효기간 정책과 handshake signature 검증을 생략하는 “accept all verifier”로 구현하지 않는다. pin된 peer CA/leaf를 어떻게 구성할지 M7 spike에서 정하고 변경 cert는 재-pair한다. private key는 OS별 restricted file부터 시작, Keychain/Windows credential protection은 후속 검토.

## ADR-009 — Tokio + native event loops / Accepted

**상황:** socket/timer는 async가 유리하지만 hook message loop와 AppKit main thread는 별도 실행 제약이 있다.

**대안:** 전부 blocking thread, 전부 Tokio task, network async + native loops.

**결정:** Tokio는 socket/read/write/reconnect만, Windows 전용 control thread, Mac main-run-loop injection actor. bounded channels와 명시적 abort flag/lease를 연결한다.

**결과:** callback에서 await하지 않는다. channel과 epoch 종료 ordering을 테스트해야 한다. OS thread가 멈춘 상태에서 network Pong만 계속 보내지 않는다. 모든 native loop를 `spawn_blocking` pool에 무기한 넣지 않는다.

## ADR-010 — all-up handoff + fail-open Local / Accepted

**상황:** Windows에서 Down, Mac에서 Up을 보내면 Windows 앱에 key가 stuck될 수 있다. 드래그를 다른 컴퓨터 앱으로 넘기는 의미도 명확하지 않다.

**대안:** held state migration과 양쪽 synthetic release/repress, held 중 handoff 금지, 단순 forwarding.

**결정:** 정상 전환은 모든 physical key/button up에서만. emergency는 즉시 suppression off와 연결 폐기. Enter/Leave ACK로 경계를 확인한다.

**결과:** Shift를 누른 채 화면 이동이나 컴퓨터 간 drag는 지원하지 않는다. Entering/Leaving 중 빠른 입력은 abort 시 소실될 수 있고 재생하지 않는다. crash 중 client release를 완전히 보장하려면 helper 같은 별도 장치가 필요할 수 있다. 운영 안전 범위를 과장하지 않는다.

## ADR-011 — 수동 IP / 단일 선택 화면 / Accepted

**상황:** 개인 장비 두 대, 고정 배치이며 discovery보다 입력 correctness가 먼저다.

**대안:** manual address, mDNS, central broker; 모든 화면 union vs 선택 display.

**결정:** 수동 IP:port, Mac TCP connect, Windows listener. 선택 화면 하나씩. Mac logical bounds를 보고하고 Windows physical bounds와 edge-height ratio로 연결한다. multi-monitor gap이 있는 union bounding box는 사용하지 않는다.

**결과:** IP 변경 시 config 변경 필요. mDNS는 pairing 이후 편의 기능으로 추가. 서로 다른 물리 모니터 높이를 정확히 맞추지 않는다. 지원 밖 모니터 배치는 거부하고 확대를 조용히 추정하지 않는다.

## ADR-012 — 자체 protocol 우선 / Accepted for this design

**상황:** 원문은 Barrier 호환 구현을 요구하지만 요청된 HID/TOML/학습 순서는 독자 protocol 설계에 적합하다.

**대안:** Barrier protocol을 처음부터 정확히 구현, 독자 protocol만, 내부 모델 + 추후 compatibility adapter.

**결정:** EdgeBridge v1은 독자 protocol, Barrier adapter는 별도 track. 원문을 읽고 확인한 차이이며 누락을 숨기지 않는다. [Coding Challenge #142](https://codingchallenges.substack.com/p/coding-challenge-142-software-kvm).

**결과:** 실제 Barrier를 M1 fake peer 대신 쓸 수 없다. 원문 Step 15를 완료했다고 할 수 없다. challenge 완전 완수가 최우선으로 바뀌면 M0 이전 이 결정을 재검토해야 한다. 참고 프로젝트는 [lan-mouse](https://github.com/feschber/lan-mouse), [Input Leap](https://github.com/input-leap/input-leap), [Barrier](https://github.com/debauchee/barrier). 코드를 가져올 때 각 라이선스 의무를 검토한다.

## ADR-013 — server-authoritative repeat / Proposed, M4 gate

**상황:** keyboard hold가 Windows와 Mac에서 각각 반복되면 두 배 입력이 될 수 있다.

**대안:** Windows repeat를 모두 전달, 최초 down/up만 보내 Mac repeat 사용, 별도 client repeat timer.

**결정 후보:** Windows 반복 Down을 Repeat로 normalize, Mac autorepeat flag로 post, client timer 없음.

**결과:** OS의 injected event repeat 동작과 text editor의 accent popup 등 차이를 시험해야 한다. 5초 hold에서 cadence/count가 실제 입력과 맞고 duplicate repeat가 없어야 확정한다. 실패 시 wire Repeat와 backend 정책을 함께 갱신한다.

## ADR-014 — clipboard 보안 이후 / Accepted

**상황:** clipboard는 대량 데이터·민감 정보·양방향 ownership loop를 추가한다.

**대안:** 초기부터 전체 MIME sync, text만 단계적 도입, 수동 paste transfer.

**결정:** M7 이후 opt-in UTF-8 text, 1MiB 상한 후보. v1.0에서는 reserved만 존재한다. M8에서 origin/change ID·동시 변경·취소·bulk 우선순위를 정의한 v1.1 계약을 먼저 쓴다.

**결과:** 초기 KVM에서 clipboard가 없는 것은 의도된 범위다. 동일 TCP bulk로 input이 지연되면 chunk scheduling/별도 stream을 측정한다. 비밀번호 자동 판별을 약속하지 않는다.
