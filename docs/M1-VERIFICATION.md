# M1 검증 기록과 수동 절차

2026-10-24 구현. 실제 mouse event를 주입하거나 TCC를 변경하지 않고 compile/unit/TCP recorder 테스트만 실행했다. M1 실기기 수락은 아직 미완료다.

## 자동 검증 환경

- macOS 26.6.2 (25G83), arm64 (Apple Silicon)
- Xcode SDK 27.0 (`xcrun --show-sdk-version`)
- rustc 1.96.0 (ac68faa20 2026-05-25), workspace pin 유지
- 실행 예정 binary: 저장소 루트 기준 `target/debug/edgebridge-client`
- 실제 TCC 귀속/서명 상태/화면 logical·pixel/권한 허용·거부 결과: **미확인**. 실행 로그와 아래 명령으로 사용자가 기록한다.

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --workspace --target x86_64-pc-windows-msvc
```

최종 결과: fmt/check, Clippy `-D warnings`, Windows 타깃 check, `git diff --check` 모두 통과. `cargo test --workspace`: **71 passed, 0 failed, 0 ignored** (client actor 9 / core unit 11 / config 8 / handshake 12 / session 8 / codec 15 / golden 6 / loopback 2).

자동 테스트는 Quartz 객체·권한 API를 호출하지 않는다. core fake clock/좌표/100회 ledger, client 실제 TCP + recorder backend, 기존 codec/handshake/loopback 테스트를 포함한다. Windows check는 링크·실행 검증이 아니다.

## 사용자가 실행할 실제 주입 절차

1. 외부 모니터/미러링을 해제하고 내장 화면 하나, 회전 0도에서 시험한다. 빈 TextEdit 문서를 열고 저장할 작업을 정리한다. Mac 트랙패드·키보드는 비상 조작에 남는다. 기본 `m1`은 클릭하지 않는다. 아래 명령은 **사용자가 실행할 때 실제 커서가 이동한다**.
2. 프로젝트에서 빌드한다. 권한 부여 후에는 같은 경로의 binary를 직접 실행해 실행 주체가 바뀌는 일을 줄인다.

```sh
cd edgebridge   # 저장소 루트
cargo build -p edgebridge-client -p edgebridge-test-peer
codesign -dv --verbose=2 target/debug/edgebridge-client 2>&1
```

3. 권한 거부 상태에서 다음 명령을 실행하면 Accessibility/post-event access denied로 종료해야 한다. Ready 로그와 이동은 없어야 한다. 기존에 권한이 있다면 이를 거부 실험의 증거로 기록하지 않는다.

```sh
./target/debug/edgebridge-client --config config/client.example.toml
```

4. System Settings → Privacy & Security → Accessibility에서 **실행하는 Terminal.app 또는 iTerm2**, 혹은 OS가 표시하는 **edgebridge-client**를 허용한다. binary를 직접 추가할 때는 그 binary의 절대 경로를 선택한다. 실제 TCC 책임 프로세스는 실행 환경별로 달라 여기서 단정하지 않는다. 허용한 항목/터미널/실행 경로/서명 출력을 기록하고 프로그램을 재시작한다. 필요할 때만 아래 명시적 요청을 사용한다. Input Monitoring은 요청하거나 켜지 않는다. TCC DB나 tccutil 수정은 하지 않는다.

```sh
./target/debug/edgebridge-client --config config/client.example.toml --request-permission
```

listener가 없으면 권한 통과 뒤 connection refused로 끝날 수 있다. 이 단계만으로 연결/주입 성공이라고 기록하지 않는다.

5. 터미널 A에서 fake server를 시작한다.

```sh
./target/debug/edgebridge-test-peer fake-server --bind 127.0.0.1:24810 --pointer-only --script m1
```

6. 터미널 B에서 client를 실행한다.

```sh
./target/debug/edgebridge-client --config config/client.example.toml --observe-pointer
```

예상: binary 경로와 screen ID/origin/logical size/pixel size/scale 출력 → Ready (inactive) → **3초 대기** → 오른쪽 안쪽 진입 → center → 좌상/우상/우하/좌하 안쪽을 500ms 간격으로 이동 → Leave → Disconnect → `inactive; held ledger=0`. inner corner는 각 경계에서 2 logical units 이상 안쪽이다. 스크립트는 약 8초 내 종료한다. 입력 이벤트는 전체 화면 내부로 제한하지만 화면 전체를 이동한다.

`pointer target=... actual=... error=...` 로그에서 center/네 corner 모두 오차 <=1 logical unit인지 확인한다. Retina 기본 모드와 다른 scaled 모드에서 각각 새 프로세스로 반복한다. 실험 중 트랙패드를 움직이면 오차 측정을 무효로 기록한다. target은 local 좌표에 origin을 더한 값이며 backing 배율을 다시 곱하지 않는다.

7. 선택적으로 빈 TextEdit 문서가 center에 오도록 배치하고 A의 `--script m1`을 `--script click`으로 바꾸어 다시 시작한다. B도 재실행한다. center에서 Left Down/Up 한 쌍이 추가된다. double/triple click, 앱별 drag 동작은 M3 미검증이다.

8. release fault를 확인하려면 A의 script를 각각 `eof`, `blackhole`로 바꾸고 B를 재실행한다. 이 두 실험은 center에서 **Left Down을 주입**하므로 민감한 앱/버튼 위에서 실행하지 않는다. Down 뒤 actor Ping barrier가 확인되면 EOF는 즉시 close, blackhole은 1.5초간 응답을 멈춘다. B는 오류 exit와 함께 inactive/ledger=0을 기록해야 한다. blackhole은 마지막 matching Pong 뒤 약 1초 + scheduling 여유에 Up이 나와야 한다. 실제 버튼이 남지 않았는지 Mac 입력으로 확인한다. 자동 테스트는 ledger release만 확인했으며 이 실제 OS 결과를 대신하지 않는다.

9. 긴급 종료는 client 터미널에 초점을 두고 **Ctrl-C**. 정상 signal 경로에서 먼저 release 후 종료한다. fake server를 Ctrl-C로 끝내도 client는 EOF로 release한다. `kill -9`/crash/잠금·절전은 cleanup 보장 밖이며 이번 절차에 포함하지 않는다. 버튼이 남으면 Mac에서 해당 버튼을 눌렀다 놓고 필요 시 앱/사용자 세션을 재시작한다.

기록: 날짜 / OS·SDK / 실행 터미널·binary·서명 / Accessibility 실제 허용 항목 / screen 로그 / display 모드 / 각 target·actual·error / 종료 이유·ledger / 실제 버튼 release 여부. ROADMAP의 실기기 체크박스는 이 증거를 얻은 뒤에만 변경한다.

## 남은 범위

- 실제 TCC 허용/거부, Quartz 전달, Retina/scaled 오차, 클릭·release·Ctrl-C 관찰
- multi-click chain(OS doubleClickInterval), Scroll/Keyboard
- sleep/lock notifications, display callback 즉시 중단, 자동 reconnect, 강제 종료 복구
- Windows 네이티브 build/실행, M2 capture 이후 end-to-end
