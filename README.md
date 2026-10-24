# EdgeBridge

Windows 데스크톱에 연결된 키보드·마우스로 왼쪽의 MacBook을 조작하는 개인용 software KVM. 화면 영상은 전송하지 않는다. 두 컴퓨터는 자기 화면을 표시하고, 입력만 LAN으로 전달한다.

**현재 상태: M0 완료, M1 구현·자동 검증 완료 / 실기기 수락 대기 (2026-10-24).** Mac client에 권한·화면 검사, Enter/절대 이동/단일 클릭·drag/Leave, ledger/epoch/lease 복구를 구현했다. 자동 테스트는 메모리 backend만 사용하며 실제 커서나 TCC를 건드리지 않는다. 실기기 injection과 Retina 오차는 아직 검증하지 않았다. [M1 수동 검증](docs/M1-VERIFICATION.md).

```text
물리 배치                   키보드 + 마우스
                                 |
+-------------------+    +-------v-----------+
| MacBook           |    | Windows desktop   |
| CLIENT: inject    | <--| SERVER: capture   |
| 개발 머신          |    |         suppress  |
+-------------------+    +-------------------+
       오른쪽 경계 <-------- Windows 왼쪽 경계: Mac 진입
       오른쪽 밖  --------> Windows 왼쪽 안쪽: 복귀

TCP 연결 시작: Mac client --> Windows server (기본 제안 포트 24810)
입력 전달 방향: Windows --> Mac / 상태·ACK: 양방향
```

## 핵심 결정

- Rust + Tokio. 공통 프로토콜/상태 머신과 OS FFI를 분리한다. 각 OS에 실행 파일 하나를 배포하되 OS framework/DLL 의존성은 존재한다.
- Mac client + fake server부터 개발한다. Windows 빌드는 Windows native를 기준으로 하고 cargo-xwin은 보조 경로다.
- Windows: low-level hooks로 키/버튼/휠 수집과 억제, Raw Input으로 상대 이동 수집. 이 조합은 M2 실험 통과 전 **Proposed**다.
- 서버가 Mac 가상 커서를 소유한다. Raw delta를 Mac 좌표 단위로 누적하고, wire에는 **절대 위치**를 보낸다. Retina backing pixel을 입력 좌표로 사용하지 않는다.
- TCP + `TCP_NODELAY`, 길이 prefix + 고정 header + postcard payload. 수동 IP, 1 server/1 client/선택 화면 1개씩으로 시작한다.
- 물리 HID usage를 전송하고 Mac에서 modifier mapping을 적용한다. 정상 화면 전환은 모든 키/버튼을 놓았을 때만 허용한다.
- 오류를 감지하면 즉시 Windows 억제를 해제하고 Mac은 자신이 주입한 키/버튼을 해제한다. 네트워크 단절 감지는 lease로 제한한다. 강제 종료 시 복구의 한계도 문서화한다.
- 평문은 명시적으로 켜는 실험 모드다. 일상 사용 전 M7의 TLS 1.3 상호 인증·수동 pairing이 필요하다.

## 읽는 순서

1. [ARCHITECTURE](docs/ARCHITECTURE.md): 컴포넌트, 좌표계, 전환·복구, TOML
2. [PROTOCOL](docs/PROTOCOL.md): wire 형식, 메시지, 순서, timeout
3. [Windows](docs/PLATFORM-WINDOWS.md) / [macOS](docs/PLATFORM-MACOS.md): API와 실험 조건
4. [KEYMAP](docs/KEYMAP.md): HID, modifier, 한영·특수 키
5. [ROADMAP](docs/ROADMAP.md): M0–M8와 수락 기준, 챌린지 단계 대응
6. [DECISIONS](docs/DECISIONS.md): 결정 이유와 대안
7. [OPEN-QUESTIONS](docs/OPEN-QUESTIONS.md): 구현 전에 결정/측정할 항목

## 빌드와 실행

toolchain은 `rust-toolchain.toml`(1.96.0)로 고정된다.

```sh
cargo fmt --all
cargo test --workspace                              # 단위 + golden + loopback 테스트
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --target x86_64-pc-windows-msvc   # Mac에서 Windows 타깃 타입 검사만

# server는 아직 config 검증 후 exit 69. 실제 client 실행은 M1 수동 검증 문서 참조.
cargo run -p edgebridge-server -- --config config/server.example.toml

# loopback fake peer: handshake + heartbeat + Disconnect
cargo run -p edgebridge-test-peer -- fake-server --bind 127.0.0.1:24899 &
cargo run -p edgebridge-test-peer -- fake-client --connect 127.0.0.1:24899 --pointer-only
```

| crate | 역할 |
|---|---|
| `crates/protocol` | frame codec, message DTO, 한계값. OS/async 의존 없음 |
| `crates/core` | handshake, heartbeat/lease, client session·ledger·좌표, epoch, HID, TOML config |
| `crates/platform-windows` / `platform-macos` | Windows stub / Quartz 권한·화면·mouse injection |
| `crates/server` / `client` | server config 검증 / Tokio network + main-thread client actor |
| `crates/test-peer` | loopback fake server/client, `--script` M1 이동/클릭/EOF/blackhole |

## 출발점과 범위 차이

[Coding Challenges #142](https://codingchallenges.substack.com/p/coding-challenge-142-software-kvm)는 실제 **Barrier wire protocol 호환**을 요구한다. 원문은 2026-10-24 HTTP로 가져와 읽었다. EdgeBridge는 학습 목표와 이 두 장비에 맞춘 자체 프로토콜을 우선한다. 따라서 챌린지의 완전한 호환 구현이라고 주장하지 않는다. 상대 이동 전송 등 의도적으로 다른 선택과 호환 adapter의 범위는 ROADMAP에 기록했다.

참고 구현: [lan-mouse (Rust)](https://github.com/feschber/lan-mouse), [Input Leap (C++)](https://github.com/input-leap/input-leap), [Barrier (C++)](https://github.com/debauchee/barrier). 설계 아이디어를 참고하며 코드 복사·의존성 도입 시 라이선스를 별도로 확인한다.
