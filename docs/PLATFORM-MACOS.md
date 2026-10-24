# macOS client backend

상태: M1 Quartz backend 구현·컴파일 완료, 실제 injection은 아직 실행하지 않았다. Mac에서 시작하되 fake server가 실제 protocol을 사용하도록 한다. client는 입력 **주입만** 수행한다.

## 1. 권한과 생명주기

`CGPreflightPostEventAccess()`로 event synthesizing 권한을 확인하고 명시적인 사용자 동작에서 `CGRequestPostEventAccess()`를 요청한다. `AXIsProcessTrustedWithOptions`는 Accessibility 상태/안내에 사용할 수 있다. System Settings → Privacy & Security → Accessibility에서 실행 주체를 허용한다. CLI를 Terminal에서 실행할 때와 서명된 app bundle에서 실행할 때 TCC 귀속이 다를 수 있으므로 M1에서 실제 허용 대상을 기록한다. binary path/서명 변경 후 재허용이 필요할 수 있다. [Apple post preflight](https://developer.apple.com/documentation/coregraphics/cgpreflightposteventaccess%28%29?language=objc).

이 injection-only 설계는 Accessibility/post-event access를 대상으로 하며 **Input Monitoring 요청은 하지 않는다**. `CGEventTapCreate`, `CGPreflightListenEventAccess`, 전역 keyboard monitor를 추가하지 않는다. 향후 Mac 로컬 입력 자동 takeover나 전역 escape를 감지하면 permission 모델을 다시 설계해야 한다. 네트워크 privacy/firewall prompt가 OS 버전과 배포 방식에 따라 별도로 나올 수 있으므로 “어떠한 다른 prompt도 없다”는 뜻은 아니다.

permission이 없으면 Ready를 보내지 않는다. 실행 중 철회가 감지되면 epoch 비활성화→ledger release 시도→Error Permission/close. 권한이 이미 철회된 상태에서는 release 자체도 실패할 수 있다. 시작/Enter/주기적 health tick(1s)에 권한을 검사한다. permission API가 true라도 대상 앱에서 실제 처리되었음을 보장하지 않는다.

## 2. Quartz 객체와 thread

`CGEventSourceCreate(kCGEventSourceStatePrivate)`를 후보로 사용하여 주입용 event source를 분리한다. source 하나를 injection actor가 소유하고 모든 이벤트에 명시적 modifier flags를 설정한다. CoreFoundation Create/Copy ownership을 RAII로 감싸 release한다. raw CGEventRef를 Tokio task 사이에 전달하지 않는다.

`CGEventCreateMouseEvent`, `CGEventCreateKeyboardEvent`, `CGEventCreateScrollWheelEvent`로 생성하고 `CGEventPost(kCGHIDEventTap, event)`로 post하는 경로를 기본 실험안으로 둔다. `kCGSessionEventTap`과 동작 차이는 M1/M4에서 확인한다. post location 상수는 event tap **설치**를 의미하지 않는다. 현재 SDK에서 `CGEventPost`는 `void` 반환이다. ACK는 함수 호출 완료이며 앱 처리 성공이 아니다.

AppKit의 `NSEvent.doubleClickInterval`, 화면/절전 알림, 추후 NSPasteboard 사용을 위해 main run loop에서 injection actor를 실행한다. network는 별도 Tokio runtime. 긴 작업은 actor에 넣지 않으며 timer/queue를 bounded batch로 처리한다. Cocoa 호출 주위 autorelease pool을 적절히 관리한다.

## 3. 화면과 위치

`CGGetActiveDisplayList`, `CGDisplayBounds`, `CGDisplayCopyDisplayMode`로 선택 화면을 조사한다. logical bounds와 mode의 pixel width/height를 구분한다. 내장 화면은 builtin selector로 선택한다. mirror 구성이나 회전 화면에서 pixel dimension/axis를 잘못 보고하지 않도록 M1에서는 단일 비회전 내장 display만 허용한다.

wire Position은 display-local Quartz 단위다. post 위치는 `(origin_x+x, origin_y+y)`. 화면 원점은 음수일 수 있다. AppKit의 bottom-left 좌표를 Quartz의 top-left/down-positive 공간에 직접 더하지 않는다. Retina라는 이유로 Position에 무조건 2를 곱하지 않는다. scaled resolution에서도 CGEvent로 요청한 위치와 실제 cursor 위치가 일치하는지 시험한다. [CGDisplayBounds](https://developer.apple.com/documentation/coregraphics/cgdisplaybounds%28_%3A%29).

`CGDisplayRegisterReconfigurationCallback`에서 알림만 enqueue하고 현재 epoch를 종료한다. callback에서 네트워크를 기다리지 않는다. display/scale 변경은 연결을 끊고 새 Hello를 보내는 방식으로 처리한다. `NSWorkspace` sleep/wake/session 알림도 같은 cleanup 경로를 사용한다. 잠금 화면이나 Fast User Switching 중 주입을 지원한다고 약속하지 않는다.

## 4. 이동·버튼·drag

| 현재 injected held button | 위치 변경 이벤트 |
|---|---|
| 없음 | `kCGEventMouseMoved` |
| Left 보유 | `kCGEventLeftMouseDragged` |
| Right만 보유 | `kCGEventRightMouseDragged` |
| Middle/Back/Forward만 보유 | `kCGEventOtherMouseDragged` + button number |

동시에 여러 버튼이 눌리면 가장 먼저 눌린 held button을 drag owner로 선택하고, owner 해제 시 다음 오래된 held button으로 전환한다. 이것은 MVP의 결정이며 앱별 복합 버튼 gesture 지원은 실험 대상이다. 단순 move만 post하면 일부 앱의 drag가 동작하지 않는다.

Button 번호는 wire Left/Right/Middle/Back/Forward를 Quartz 0/1/2/3/4로 매핑한다. Other 이벤트에는 `kCGMouseEventButtonNumber`도 설정한다. 버튼 down/up은 해당 `kCGEventLeftMouseDown/Up`, Right, Other 유형으로 생성한다. 모든 move/drag/button/scroll 이벤트에 remapped modifier flags를 적용한다. CGEventCreateMouseEvent의 위치만 바꾸는 대신 drag type/필드를 일관되게 관리한다.

### double/triple click

네트워크는 Button의 source_ms와 position을 보낸다. client가 버튼별 click count를 계산한다. 이전 같은 버튼 up 후 다음 down까지 간격이 `NSEvent.doubleClickInterval` 이하이고 down 위치 거리 <=4 logical units, intervening 다른 버튼/drag/Leave가 없으면 증가(최대 3); 아니면 1. 시간은 server monotonic 차이로 비교한다. down/up 한 쌍은 동일 count. `CGEventSetIntegerValueField(kCGMouseEventClickState, count)`로 설정한다. drag 판정 거리 초과/새 epoch/Disconnect에서 chain을 초기화한다. 4-unit threshold는 앱/OS와 비교할 **Proposed** 값이다. [Apple click state](https://developer.apple.com/documentation/coregraphics/cgeventfield/mouseeventclickstate).

## 5. scroll

`CGEventCreateScrollWheelEvent(source, unit, wheelCount, ...)`의 `kCGScrollEventUnitLine`과 `kCGScrollEventUnitPixel`을 구분한다. wheel1=vertical, wheel2=horizontal. 사용 SDK의 non-variadic `CGEventCreateScrollWheelEvent2` availability를 확인하고 가능한 wrapper를 선택한다. variadic FFI를 사용할 때 C 정수 promotion/argument 개수를 지킨다. [Apple scroll API](https://developer.apple.com/documentation/coregraphics/cgeventcreatescrollwheelevent).

Windows wheel의 signed delta는 120 단위에 한 notch이지만 high-resolution wheel은 그보다 작은 값이 가능하다. server는 기본 `lines_per_notch=3.0`으로 크기를 환산한다. wire가 콘텐츠의 실제 이동 방향을 나타내므로 기본 변환은 `dy=-(vertical_delta/120)*3`, `dx=-(horizontal_delta/120)*3`다. Windows scrolling 설정을 그대로 복제한다는 뜻은 아니다. OS의 page-scroll 설정은 MVP에서 별도 지원하지 않는다. Quartz wheel1/wheel2 부호로의 최종 변환은 M3 browser fixture에서 콘텐츠 이동으로 검증하고, natural-scroll 설정을 두 번 적용하지 않는다.

client는 축/단위별 fractional remainder를 유지하고 API가 요구하는 정수 단위가 될 때 post한다. 음수 값도 부호 대칭으로 누적한다. fractional high-resolution 값이 버려지지 않는지 시험한다. line→pixel은 API 기본 환산에 맡기지 않고 협상 unit 그대로 전달한다. pixel scroll은 capability를 켠 fake peer로 먼저 검증하고, 일반 Windows wheel을 정밀 trackpad gesture라고 위장하지 않는다. phase/momentum/pinch는 v1에서 생성하지 않는다. natural scrolling과 앱별 처리 차이는 Safari/TextEdit/Terminal에서 확인하고 축별 invert config로 보정한다.

## 6. keyboard / flags / release

HID→Mac virtual keycode는 [KEYMAP](KEYMAP.md). `CGEventCreateKeyboardEvent(source, vk, down)` 생성 후 `CGEventSetFlags`를 mapped held set에서 재계산한다. modifier Down에는 추가된 flags, Up에는 제거된 flags를 사용한다. Shift/Control/Option/Command의 flagsChanged 유형과 좌우별 동작은 실제 앱에서 확인한다. `kCGKeyboardEventAutorepeat`는 Key Repeat에만 1. 최초 Down/Up은 0. client 자체 repeat timer는 두지 않는다. OS가 injected keydown을 자동 반복하는지 M4에서 확인하여 이중 repeat가 있으면 반복 정책 ADR을 수정한다.

ledger는 physical usage→mapping 결과, target key reference count를 모두 가진다. Down 시 mapping을 고정하여 Up에서도 같은 target을 해제한다. 한 target에 여러 source가 매핑되면 첫 down/마지막 up만 post한다. modifier flags는 left/right refcount를 합산한다. 누른 상태에서 config를 바꾸지 않는다.

Leave/EOF/timeout은 active epoch를 먼저 닫고 pending command를 폐기한다. 실제 post한 일반 키→modifier→button 순서로 up, flags=남은 held modifiers. client가 주입하지 않은 Mac 로컬 키는 일괄 up하지 않는다. 두 소스가 같은 키를 동시에 잡는 경우 OS 합성 상태의 완전한 분리가 보장되지 않으므로 동시 사용은 지원 범위 밖이다.

Secure Input, password field, 게임, 일부 sandbox 앱에서 합성 입력이 다를 수 있다. 모든 앱에 입력된다고 약속하지 않는다. 실패 시 OS 보호를 우회하지 않고 사용 범위를 기록한다. Caps Lock/IME/미디어는 일반 문자 키와 같지 않다.

## 7. 후보 crate와 실험 기록

- `core-graphics` + `core-foundation`: Quartz wrapper 후보. 지원하지 않는 API만 좁은 FFI로 보완한다. [core-foundation-rs](https://github.com/servo/core-foundation-rs).
- `objc2`, `objc2-app-kit`, `objc2-foundation`, `objc2-core-graphics`: AppKit/main-thread 표식과 현대 bindings 후보. [objc2](https://github.com/madsmtm/objc2). 같은 CF 객체를 서로 다른 ownership wrapper에 중복 소유시키지 않는다.
- 어느 조합을 쓰는지는 M1에서 permissions/scroll/flags API availability와 unsafe 면적을 비교해 확정한다. 문서 작성 시 crate version을 잠그지 않는다.

M1 필수 기록: macOS build, CPU arch, Xcode SDK, 실행 경로/서명/권한 주체, display bounds/pixel size, permission 허용/거부 결과. native MacBook에서 4 corners/center 이동 및 선택 display clamp를 관찰한다. test harness에 3초 countdown과 제한된 이동 영역, 종료 시 release를 넣는다. 테스트는 비밀번호 필드가 아닌 빈 TextEdit 문서에서 수행한다.

**강제 종료 한계:** SIGKILL/crash이면 destructor/lease timer가 실행되지 않는다. OS가 이미 받은 Down을 자동으로 풀어준다고 가정하지 않는다. M6에서 modifier/button hold 후 client 강제 종료 시험을 수행하고 별도 watchdog process가 필요한지 결정한다. production 수준 crash recovery를 요구한다면 이 미정을 해소하기 전 일상 사용 승인을 하지 않는다.

### 조사 근거

웹 Apple 문서는 일부 페이지가 JS shell만 반환하여 로컬 Xcode SDK `CoreGraphics.framework/Headers/CGEvent.h`, `CGEventTypes.h`도 2026-10-24에 확인했다. CreateMouseEvent/KeyboardEvent/ScrollWheelEvent, click-state 필드, `CGEventPost`의 void 반환, post/listen permission API 분리는 header에서 확인했다. TCC 실제 귀속·사용감·injection 성공은 아직 실험하지 않았다.


## M1 현재 구현과 수동 검증

ADR-015의 core-graphics/core-foundation 조합을 사용한다. AX 신뢰와 post preflight가 모두 true여야 시작/Ready/Enter를 허용한다. `--request-permission` 외에는 권한 prompt를 요청하지 않는다. 실제 환경별 허용 대상은 [M1 검증 절차](M1-VERIFICATION.md)에서 사용자 확인이 필요하다.

현재 click-state는 항상 1이고 AppKit multi-click interval 연동은 미구현이다. display 변경은 1s health polling이며 callback/sleep/lock 알림은 M6까지 남는다. `--observe-pointer`는 이동 post 약 50ms 뒤 idle에서 CGEvent 위치를 읽어 global target/actual/오차를 출력한다. event tap을 설치하지 않으며 자동 테스트에서 실행되지 않는다. 관측 로그는 실제 앱의 클릭 처리 성공을 보증하지 않는다.
