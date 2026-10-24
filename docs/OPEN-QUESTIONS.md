# 미정 사항과 검증 실험

이 목록은 현재 문서를 읽기 위한 추가 답변 요구가 아니다. 답이 없어도 아래 기본값으로 M0/M1 설계를 진행할 수 있다. 구현 중 각 항목을 결정하면 근거·환경·관측 결과를 DECISIONS에 연결한다.

## 먼저 사용자 의도가 필요한 항목

| 우선순위 | 질문 | 현재 기본값 | 결정 시점 / 영향 |
|---|---|---|---|
| P0 | 원문 Barrier 상호운용까지 완수할 것인가, 개인 EdgeBridge 완성이 먼저인가? | 자체 protocol 우선 | M0 전 재확인 권장. 호환 필수라면 codec/fixture/roadmap이 달라짐 |
| P1 | Ctrl를 Mac Command로 바꿀 것인가? | physical; ctrl-cmd-swap 선택 가능 | M4 전. Terminal Ctrl+C와 일반 앱 Cmd+C의 tradeoff |
| P1 | 실제 Windows 키보드는 ANSI/한국어 103/106-key 중 무엇이며 OS layout은? | 관측 전 미정 | M2/M4. RightAlt·한영·한자 normalization 결정 |
| P1 | 정상 화면 전환 때 모든 키/버튼을 놓아야 하는 제한을 수용하는가? | all-up 정책 | M5. held transfer를 원하면 별도 transaction 설계 |
| P1 | 외장 모니터/도킹/클램셸을 처음부터 지원해야 하는가? | 내장 Mac + Windows 단일 선택 화면 | M1/M5. stable display selector와 topology 범위 |
| P2 | 인증은 공개 지문을 양쪽에 수동 등록하는 것으로 충분한가? | 수동 pairing | M7. QR/PIN/UI 필요 여부 |
| P2 | clipboard 자동 동기화인가, 수동 전송인가? | M8 opt-in active-screen text sync | M8. 원치 않는 덮어쓰기와 민감 데이터 정책 |

## 구현을 막을 수 있는 플랫폼 실험

### Q1. Raw Input과 LL suppression을 함께 써도 되는가? — P0 / M2

환경을 기록하고 LL callback이 1을 반환하는 5초 제한 실험에서 raw 상대 delta를 수집한다. edge에 커서를 둔 채 이동, foreground 전환, 빠른 move+click, 고속 polling을 포함한다. 통과 기준은 delta 지속 수신, 중복 click 0, click 위치 오차 목표 <=4 logical units, callback timeout/큐 성장 없음. 실패 시 mouse discrete event까지 Raw 경로로 통합하는 대안 또는 center-pin을 비교한다. Microsoft 문서가 이 조합의 모든 driver별 순서를 보증한다고 간주하지 않는다.

### Q2. Mac TCC와 좌표 단위는 실제 배포 형태에서 어떤가? — P0 / M1

Terminal 실행, 고정 경로 CLI, 추후 app bundle 각각 권한 주체를 기록한다. Accessibility만 허용한 상태에서 주입, 거부/철회, 재빌드·경로 변경 후 상태를 확인한다. 내장 Retina 기본/다른 scaled mode에서 CGDisplayBounds와 mode pixel size, CGEvent 요청/관측 cursor 좌표를 비교한다. pixel 배율을 주입 좌표에 잘못 적용하지 않았다는 증거가 필요하다.

### Q3. client crash 중 Down 상태는 어떻게 복구되는가? — P0 / M6

Ctrl/Shift/Left button을 각각 누른 상태에서 정상 종료, SIGTERM, SIGKILL, 강제 crash를 구분하여 시험한다. server local 복귀와 Mac held 해제를 별도로 관측한다. destructor가 실행되지 않는 경우 OS가 자동 release하는지 추정하지 않는다. stuck이 남으면 별도 helper/watchdog가 필요한지, 또는 프로젝트를 실험용으로 제한할지 결정한다. helper도 같은 TCC 권한과 identity/IPC 인증이 필요하므로 공짜 해결책이 아니다.

### Q4. hook이 조용히 제거되었음을 어떻게 다룰 것인가? — P1 / M2, M6

의도적 callback 지연은 disposable test에서만 수행한다. Windows의 silent removal은 신뢰할 수 있는 직접 notification이 없으므로 hook progress가 없다는 사실과 사용자가 입력하지 않는 상태를 구분하기 어렵다. raw-vs-hook 활동 discrepancy를 진단할 수 있는지 검토하되 완전 탐지라고 부르지 않는다. 재설치는 Local 상태에서만 수행한다. stalled control thread 중 atomic lease가 실제 local 통과를 회복시키는지도 측정한다.

### Q5. repeat / modifier flags / Caps Lock — P1 / M4

server Repeat와 macOS injected keydown의 자동 반복이 겹치는지 TextEdit/Terminal에서 5초 hold로 확인한다. 좌우 modifiers, modifier up event flags, Command-click/Shift-drag를 확인한다. Caps Lock은 toggle state·LED·IME 전환 효과를 별도로 시험한다. 결과 전에는 일반 키와 동일한 기능이라고 광고하지 않는다.

### Q6. high-resolution scroll / double-click / 복합 drag — P1 / M3

fractional wheel delta가 작은 값으로 여러 번 왔을 때 총량이 보존되는가? wire content direction과 Windows wheel/Quartz 부호를 실제 스크롤 콘텐츠로 비교한다. natural scrolling 옵션 영향은 앱별로 기록한다. double-click interval은 Mac 설정을 사용하지만 거리 4-unit은 실험값이므로 Finder/TextEdit 기본 동작과 비교한다. 여러 버튼 drag owner 정책이 필요한 앱을 망가뜨리지 않는지 확인한다.

## 나중에 확정할 운영 결정

- **OS 최소 버전:** 사용 장비 OS build를 먼저 기록. Swift/ObjC bindings API availability와 TLS provider 지원에 맞춰 명시. “최신 OS”를 영구 지원 기준으로 쓰지 않는다.
- **crate 선택:** Mac core-graphics/objc2-core-graphics의 실제 API coverage와 ownership 간결성을 M1에서 비교. Windows는 공식 windows bindings 기준. Cargo workspace를 만드는 시점에 version/MSRV를 고정한다.
- **감도/가속:** count→logical unit 1.0, no acceleration은 기본 실험값. Windows/Mac 사용감 일치가 필요하면 별도 curve/물리 DPI calibration. Retina 배율로 감도를 억지 보정하지 않는다.
- **timeout:** 250ms Ping/1000ms lease/100ms queue age는 LAN 초기 정책. Wi-Fi 지터로 false fail이 많으면 정상 부하와 장애 복구 시간을 함께 측정해 조정. timeout 완화가 stuck 시간을 늘린다는 tradeoff를 기록한다.
- **TLS identity:** leaf pin vs private CA trust 구조, 인증서 만료/시계 오차, 키 분실·교체·폐기 UX, macOS Keychain/Windows key protection. signature 검증을 생략하지 않는 rustls integration 확인.
- **secure desktop/잠금:** MVP는 로그인 user session만. UAC/login window 지원을 위해 무조건 관리자·root로 실행하지 않는다.
- **로컬 Mac 입력 takeover:** 현재 자동 감지 없음. 도입하면 capture/Input Monitoring 여부, server에 Leave를 요청하는 wire, injection-loop 필터가 추가로 필요하다.
- **multi-monitor:** 서로 다른 DPI, 빈 공간, 회전, mirroring, 선택 edge가 다른 monitor와 맞닿는 경우를 다룰 screen graph가 필요하다. 단일 bounding box 확장으로 끝나지 않는다.
- **clipboard:** simultaneous copy winner, stale offer 취소, active ownership 전환 시 기존 내용 publish 여부, input과 bulk 분리. v1.1 전에 확정한다.

## 완료 기록 형식

각 실험에는 `질문 / OS·하드웨어·버전 / 절차 / 예상 / 실제 / 실패 재현 / 결정한 정책 / 관련 ADR`을 남긴다. fake peer pass, cross-build pass, 실제 OS injection pass는 서로 다른 증거다. 실패를 숨기기 위해 수락 기준을 사후 낮추지 않으며 변경한 기준에는 이유를 남긴다.
