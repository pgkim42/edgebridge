# HID와 키 매핑

상태: 물리 키 전달 정책은 Accepted, IME/lock/media/Fn 동작은 아래처럼 구분한다. wire 정의는 [PROTOCOL](PROTOCOL.md). 단순 ASCII 변환기가 아니라 양쪽 OS의 물리 키 의미를 연결한다.

## 1. 변환 경로

```text
Windows KBDLLHOOKSTRUCT
  scanCode + LLKHF_EXTENDED + vkCode(특수 키 보조)
       -> normalize Windows scan identity
       -> USB HID { page, usage }   (physical, remap 전)
       -> Key(epoch,hid,Down/Up/Repeat)
       -> Mac mapping profile
       -> mac virtual keycode + held flags
       -> CGEventCreateKeyboardEvent / CGEventPost
```

HID Keyboard/Keypad page `0x07`와 usage를 분리해서 전송한다. 예를 들어 A는 `{0x07,0x04}`. 문자열 “A”나 Windows VK_A 숫자가 아니다. 배열 index를 keycode로 쓰지 않는다. source layout에서 문자가 무엇인지 서버가 번역하지 않는다. Mac의 활성 input source/IME가 최종 문자열을 결정한다. [USB-IF HID Usage Tables](https://www.usb.org/document-library/hid-usage-tables-17).

Windows의 set-1 유사 scan identity는 일반 키에 유용하지만 모든 key가 한 바이트인 것은 아니다. extended flag로 main Enter/Keypad Enter, left/right Ctrl·Alt, navigation/keypad를 구분한다. Pause(E1 계열), PrintScreen(fake Shift 포함 가능), 한국어 IME, vendor driver는 별도 normalization fixture를 만든다. hook의 scanCode가 E0/E1 byte stream을 그대로 준다는 가정은 금지한다. `vkCode` fallback은 알려진 예외에만 허용한다.

MVP는 keyboard page의 아래 “MVP” 행만 공통 지원 집합으로 둔다. 나머지는 normalizer가 미지원으로 분류하고 server에서 전송하지 않는다. Remote에서 미지원 물리 key도 local shortcut으로 새지 않도록 일반 억제 정책을 유지한다. unsupported count를 표시하되 사용자 입력 내용을 기본 로그로 남기지 않는다.

## 2. 매핑 표 초안

수치는 hex. Mac은 Carbon `kVK_*` 상수에 대응하는 virtual keycode다. ANSI/ISO/JIS 차이를 숨기지 않는다. 여기 없는 usage는 임의로 근접 코드에 매핑하지 않는다. M4 구현 전 선택 SDK의 `HIToolbox/Events.h`와 physical fixture로 전체 표를 확인한다.

| 키 | Windows scan identity 예 | HID usage (page 07) | Mac vk | 범위 |
|---|---|---|---|---|
| A | 1E | 04 | 00 | MVP |
| B | 30 | 05 | 0B | MVP |
| C | 2E | 06 | 08 | MVP |
| D | 20 | 07 | 02 | MVP |
| E | 12 | 08 | 0E | MVP |
| F | 21 | 09 | 03 | MVP |
| G | 22 | 0A | 05 | MVP |
| H | 23 | 0B | 04 | MVP |
| I | 17 | 0C | 22 | MVP |
| J | 24 | 0D | 26 | MVP |
| K | 25 | 0E | 28 | MVP |
| L | 26 | 0F | 25 | MVP |
| M | 32 | 10 | 2E | MVP |
| N | 31 | 11 | 2D | MVP |
| O | 18 | 12 | 1F | MVP |
| P | 19 | 13 | 23 | MVP |
| Q | 10 | 14 | 0C | MVP |
| R | 13 | 15 | 0F | MVP |
| S | 1F | 16 | 01 | MVP |
| T | 14 | 17 | 11 | MVP |
| U | 16 | 18 | 20 | MVP |
| V | 2F | 19 | 09 | MVP |
| W | 11 | 1A | 0D | MVP |
| X | 2D | 1B | 07 | MVP |
| Y | 15 | 1C | 10 | MVP |
| Z | 2C | 1D | 06 | MVP |
| 1..9,0 | 02..0B | 1E..27 | 12,13,14,15,17,16,1A,1C,19,1D | MVP; 순서 주의 |
| Enter / Keypad Enter | 1C / E0-1C | 28 / 58 | 24 / 4C | MVP |
| Escape | 01 | 29 | 35 | MVP; escape chord는 server 우선 |
| Backspace / Tab / Space | 0E / 0F / 39 | 2A / 2B / 2C | 33 / 30 / 31 | MVP |
| - / = | 0C / 0D | 2D / 2E | 1B / 18 | MVP |
| [ / ] / backslash | 1A / 1B / 2B | 2F / 30 / 31 | 21 / 1E / 2A | MVP ANSI |
| semicolon / quote / grave | 27 / 28 / 29 | 33 / 34 / 35 | 29 / 27 / 32 | MVP ANSI |
| comma / period / slash | 33 / 34 / 35 | 36 / 37 / 38 | 2B / 2F / 2C | MVP |
| Caps Lock | 3A | 39 | 39 | Proposed, 기본 미지원 |
| F1..F12 | 3B..44,57,58 | 3A..45 | 7A,78,63,76,60,61,62,64,65,6D,67,6F | MVP; media 기능과 다름 |
| PrintScreen / ScrollLock / Pause | 특수 normalize | 46 / 47 / 48 | 기본 없음 | 보류 |
| Insert / Application(Menu) | E0-52 / E0-5D | 49 / 65 | 기본 없음 | 보류 |
| Home / PageUp | E0-47 / E0-49 | 4A / 4B | 73 / 74 | MVP; 앱 의미는 다를 수 있음 |
| Delete forward / End / PageDown | E0-53 / E0-4F / E0-51 | 4C / 4D / 4E | 75 / 77 / 79 | MVP |
| Right / Left / Down / Up | E0-4D / E0-4B / E0-50 / E0-48 | 4F / 50 / 51 / 52 | 7C / 7B / 7D / 7E | MVP |
| Keypad 0..9 / decimal | non-E0 keypad scans | 62,59..61 / 63 | 52,53,54,55,56,57,58,59,5B,5C / 41 | 보류; NumLock 정책 먼저 |
| Keypad / * - + | E0-35 / 37 / 4A / 4E | 54 / 55 / 56 / 57 | 4B / 43 / 4E / 45 | 보류 |
| Left Ctrl / Shift / Alt / Win | 1D / 2A / 38 / E0-5B | E0 / E1 / E2 / E3 | 3B / 38 / 3A / 37 | MVP physical profile |
| Right Ctrl / Shift / Alt / Win | E0-1D / 36 / E0-38 / E0-5C | E4 / E5 / E6 / E7 | 3E / 3C / 3D / 36 | MVP physical profile |

표의 `E0-xx`는 정규화된 scan identity 표기이며 wire 값이 아니다. Mac ANSI 기반 숫자는 ISO extra key (`usage 64`)와 JIS 키에 자동 적용하지 않는다. Windows와 Mac layout 조합별로 실제 label/문자/물리 위치가 다른 사례를 기록한다.

## 3. modifier profile

기본은 `physical`: Ctrl→Control, Alt→Option, Win→Command, Shift→Shift. 개발자 편의 profile `ctrl-cmd-swap`은 좌우별 Ctrl↔Win(=Mac Command/Control)을 서로 바꾼다. Alt→Option/Shift→Shift는 유지한다. Ctrl만 Command로 보내고 Win도 Command로 보내는 many-to-one보다 교환이 shortcut 의미를 보존하기 쉽다. 어느 것이 사용자의 기본이 될지는 [OPEN-QUESTIONS](OPEN-QUESTIONS.md)에 남긴다.

```toml
[modifiers]
profile = "ctrl-cmd-swap"
# 아래 custom 표기는 후속 설계 예. MVP parser는 두 profile만 지원한다.
# [modifiers.custom]
# LeftCtrl = "LeftGui"
# LeftGui  = "LeftCtrl"
```

mapping은 client만 적용한다. server의 escape와 all-up 검사는 raw physical set 기준이다. Down 시 `(source HID -> mapped target)`를 ledger에 저장하고 Up에서 같은 대상을 해제한다. mapping reload는 Leave/close 후에만 한다. custom mapping을 추가할 때 target refcount와 modifier 좌우 보존 테스트가 필요하다. 하나의 target을 두 physical source가 잡은 경우 한 source Up만으로 target Up을 내면 안 된다.

AltGr은 일부 Windows layout에서 Ctrl+RightAlt 형태로 나타날 수 있다. 양쪽 실제 layout 확인 없이 가짜 Ctrl을 임의 삭제하지 않는다. MVP는 미국 ANSI 또는 실제 사용하는 한국어 layout을 명시하고, AltGr 지원은 별도 fixture로 검증한다. Windows global shortcut은 hook의 권한/시스템 처리에 따라 Mac으로 모두 넘길 수 없을 수 있다.

## 4. repeat와 IME

physical Down이 이미 held면 server는 `Repeat`로 정규화한다. client는 repeat flag가 있는 keydown을 post하고 held refcount를 증가시키지 않는다. 최초 down→여러 repeat→up 한 쌍을 유지한다. modifier/lock 키의 반복은 server에서 버리고, 일반 키의 repeat cadence는 Windows가 결정한다. Mac 자체 repeat timer를 추가하지 않는다. 실제 CGEvent 자동 repeat와 이중 생성 여부는 M4 승인 조건이다.

한국어는 **Mac에서 조합**한다. Windows IME가 만든 완성 문자열, WM_CHAR, Unicode injection은 v1 경로가 아니다. Mac 한국어 2벌식 활성 상태에서 `rksk` 물리 입력이 예상 조합이 되는지, backspace/space/enter/composition 중 shortcut 동작을 확인한다.

한영은 USB Keyboard LANG1=`0x90`, 한자는 LANG2=`0x91`가 관련 usage다. 그러나 Windows 101/103/106-key 설정/드라이버에 따라 RightAlt/RightCtrl 또는 `VK_HANGUL`/`VK_HANJA`로 보일 수 있다. scanCode 하나만 보고 “한영”으로 확정하지 않는다. 이 값들을 Mac JIS Kana/Eisu로 무조건 대응시키지도 않는다.

v1.0 기본은 LANG1/LANG2 미지원. 당장 입력 소스 전환은 Mac에서 설정한 shortcut을 물리 키 조합으로 보낸다(예: physical profile의 Control+Space; 실제 사용자 설정 확인). 후속 mapping은 `LANG1 -> configured chord` 후보지만 chord 자체 modifier와 이미 held인 modifier가 충돌한다. 추가하려면 별도 chord ledger, 원래 held 상태 복원, Up 없는 입력 정규화, 입력 소스 활성 결과 확인을 설계해야 한다. 특정 OS shortcut을 고정 보편값으로 가정하지 않는다.

## 5. lock / media / Fn

- **Caps Lock:** key held 상태와 lock toggle 상태가 다르다. Windows LED, Mac caps state, 한국어 input source toggle 기능까지 동기화되는 것이 아니다. v1.0 기본 미지원. 단순 down/up, flagsChanged/AlphaShift, 빠른 재연결, 원래 Mac lock 상태를 M4 spike에서 확인한 후 `LockState` 확장 여부를 결정한다. release-all로 lock을 꺼버리지 않는다.
- **NumLock/ScrollLock:** 일반 modifier가 아니다. Mac keypad/clear 의미가 다르다. wire HID 보존은 가능하지만 기능 지원은 후속.
- **media:** Consumer page `0x0C`(예: volume up 0xE9, down 0xEA, mute 0xE2, play/pause 0xCD). LL keyboard hook이 HID consumer report를 항상 제공하지 않는다. `WM_APPCOMMAND`/Raw HID와 Mac system-defined event 계층이 별도다. v1.0은 지원하지 않고 일반 keyboard page로 위장하지 않는다.
- **Fn:** 많은 키보드에서 firmware가 처리하여 독립 OS 이벤트가 없다. Windows Fn→Mac Globe/Fn을 보장하지 않는다. 실제 F1 키와 brightness/media를 구분한다. 이벤트가 없는 것을 프로토콜로 복원할 수 없다.

## 6. 테스트 계약

키별 fixture는 `(scanCode,flags,vkCode,action) -> HID -> mapped Mac vk/flags`를 기록한다. table-driven unit test뿐 아니라 TextEdit/Terminal/브라우저에서 Shift+A, Ctrl/Command+C/V, Option+arrow, 좌우 modifier 동시 hold, F-key, navigation, 한글 조합을 확인한다. keydown 없이 keyup을 받는 fixture는 실패/abort를 기대한다.

필수 adversarial cases: 서로 다른 source→같은 target의 마지막 Up, repeat가 refcount를 늘리지 않음, modifier down 후 disconnect, Enter 직전 key hold, held 중 오른쪽 edge, unsupported 키, layout 변경, client 재시작. 테스트 로그에 실제 비밀번호나 업무 텍스트를 포함하지 않는다.
