# EdgeBridge wire protocol v1.0 제안

상태: 설계 계약, 아직 구현/호환성 검증 없음. 변경하면 이 문서와 [DECISIONS](DECISIONS.md)를 함께 수정한다. **Barrier 호환 wire가 아니다.** `Hello` 등의 이름이 유사해도 message code/encoding은 다르다.

## 1. transport와 경계

Mac이 Windows TCP listener에 접속한다. 제안 port 24810은 프로젝트 로컬 기본값이며 등록된 전용 port라는 의미가 아니다. 양쪽 `TCP_NODELAY=true`. TCP keepalive는 보조이고 application heartbeat를 대체하지 않는다. 연결 한 개에 client 한 개만 허용한다. Ready client가 있으면 신규 연결은 Busy로 거부하고 기존 연결을 빼앗지 않는다.

M1–M6은 opt-in plaintext-dev; 기본 loopback. LAN 실험은 양쪽 명시 허용이 필요하다. M7부터 TLS 1.3 상호 인증 후 아래 bytes를 교환한다. TLS 실패 시 평문으로 재시도하지 않는다. TLS ALPN은 `edgebridge/1`, 0-RTT는 사용하지 않는다. 입력 frame을 다음 연결에서 재전송하지 않는다.

## 2. frame 형식

```text
TCP stream (TLS 사용 시 decrypted application stream)
+ u32 length BE -----------------------------------------------+
| magic[4]="EDGB" | major:u16 BE | minor:u16 BE | kind:u16 BE    |
| flags:u16 BE=0  | seq:u64 BE                                  |
| payload: postcard struct bytes                              |
+--------------------------------------------------------------+
```

length는 prefix 자신을 제외한 header+payload 길이. 고정 header 20 bytes, `20 <= length <= 65536`. 첫 4 bytes를 읽은 뒤 길이를 검증하고 할당한다. seq는 각 방향 독립, TCP 연결마다 첫 frame=1, 정확히 1씩 증가. overflow 전에 연결을 새로 만든다. retransmission/seq skip은 허용하지 않는다. TCP 자체의 재전송은 앱에서 보이지 않는다.

header 정수는 network big endian, payload는 postcard 규칙을 따른다. postcard의 정수 varint/zigzag와 header encoding을 혼동하지 않는다. payload DTO의 **아래 표 필드 순서가 wire 순서**다. kind는 수동 지정하고 Rust enum 선언 순번을 wire tag로 사용하지 않는다. fixed `[u8;16]`은 길이 prefix가 없는 배열, `String`/`Vec`은 postcard 길이 prefix를 사용한다. `f64`는 postcard 규격의 little-endian IEEE 754이며 NaN/Infinity는 의미 검증에서 거부한다. `usize`, 플랫폼 enum, `CGPoint`, 메모리 layout dump는 wire에서 금지한다. trailing payload byte도 오류다. [Postcard wire specification](https://postcard.jamesmunns.com/wire-format.html).

golden fixture 예: 첫 Hello보다 먼저 보내면 상태 오류지만, codec 수준 Ping(seq=1, token=0)의 bytes는 `00 00 00 15 45 44 47 42 00 01 00 00 00 08 00 00 00 00 00 00 00 00 00 01 00` (length 21, kind 8, payload varint 0). M0에서 독립 encoder와 대조한다.

## 3. 공통 타입

```rust
// DTO 스케치. field 순서 고정; 실제 구현은 별도 작업.
struct HidUsage { page: u16, usage: u16 }
struct Position { x: f64, y: f64 }
struct ScreenInfo {
    id: u32, origin_x: f64, origin_y: f64,
    width: f64, height: f64,
    pixel_width: u32, pixel_height: u32,
    scale_x: f64, scale_y: f64,
}
```

origin은 Mac global Quartz 좌표이고 width/height와 같은 단위다. 유효 범위: width/height 16..65536, |origin|<=1,000,000, pixel dimension 16..131072, scale 0.25..8.0. 모두 finite. pixel/logical 비율과 scale은 1% 이내여야 한다. screen id는 연결 내 고유하며 재접속 후 재사용 여부를 가정하지 않는다. screens는 C Hello에서 **선택 화면 하나**, S Hello에서 빈 목록. 추후 다중 화면 지원은 capability/새 버전으로 확장한다. Position은 선택 화면 local, `0 <= x <= width-1`, `0 <= y <= height-1`.

`role:u8`: Server=0, Client=1. `edge:u8`: Right=0 (v1에서 Mac 진입 edge는 이것만). `action:u8`: Down=0, Up=1, Repeat=2 (Button에는 0/1만). `button:u8`: Left=0, Right=1, Middle=2, Back=3, Forward=4. OS button 번호와 변환은 backend 책임.

`reason:u8`: Normal=0, Escape=1, Timeout=2, QueueOverflow=3, Permission=4, DisplayChanged=5, SleepOrLock=6, ProtocolError=7, Shutdown=8, BackendFailure=9. `code:u16`: Version=1, BadFrame=2, BadState=3, InvalidValue=4, Unsupported=5, Busy=6, Permission=7. 미정의 숫자는 invalid다.

hello role은 상대 역할이어야 하며, Ready의 capabilities는 S/C Hello 교집합과 정확히 일치해야 한다. 양 끝이 낸 name은 인증용 identity로 사용하지 않는다.

capabilities `u64`: bit0 Pointer, bit1 Keyboard, bit2 ScrollLines, bit3 ScrollPixels. 나머지는 0이어야 한다. 각 endpoint가 구현한 기능을 광고하고 교집합만 선택한다. Pointer는 필수; M1 fake peer는 Pointer만 가능. consumer media key, clipboard, multi-display, layout live update는 v1.0에 없다.

## 4. message registry

S=Windows, C=Mac. `epoch`은 server가 Enter마다 증가시키는 u64, 1부터 시작하며 session 내 재사용하지 않는다. 모든 input은 현재 active epoch에만 적용한다.

| kind | 이름 / 방향 | payload: 필드 순서 | 의미 |
|---|---|---|---|
| 1 | Hello / S↔C | role:u8, name:String, capabilities:u64, screens:Vec<ScreenInfo> | name UTF-8 1..64 bytes, 제어 문자 금지. display name은 인증 ID가 아님 |
| 2 | Ready / S↔C | session_id:[u8;16], capabilities:u64 | S가 random 128-bit ID와 교집합을 지정, C가 동일 값 echo |
| 3 | Enter / S→C | epoch:u64, screen_id:u32, edge:u8, position:Position | Ready+inactive에서만. 빈 ledger 확인 후 entry post |
| 4 | EnterAck / C→S | epoch:u64 | entry post 호출까지 완료, active로 변경 |
| 5 | Leave / S→C | epoch:u64, reason:u8 | 앞선 input 처리 후 release, inactive |
| 6 | LeaveAck / C→S | epoch:u64 | ledger release 시도 완료. Permission 오류면 ACK 대신 연결 종료 |
| 7 | Disconnect / 양방향 | reason:u8 | 즉시 local cleanup; 응답 대기 없이 close |
| 8 | Ping / 양방향 | token:u64 | Ready 이후, 방향별 증가 token; control/inject actor까지 전달 |
| 9 | Pong / 양방향 | token:u64 | 해당 Ping까지 앞선 명령 처리 후 echo |
| 10 | Error / 양방향 | code:u16, detail:String | 최대 256 UTF-8 bytes, 민감 값 금지. 전송 시도 후 cleanup+close |
| 16 | MouseMove / S→C | epoch:u64, position:Position | absolute Mac-local 위치. active만 |
| 17 | Button / S→C | epoch:u64, button:u8, action:u8, position:Position, source_ms:u64 | 위치 확정 후 down/up; source_ms는 server monotonic ms |
| 18 | Key / S→C | epoch:u64, hid:HidUsage, action:u8 | physical HID, client mapping 후 inject |
| 19 | Scroll / S→C | epoch:u64, unit:u8, dx:f64, dy:f64, position:Position | unit Lines=0 / Pixels=1, 둘 중 협상된 capability 필요 |
| 32..35 | Clipboard 계열 / reserved | v1.0에서 전송 금지 | M8에서 schema/버전 확정 |

Hello의 header는 1.0이다. 서버가 먼저 Hello, client는 compatible header를 확인한 뒤 자기 Hello를 보낸다. Ready 전에는 Hello/Ready/Error/Disconnect만 유효하다. server는 C Hello에서 선택 화면과 name=`mac`을 확인하고 Ready를 전송한다. client가 permission/backend 준비 완료 후 Ready를 echo해야 server가 edge를 arm한다. client는 Ready echo 이후 Enter를 받을 준비가 끝난 상태다.

source_ms는 session 시작 이후이고 Button마다 비감소해야 한다. 입력 지연 측정용으로 client wall-clock과 빼지 않는다. 한 번의 press-release 및 다음 down 간 시간 비교로만 click-count 판단에 사용한다.

Keyboard capability의 v1 지원 usage는 [KEYMAP](KEYMAP.md)의 MVP 행이다. 미지원 usage는 server에서 forward하지 않고 진단 count만 증가시킨다. wire로 미지원 usage를 받으면 Error Unsupported 후 세션 중단한다. 임의 숫자를 mac virtual keycode로 캐스팅하지 않는다.

## 5. input 상태 규칙

- Input은 EnterAck 이후 전송. Entering buffered 이벤트는 ACK 후 원래 capture 순서로 전송하고 버퍼 age 제한을 지킨다.
- MouseMove 절대 좌표는 client ledger의 held button에 따라 move 또는 drag 이벤트가 된다. Button/Scroll에도 Position을 넣어 직전 move coalescing에 의존하지 않는다. 위치 이동이 필요하면 기존 held 상태로 먼저 move/drag하고 그 위치에서 discrete 이벤트를 만든다.
- Key Down은 미보유 usage, Up/Repeat는 보유 usage에만 허용. Repeat는 새 hold가 아니다. 중복 Down/고아 Up/보유하지 않은 Repeat는 BadState. toggle/unsupported usage 예외는 backend에서 normalize하여 wire에 유효한 한 쌍만 보낸다.
- Button Down/Up도 같은 ledger 규칙. Up을 받으면 click-state는 대응 Down과 같은 값. mouse repeat는 없다.
- Scroll dx/dy는 유한수, 절대값 <=10000; +dy는 내용이 위쪽으로, +dx는 내용이 오른쪽으로 이동하도록 정의한다. backend가 OS wheel 부호에 맞춰 변환한다. 물리 wheel rotation의 부호와 동일하다고 가정하지 않는다. float sub-unit 잔여량을 보존한다.
- Keyboard는 text/IME 결과가 아니라 물리 usage. modifier flags는 client가 remapped held set에서 생성한다. wire mask를 별도로 두지 않아 서로 다른 두 진실을 만들지 않는다.
- TCP 순서가 있으므로 잘못된 epoch/state/seq는 재전송으로 복구하지 않고 abort한다. 이미 종료한 연결에서 도착하는 내부 callback은 connection generation으로 폐기한다.
- Leave 후 새 Enter까지 input 금지. client cleanup은 먼저 epoch를 비활성화하고 pending input을 폐기한 뒤 held 일반 키→modifier→button 순서로 release를 시도한다. 버튼은 현재 virtual position에서 up, flags는 해제 단계마다 재계산한다. cleanup 여러 번 호출은 무해해야 한다.

## 6. timeout / backpressure

| 항목 | 초기 계약 | 처리 |
|---|---|---|
| TCP connect / TLS handshake | 각각 5s | close, backoff 또는 인증 오류 정지 |
| application handshake | 첫 Hello부터 전체 3s | Ready 못 하면 close, 억제 금지 |
| Ping 주기 | 양쪽 각각 250ms | actor가 처리 후 Pong |
| peer lease | 마지막 유효 matching Pong 수신부터 1000ms | timeout cleanup; 일반 input 수신은 lease 갱신 안 함 |
| EnterAck / LeaveAck | 각 500ms | abort → Local/close |
| frame 조립 | 첫 prefix byte부터 1000ms, trickle로 연장 금지 | partial frame close |
| write stall | frame write 전체 250ms | partial write도 socket 폐기; 같은 frame 재시도 금지 |
| 입력 큐 oldest | 100ms | coalesce 가능 move 외 abort; key/button drop 금지 |
| graceful close | 최대 100ms 전송 시도 | cleanup은 먼저 실행 |

Ready 완료 시 lease를 시작한다. Pong은 미응답 token과 일치하고 token 발행 후 1000ms 이내여야 유효하다. 발행 간격 250ms, 최대 4개 outstanding, token별 발행시각을 저장한다. 이미 처리한 token/모르는 token은 protocol error. 양쪽 각각 peer 진행을 검사한다. Ping은 입력 순서를 넘어서 우선 처리하지 않는다. network reader가 바로 Pong을 보내면 injection actor가 멈춘 상태를 놓친다.

초기 Entering을 포함해 수신 명령을 제때 처리 못 하면 lease보다 100ms queue age가 먼저 실패할 수 있다. OS가 정지한 동안 정확히 timeout 시각에 cleanup할 수는 없다. wake에서는 timeout을 연장하지 않고 항상 session을 버린다.

## 7. 시퀀스

```text
Connect
Mac C                              Windows S
  |---- TCP connect ----------------->|
  |<=== M7 mutual TLS ================>|
  |<--- Hello(Server, screens=[]) -----|
  |---- Hello(Client, [screen]) ------>|
  |<--- Ready(new session, caps) ------|
  | permission + backend ready        |
  |---- Ready(same session, caps) ---->|
  |<--- Ping -------------------------|
  | actor processes -> Pong ---------->|
  |---- Ping ------------------------>|
  |<--- control thread -> Pong -------|
```

```text
Enter (Windows left -> Mac right)
Windows S                          Mac C
  | latch on; Entering               |
  |---- Enter(epoch=1, right,x=W-2) ->|
  | buffer bounded input             | release old ledger; post entry
  |<--- EnterAck(1) ------------------|
  | Remote; buffered input flush     |
  |---- MouseMove(1, absolute) ------>|
  |---- Button(1,Down,pos,time) ------>| ledger + inject
```

```text
Leave (virtual Mac x>=W, all up)
Windows S                          Mac C
  | Leaving; no more input           |
  |---- Leave(1,Normal) ------------->|
  |                                  | release; inactive
  |<--- LeaveAck(1) ------------------|
  | warp Windows to L+2; latch off    |
  | Local; edge unarmed until L+8     |
```

```text
Disconnect / blackhole
Windows S                          Mac C
  | Ctrl+Alt+Escape: latch off        |
  |---- Disconnect(Escape) ---X       | (미전달 시에도 lease 정리)
  | close + queues discard           | no matching Pong for 1000ms
  | Local                            | deactivate; release ledger; close
  |                                  | backoff; reconnect with new session
```

마지막 도식의 의미: Disconnect가 전달되지 않아도 상대는 자기 lease로 정리한다. 이미 post된 Down을 OS가 유지하는 상태에서 client 프로세스까지 죽으면 lease 코드도 실행되지 않는다. [macOS 제한](PLATFORM-MACOS.md)을 참조한다.

## 8. 버전·오류·공격 입력

v1.0은 major/minor **정확히 일치**해야 한다. minor downgrade나 unknown message skip은 아직 없다. 새 선택 기능도 schema를 변경하면 버전을 올리고 호환 테스트를 추가한다. protocol 1.0과 crate semver는 독립이다. enum 재정렬/field 추가가 자동 호환이라는 가정은 금지한다.

magic/길이 불량은 바로 close. magic이 맞고 버전만 다르면 공통 header 1.0의 Error Version을 best-effort로 보내고 close; 상대가 Error payload를 해석 못 해도 연결은 종료된다. 이후 입력 주입은 없다. 반사/증폭을 줄이기 위해 handshake 전 오류 응답은 1 frame로 제한한다. frame 제한뿐 아니라 String/Vec 길이 제한을 deserialize 시점에 적용하여 길이 prefix만 큰 입력의 선할당을 막는다. Ready client 한 개, pending handshake 최대 4개, IP별 재시도 rate limit을 후속 구현한다.

M8 clipboard는 v1.0에 몰래 넣지 않는다. 후보 schema: `ClipboardOffer(origin_id:[u8;16], change_id:u64, mime:u8, size:u32)`, `ClipboardRequest(origin_id,change_id)`, `ClipboardData(origin_id,change_id,offset:u32,bytes:Vec<u8>)`, `ClipboardCancel(origin_id,change_id)`. UTF-8 text만, 전체 1MiB/청크 16KiB, 동시에 한 transfer. 협상·취소·stale version·loop 방지·별도 bulk stream 여부를 확정한 뒤 v1.1로 정의한다. 이것은 reserved 설계 스케치이며 현재 codec 명세가 아니다.
