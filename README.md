# EdgeBridge

Windows 데스크톱의 키보드·마우스로 옆에 놓인 MacBook을 조작하기 위한 Rust 프로젝트입니다. 두 컴퓨터는 각자의 화면을 사용하고, LAN으로 입력만 전달하는 software KVM을 목표로 합니다. 화면 영상은 전송하지 않습니다.

**현재 위치: Mac 입력 클라이언트(M1)와 Windows 입력 수집 진단 도구(M2)가 구현됐습니다. Windows에서 수집한 입력을 Mac으로 보내는 연결(M3)은 아직 없습니다.**

## 지금 무엇을 할 수 있나

| 기능 | 상태 |
|---|---|
| Mac에서 테스트 서버에 접속해 커서 이동·클릭·연결 끊김 복구 실행 | 구현됨. 권한·좌표 오차·실제 버튼 해제는 기기에서 확인 필요 |
| Windows에서 키보드·마우스 이벤트를 터미널에 출력 | 구현됨. Windows 실기기 검증 대기 |
| Windows 마우스로 Mac 커서 조작 | 다음 단계 M3 |
| Mac으로 키보드 입력 전달 | M4 |
| 화면 경계를 넘으면 자동 전환 | M5 |
| 재연결·절전 등 오류 복구, TLS 인증, 클립보드 | M6 / M7 / M8 |

자동 테스트와 Windows 타깃 타입 검사는 Mac에서 수행했습니다. Windows 실행·입력 수집 성공을 확인했다는 뜻은 아닙니다. 단계별 완료 조건은 [로드맵](docs/ROADMAP.md)에 있습니다.

## Windows: 입력 수집 진단 실행

Windows에서 **Rust MSVC toolchain과 Visual Studio C++ Build Tools**가 필요합니다. 저장소 루트의 PowerShell에서 실행합니다. Rust 버전은 `rust-toolchain.toml`의 1.96.0으로 고정됩니다.

```powershell
cargo build -p edgebridge-server
.\target\debug\edgebridge-server.exe --capture-log --duration-secs 30
```

실행 후 빈 메모장에 테스트 문자를 입력하고 마우스를 움직이거나 클릭·스크롤해 봅니다.

- 일반 키와 좌우 modifier의 `Down` / `Repeat` / `Up`, 마우스 버튼, 휠, Raw Input 이동이 출력됩니다.
- **Windows 앱의 입력은 그대로 동작합니다.** 커서를 붙잡거나 키를 차단하지 않습니다.
- `--capture-log`를 명시해야 키 값이 기록됩니다. 진단 중에는 테스트 입력만 사용하고 비밀번호·민감한 내용을 입력하지 마세요.
- Mac 연결이나 네트워크 설정은 필요 없습니다. 이 모드는 포트를 열거나 Mac으로 입력을 보내지 않습니다.
- 30초 뒤 자동 종료됩니다. 먼저 끝내려면 `Ctrl-C`를 누릅니다. `--duration-secs`를 빼면 `Ctrl-C`로 종료할 때까지 수집합니다.
- 종료 시 전달/누락 이벤트 수, callback 처리 시간, 오류가 출력됩니다. `events_dropped: 0`, `errors: []`인지 확인합니다. 오류가 있으면 종료 코드도 실패입니다.

실제 Windows에서 확인할 순서와 통계 해석은 [Windows 검증 절차](docs/PLATFORM-WINDOWS.md#7-windows-실기기-검증-절차)에 있습니다. Mac에서 `--capture-log`를 실행하면 Windows가 필요하다는 오류와 종료 코드 69가 나옵니다.

## Mac: 테스트 서버로 커서 이동 확인

Windows 없이 Mac 하나에서 수신·주입 경로를 확인할 수 있습니다. 아래 테스트는 **실제 커서를 움직입니다.** 처음에는 내장 화면 하나로 진행합니다.

```sh
cargo build -p edgebridge-client -p edgebridge-test-peer
```

터미널 A에서 테스트 서버를 실행합니다.

```sh
./target/debug/edgebridge-test-peer fake-server \
  --bind 127.0.0.1:24810 --pointer-only --script m1
```

터미널 B에서 클라이언트를 실행합니다.

```sh
./target/debug/edgebridge-client \
  --config config/client.example.toml --observe-pointer
```

Accessibility 권한 오류가 나면 시스템 설정의 **개인정보 보호 및 보안 → 손쉬운 사용**에서 실행 터미널 또는 OS가 표시하는 `edgebridge-client`를 허용하고 다시 실행합니다. Input Monitoring 권한은 필요하지 않습니다.

기본 `m1` 스크립트는 클릭 없이 커서를 중앙과 네 모서리 안쪽으로 옮긴 뒤 약 8초 안에 종료합니다. **실험 중에는 마우스·트랙패드에서 손을 떼세요.** 직접 커서를 움직이면 `error` 측정은 무효입니다. 각 위치의 `error`가 1 이하이고 마지막 로그가 `inactive; held ledger=0`인지 확인합니다.

클릭, 연결 끊김, 응답 중단, Retina 화면 배율 검증은 [M1 수동 검증](docs/M1-VERIFICATION.md)에 따릅니다. 자동 테스트의 메모리 backend와 실제 OS 입력 전달은 별개의 검증입니다.

## 서버 설정만 검사하기

네트워크 연결 단계에서 사용할 TOML 설정은 어느 OS에서든 따로 검사할 수 있습니다.

```sh
cargo run -p edgebridge-server -- --check-config --config config/server.example.toml
```

설정만 검사하고 종료합니다. 입력을 수집하거나 포트를 열지 않습니다. `--capture-log`와 `--check-config`는 함께 사용할 수 없습니다.

## 개발과 검증

저장소 루트에서 실행합니다.

```sh
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Mac에서 Windows 코드의 타입/빌드 조건 검사. Windows 링크·실행 검증은 아님.
rustup target add x86_64-pc-windows-msvc
cargo check --workspace --all-targets --target x86_64-pc-windows-msvc
cargo clippy --workspace --all-targets --target x86_64-pc-windows-msvc -- -D warnings
```

현재 자동 검증은 프로토콜, 설정, handshake/heartbeat, Mac 세션·복구, Windows 이벤트 정규화·queue·통계, 서버 CLI의 입력 오류 처리를 다룹니다. 실제 마우스·키보드 hook이나 Quartz 권한을 자동 테스트에서 실행하지 않습니다.

다음 순서는 **M1 남은 실기기 확인 → M2 Windows 수집 실기기 확인 → M3 Windows→Mac 마우스 연결**입니다. M2의 별도 입력 억제 실험은 아직 구현하지 않았고, Raw Input + LL hook 조합의 채택도 실험 전까지 Proposed입니다. 일상 사용 기준은 오류 복구(M6)와 인증·암호화(M7)까지 통과하는 것입니다.

## 코드와 설계 문서

| 위치 | 역할 |
|---|---|
| `crates/protocol` | frame codec, 메시지, 입력값 제한 |
| `crates/core` | 설정, handshake/heartbeat, 세션·좌표·입력 상태 |
| `crates/platform-windows` | Windows hook·Raw Input 수집과 진단 |
| `crates/platform-macos` | macOS 권한·화면 정보·Quartz 마우스 주입 |
| `crates/server` / `crates/client` | 각 OS의 실행 프로그램 |
| `crates/test-peer` | 실제 장비 연결 전 시험할 가짜 상대 프로그램 |

- [ARCHITECTURE](docs/ARCHITECTURE.md): 컴포넌트, 상태·좌표·복구 설계
- [PROTOCOL](docs/PROTOCOL.md): 자체 wire 형식과 메시지 계약
- [Windows](docs/PLATFORM-WINDOWS.md) / [macOS](docs/PLATFORM-MACOS.md): OS backend와 검증
- [ROADMAP](docs/ROADMAP.md): M0–M8의 관측 가능한 완료 조건
- [KEYMAP](docs/KEYMAP.md): 키·modifier·한영 입력 설계
- [DECISIONS](docs/DECISIONS.md) / [OPEN-QUESTIONS](docs/OPEN-QUESTIONS.md): 결정 근거와 미해결 항목

## Coding Challenges와의 관계

출발점은 [Coding Challenges #142: Software KVM](https://codingchallenges.substack.com/p/coding-challenge-142-software-kvm)입니다. 원문은 **Barrier 프로토콜 호환**을 요구하지만, EdgeBridge는 이 Windows–Mac 구성에 맞춘 자체 프로토콜로 개발합니다. 실제 Barrier와 호환되지 않으며, 챌린지 전체를 완료했다고 주장하지 않습니다. 호환 adapter는 [별도 track](docs/ROADMAP.md#barrier-호환을-원하면-필요한-별도-track)입니다.

참고 구현: [lan-mouse](https://github.com/feschber/lan-mouse), [Input Leap](https://github.com/input-leap/input-leap), [Barrier](https://github.com/debauchee/barrier). 코드 복사·의존성 도입 시 라이선스는 별도로 확인합니다.
