# Windows server backend

상태: API 조사 + 구현 제안. 실제 Windows 장비에서 검증하지 않았다. 기본 대상은 Windows 11 interactive user session이며 지원할 최소 build는 M2에서 기록한다. [아키텍처](ARCHITECTURE.md)의 상태/큐 계약을 따른다.

## 1. hook 설치와 callback

`SetWindowsHookExW(WH_KEYBOARD_LL, ...)`, `SetWindowsHookExW(WH_MOUSE_LL, ...)`를 dedicated control thread에 설치한다. 설치 결과가 NULL이면 `GetLastError`를 보고하고 Remote를 금지한다. hidden window를 만들고 `GetMessageW` / `TranslateMessage` / `DispatchMessageW` loop를 유지한다. shutdown은 thread에 종료 메시지를 보내고 `UnhookWindowsHookEx`로 정리한다. hook handle/FFI callback lifetime은 thread보다 짧아지면 안 된다.

callback은 설치한 thread에서 호출되므로 message loop가 필수다. `LowLevelHooksTimeout` 초과 시 Windows 7 이후 hook이 조용히 제거될 수 있다. Windows 10 1709 이후 시스템 허용 최대값은 1000ms지만 이것은 성능 예산이 아니다. 목표는 callback p99 <100µs, 관측 최대 <1ms; 실제 hardware/load를 함께 기록한다. OS scheduler pause를 완전히 방지할 수 없으므로 “절대 제거되지 않음”을 보장하지 않는다. [Microsoft LowLevelMouseProc](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelmouseproc).

- `nCode < 0`: 어떤 억제 정책보다 먼저 `CallNextHookEx` 결과 반환.
- keyboard: `WM_KEYDOWN/UP`, `WM_SYSKEYDOWN/UP`, `KBDLLHOOKSTRUCT.scanCode/vkCode/flags/time` 복사.
- mouse: `WM_MOUSEMOVE`, left/right/middle/X button down/up, `WM_MOUSEWHEEL/HWHEEL`의 최소 필드 복사.
- Local은 `CallNextHookEx`, Remote/Entering/Leaving은 유효 lease일 때 1 반환. emergency·overflow·expired lease는 latch를 내려 즉시 통과.
- hook 내부에서 `GetAsyncKeyState`로 현재 이벤트 처리 후 상태를 판별하지 않는다. hook은 async key state 갱신보다 앞서 호출된다. 자체 held bits를 사용한다. [Microsoft LowLevelKeyboardProc](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelkeyboardproc).
- FFI boundary 밖으로 Rust panic unwind 금지. panic을 막는 최소 callback, fallible enqueue, 별도 error flag를 사용한다. callback에서 문자열 format/println/tracing exporter를 호출하지 않는다.

시작/세션 복귀 시에는 아직 hook이 보지 못한 held 키가 있을 수 있다. hook 밖에서 `GetAsyncKeyState` 초기 snapshot과 이후 관측 이벤트를 합쳐 보수적으로 held set을 만들고, 전체 up이 관측되기 전 edge를 arm하지 않는다. 알 수 없는 상태를 빈 set으로 간주하지 않는다. 장치 분리·hook 재설치 후에도 이 초기화 절차를 반복한다.

정상 억제가 모든 프로그램의 모든 입력을 차단한다는 보장은 없다. raw input reader, 일부 게임, 다른 hook, privileged/secure desktop은 별도 경로를 쓸 수 있다. 일반 desktop 앱의 동작을 M3 수락 범위로 한정한다.

## 2. Raw Input vs center pin

| 항목 | LL mouse + center pin | Raw Input + LL suppression |
|---|---|---|
| 이동 | cursor position - center, 매번 recenter | `RAWMOUSE.lLastX/Y` 상대 count |
| edge clamp | recenter 전 큰 이동이 잘릴 수 있음 | OS cursor edge와 독립적인 상대 값 |
| 감각 | Windows 가속이 적용된 이동에 가까움 | raw count이므로 별도 sensitivity 필요 |
| 부작용 | warp 생성 이벤트 필터, race, cursor jump | 두 이벤트 경로의 순서/중복, device 특성 |
| 복구 | ClipCursor/숨김/warp 잔여 상태 정리 필요 | 기본적으로 ClipCursor 없이 운용 가능 |

**선택: Raw Input + LL suppression, Proposed.** `RegisterRawInputDevices`에서 mouse UsagePage=0x01, Usage=0x02, `RIDEV_INPUTSINK | RIDEV_DEVNOTIFY`, `hwndTarget=hidden window`. `WM_INPUT`에서 `GetRawInputData`로 읽는다. foreground `RIM_INPUT`의 정리는 `DefWindowProc` 요구사항에 맞춘다. process 내 같은 device class 등록은 하나의 window로 소유권을 모은다. `RIDEV_NOLEGACY`는 다른 앱의 입력을 전역 차단하는 수단이 아니며 이 설계에서 사용하지 않는다. [등록 API](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerrawinputdevices).

Raw는 **motion만** 사용하고 button/wheel은 LL만 전송하여 중복을 막는다. `MOUSE_MOVE_RELATIVE`와 `MOUSE_MOVE_ABSOLUTE`를 구분한다. absolute tablet/RDP 장치 데이터를 delta로 해석하지 않는다. MVP는 relative mouse만 Remote 대상으로 지원하고 absolute device가 활성화되면 Local 유지/복귀한다. `WM_INPUT_DEVICE_CHANGE` 시 held 상태가 불명확하면 abort한다. [RAWMOUSE](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-rawmouse).

Remote cursor는 LL 억제로 Windows 위치가 움직이지 않게 두며 매 이벤트 center warp/ClipCursor를 하지 않는다. Windows 화면에 남은 pointer 숨김은 필수가 아니다. 나중에 숨길 경우 `ShowCursor` counter semantics와 종료 복구를 검증한다. Raw와 LL의 상대 순서는 보장된 것으로 가정하지 않는다. M2에서 다음을 모두 통과해야 이 선택을 Accepted로 바꾼다.

1. LL return=1 중에도 WM_INPUT delta가 10분 연속 도착하는가?
2. 화면 가장자리에서 길게 이동해도 누적 delta가 계속 증가하는가?
3. 125/1000Hz 장치, 지원 가능하면 8000Hz에서 loop starvation/overflow가 있는가?
4. 빠른 이동-클릭, drag 시작/종료에서 최대 위치 오차가 허용 가능한가?
5. foreground 앱이 바뀌거나 Windows 배율이 바뀌면 입력이 새는가?

실패하면 Raw의 mouse 버튼/휠까지 통합하는 방안과 center-pin fallback을 비교한다. 자동 runtime fallback은 서로 다른 sensitivity/ordering 때문에 하지 않는다.

## 3. injected 입력과 escape

`LLKHF_INJECTED`, `LLMHF_INJECTED`는 수집·전송 대상에서 제외하고 `CallNextHookEx`로 통과시킨다. 자기 `SendInput` 복구 이벤트에는 `dwExtraInfo` marker도 붙여 진단한다. marker는 보안 인증 값이 아니다. 이 정책 때문에 다른 macro/remapper가 만든 입력은 Mac으로 가지 않으며 Windows에 나타날 수 있다. raw device event에는 LL injected bit와 같은 provenance가 없으므로 완벽한 합성 입력 식별을 주장하지 않는다. MVP는 물리 장치 조작을 전제한다. [KBDLLHOOKSTRUCT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-kbdllhookstruct).

기본 escape는 remapping 전 물리 `LeftCtrl+LeftAlt+Escape`. 마지막 Escape Down을 hook에서 확인하자마자 latch off, abort flag, epoch 폐기; network ACK를 기다리지 않는다. 앞서 Mac으로 보낸 Ctrl/Alt는 client release 경로가 정리한다. chord에 쓰인 key를 억지로 Windows Down으로 replay하지 않는다. 사용자는 모두 놓고 다시 입력한다. hotkey 변경은 항상 하나의 keyboard-only escape를 남기고 충돌을 startup에서 검사한다.

`Ctrl+Alt+Delete`는 OS secure attention sequence이므로 Mac으로 전달하거나 차단하는 기능을 약속하지 않는다. taskbar/tray의 “로컬 제어로 복귀”도 추가 가능하지만 마우스가 이미 remote라 유일한 탈출구가 될 수 없다. hook thread 자체가 죽거나 hook이 제거되면 hotkey 탐지도 실패할 수 있다. 별도 watchdog은 위험 완화이며 OS 전체 hang 해결책은 아니다.

## 4. DPI / monitor / session

프로세스 시작 시 HWND 생성 전에 manifest 또는 `SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)`로 DPI awareness를 확정한다. `EnumDisplayMonitors`, `GetMonitorInfoW`의 physical screen rect를 사용하고 virtual desktop origin이 음수인 경우를 테스트한다. work area 대신 full monitor bounds를 쓴다. taskbar 위치가 edge mapping을 바꾸면 안 된다.

Local 진입은 실제 선택 화면 왼쪽 edge, 복귀는 `SetCursorPos(L+2,y)`. warp 이벤트는 re-arm latch와 자기 warp generation으로 무시한다. `SetCursorPos` 이벤트가 반드시 injected flag를 가진다고 가정하지 않는다. `WM_DISPLAYCHANGE`, DPI/monitor configuration 변경, desktop 전환, session lock, suspend에선 모든 Remote를 종료하고 화면을 재조회한 후 handshake한다. session notification은 `WTSRegisterSessionNotification`, power는 `WM_POWERBROADCAST`/지원되는 power notification을 검토한다.

관리자 실행은 기본 요구가 아니다. 높은 integrity 앱/secure desktop과 상호작용은 제한될 수 있다. `SendInput`은 UIPI 제약을 받으며 현재 keyboard state를 자동 reset하지 않는다. 이 설계는 정상 전환 all-up으로 local stuck 상태를 예방하고, privileged 앱을 강제로 복구하려고 elevation하지 않는다. [SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput).

## 5. 후보 crate / 빌드 / 체크리스트

- `windows`: Microsoft의 Win32 bindings. 필요한 Win32_UI_WindowsAndMessaging, Input_KeyboardAndMouse, HiDpi, Gdi, RemoteDesktop, Power 계열 feature를 최소화한다. 실제 모듈 위치는 선택 버전에서 확인한다. [windows-rs](https://github.com/microsoft/windows-rs).
- `tokio`, `serde`, `postcard`, `toml`, `tracing`: 상위 계층. hook은 Tokio task가 아니다.
- `rdev`/`enigo` 같은 고수준 abstraction은 prototype 비교 대상으로만 둔다. suppression/extended scancode/raw ordering을 숨기면 core backend로 선택하지 않는다.
- native Windows MSVC 빌드/실행이 기준. Mac의 [cargo-xwin](https://github.com/rust-cross/cargo-xwin)은 MSVC target cross-build에 사용 가능하지만 SDK/CRT 및 crypto provider 빌드 요구를 확인해야 한다. cross-build 성공은 hook 검증이 아니다.

M2 기록에는 OS build, keyboard 종류/레이아웃, mouse model/poll rate, 모니터 rect/DPI, 권한 수준을 포함한다. password 앱에서 capture 로그 테스트하지 않는다. Windows firewall은 지정 private interface/port로 한정하며 all-profile 광역 허용을 기본으로 만들지 않는다. background service/session 0 구현은 MVP에서 제외한다.
