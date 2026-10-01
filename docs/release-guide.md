# Windows 1.0.0 정식 배포와0.x 시험판 이력 안내

현행: M9/MVP 개발 마감과 V1-RELEASE-PREP-AUDIT-001의 배포 준비 핵심 수용을 바탕으로1.0.0 게시 문구와 실행 대상을 마감한다. GitHub 공개와 Store 제출은 별도 지시로 실행하며, 이 안내 자체는 실행 완료를 뜻하지 않는다. 실제 public SHA/CI/draft/asset/download/feed와 Store 설치·제출 상태는 해당 작업 receipt를 따른다. [v0.1.0 수동 설치 시험판](https://github.com/skais5384-jpg/dreamrugi-worldbuild-tool/releases/tag/v0.1.0)은 source307에 고정한 과거 배포다. 공개0.1.0은 updater/팀 버전 정책이 없어 첫1.0.0 전환은 수동 설치한다. 개발0.2.0은 공개 다운로드가 아닌 과거 개발 기준이며 자세한 이력은 [M8 배포 준비](m8-release-readiness.md)에 남긴다.

## 정식판 고정 소스와 실행 경계

V1-RELEASE-FINALIZE-001은 수용된 로컬 준비 소스의 좁은 게시 문구·설치/검사 설명 마감입니다. 별도 실행 지시 전에는 원격 반영·tag/Release/draft·workflow·feed·Store 제출을 하지 않습니다. 0.x는 prerelease=true, 정식1.x 이상은 prerelease=false이며 검토용 준비 요청은 draft=true·make_latest=false를 유지합니다. 정식 .sig에는 인증된 trusted comment version이 앱/manifest와 같아야 합니다. 이전0.x 서명 검증은 그대로 유지합니다.

Store/release에는 실제 Partner Center identity/history JSON을 StoreIdentityPath로 지정합니다. 확인한 상품은 Dreamrugi.DreamrugiWorldbuildTool / Store ID9N6FCVJ83ZLN이며 최초 제출 이력 없음에 따라 앱1.0.0/패키지1.0.0.0입니다. 값/이력 미확인은 빌드 전에 거절합니다. updater 키/암호는 Store 준비/빌드에 공급할 수 없습니다. 제출용 unsigned MSIX와 선택적인 MsixThumbprint로 만든 local-signed 사본을 구분하며 호스트 신뢰 등록은 하지 않습니다. 격리 시험 환경의 실제 설치·자료 보존과 제출 단계에서의 Store 재서명을 구별합니다. 기존 family는 확인한 최상위 버전보다 높은 명시 버전을 사용합니다. Store/test의 major+1은 ELocalTest 전용입니다. 상세는 packaging/store-submission.md를 확인합니다.

이번 private SHA의 공개 export와 공개 저장소 SHA는 서로 다릅니다. 독립 검토→별도 반영 지시→public 기본 branch의 고정 SHA로 서명 재빌드→공개 asset 지문 검증→feed 일치/게시 순서입니다. 이 로컬 후보의 설치 시험을 미래 public SHA 빌드의 성공으로 소급하지 않습니다.

## 고정 소스와 로컬 경로

PowerShell 7, Python 3.10 이상, Node.js 24.20.0, Rust 1.98.0과 Windows Tauri 사전 요구 사항을 준비한다. GitHub actions는 검토된 commit에 고정한다. npm/Cargo 잠금 파일과 제품 버전·GPLv3·리소스 목록을 대조한다.

```powershell
python scripts/release-candidate.py snapshot --ref <40자리 수용 commit> --version 1.0.0 --output <새 소스 폴더>
pwsh -File scripts/manage-release-key.ps1 -Action Build -Directory <배포 키 폴더> -Snapshot <새 소스 폴더> -OutputDirectory <새 asset 폴더>
python scripts/release-candidate.py draft --folder <asset 폴더> --repo <확정 owner/repo> --target <같은 commit> --tag v1.0.0
```

마지막 명령은 요청/asset 미리보기만 만든다. 원격 실행은 별도 승인된 단계에서 `--execute --expected-public-key-sha256 <신뢰한 공개키 지문>`을 붙인다. 기술 후보·미커밋 overlay 후보는 원격 생성에서 거절한다. 다른 후보 draft·기존 tag·변경 asset은 덮어쓰지 않으며, 같은 후보 draft의 동일 hash asset만 재사용한다. 항상 draft이며0.x만 prerelease다. 최신판 승격과 publish 코드는 없다.

신규·재개 모두 정확한 `refs/tags/{tag}`가 이미 있으면 같은 commit/annotated tag라도 명시 조정 전 중단한다. prefix가 다른 tag는 해당 tag로 취급하지 않는다. 조회 실패는 없음으로 간주하지 않는다. 업로드 후 tag 부재·tag_name/target/marker·draft/prerelease·asset digest를 다시 확인한다. 중간 변경/확인 실패에는 이미 올린 asset을 보존하고 미완료로 처리한다. 이 조회는 원자적 동시 변경 방지가 아니므로 명시 공개 전 다시 확인해야 한다.

검증된 불변 의존 묶음을 재사용할 때 Build에 `-DependencySourceDirectory <기존 asset 폴더>`를 지정할 수 있다. 같은 HEAD와 npm/Cargo lock hash·archive hash가 일치해야 하며, bundle에서 모든 archive 파일을 다시 검증한다. 의존 source 수집만 생략한다.

구현 중에는 `--overlay <JSON>`으로 기준 commit과 명시 파일의 bytes/SHA-256을 함께 검증한다. 이 후보는 최종 커밋 소스로 표시하지 않는다. 공개 export는 `packaging/public-source-policy.json`의 제품·필수 빌드 자료만 포함한다. 전체 개발/감사 사본은 `export-corresponding-source.py --ref ... --overlay ...`로 별도 생성하며 공개하지 않는다. 소스/의존 ZIP 내부 전체 목록과 각 파일 hash도 검증한다.

키가 준비되지 않은 동안 `build-release-candidate.ps1 -TechnicalOnly`는 정식 identity의 미서명 기술 NSIS만 만든다. `.sig`와 배포용 공개키가 없으므로 공개 준비 완료로 취급하지 않는다. E Local Test 키·identity·DPAPI 백업은 이 경로의 배포 키를 대신하지 않는다.

## 사용자 소유 키·휴대 가능한 백업

새 키의 권장 위치는 `%LOCALAPPDATA%\WorldbuildTool-DeploymentKey`다. 이미 배포 키가 있으면 그대로 인수한다. 키·암호를 채팅이나 명령줄 인자에 넣지 않는다. 아래 두 단계의 암호는 사용자가 직접 대화형 터미널에 입력한다. 그동안 자동 입력·화면 조회/캡처를 멈춘다.

```powershell
pwsh -File scripts/manage-release-key.ps1 -Action Generate -Directory <저장소 밖 새 키 폴더>
pwsh -File scripts/manage-release-key.ps1 -Action BackupAndVerify -Directory <같은 폴더> -BackupDirectory <사용자가 별도 관리하는 새 백업 폴더>
```

Generate는 현재 사용자/SYSTEM만 접근하는 폴더를 만들고 Tauri의 암호 입력으로 암호화 키를 생성한다. BackupAndVerify는 별도 백업에서 새 임시 위치로 **암호화된 키**를 복원하고 작은 표본에 서명한 뒤 같은 공개키로 검증한다. 비밀 없는 receipt에 공개키 지문·백업 hash·실행 시각·휴대 가능한 백업 검증 여부를 기록한다. 원 키와 백업은 보존한다. 이는 새 기기의 실제 복원 시험과 구별한다. 암호는 키 백업과 별도로 사용자 관리한다.

확인된 receipt의 공개 내용만 `packaging/release-key-receipt.json`에 반영한다. 이번 후보는 사용자가 새 암호화 키를 생성하고 OneDrive 독립 백업·별도 위치 복원·표본 서명 검증·업로드 완료를 확인했다. 공개키는 `packaging/updater-public.key.pub`, 지문은 receipt에서 확인한다. GitHub secret 연결과 실제 CI 서명은 M7에서 완료했다. 새 기기 실제 시험은 미확인으로 유지한다. Build는 암호를 현재 프로세스에만 전달하고 종료 시 정리한다. 키 분실/노출 시 기존 신뢰를 임의 교체하지 않고 배포를 멈춰 M8의 키 변경·사용자 이동 절차를 검토한다.

## GitHub 설정과 다음 배포 실행

수동 실행에는 workflow가 대상 기본 브랜치에 있어야 한다. 새 개발 후보를 실제 Actions 성공으로 세지 않는다. M7의 환경/서명/초안/공개 실행은 완료된 이력이다. 독립 검토 → 별도 Git 반영 → 확정 목적지의 기본 브랜치 적용 → 환경/키 연결 → 실제 Actions/draft 확인 → 사용자 명시 공개 순서다. [GitHub 수동 실행 문서](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow)

| 설정                                     | 등록 위치/역할                                                        |
| ---------------------------------------- | --------------------------------------------------------------------- |
| `WORLDBUILD_UPDATER_PRIVATE_KEY`         | `prerelease-signing` 환경 secret, 암호화된 Tauri 키. build job만 사용 |
| `WORLDBUILD_UPDATER_KEY_PASSWORD`        | 같은 환경 secret, 키 암호. build step 프로세스에서만 사용             |
| `WORLDBUILD_UPDATER_PUBLIC_KEY`          | 저장소 variable, Tauri 공개키                                         |
| `WORLDBUILD_UPDATER_PUBLIC_KEY_SHA256`   | 저장소 variable, 검증한 42-byte 공개키 packet 지문                    |
| `prerelease-signing`, `prerelease-draft` | 기본 브랜치만 허용하고 사용자 required reviewer 구성 후 실행          |

validate/build는 contents read, draft만 contents write다. 임의 PR 코드에 키를 공급하지 않고 source input이 실행된 기본 브랜치의 SHA와 같아야 한다. 새 PAT는 기본 요구가 아니다. 대상 확정 전 secret을 등록하지 않는다. 공개키/receipt만 소스에 두며 secret 원문·암호를 workflow나 로그에 넣지 않는다.

| 목적지 안                                      | 공개되는 범위와 준비                                                                                                                                                   |
| ---------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 기존 `skais5384-jpg/worldbuild-tool` 공개 전환 | 현재 제품 파일 외 Git 전체 이력·이슈/첨부·PR·Actions 기록 노출을 별도 조사해야 함. 현재 파일의 공개 export 검사만으로 전환 근거가 되지 않음                            |
| **별도 공개 저장소 권고**                      | 후보 이름 `skais5384-jpg/dreamrugi-worldbuild-tool`. 검증된 public source export와 SOURCE 대응, 빌드/고지 자료만 새 이력으로 적용. 내부 개발 이슈/로그는 이관하지 않음 |

별도 공개 저장소는 생성되어 v0.1.0을 게시했다. 아래 고정 소스·서명·후속 승인 절차는 다음 배포에도 적용한다. 실제 공개 저장소 commit은 원 private commit과 달라질 수 있으므로 최종 workflow snapshot은 공개 저장소의 수용 SHA에서 다시 빌드한다. 원 소스와 public export의 파일 hash 매핑을 별도로 남긴다. 다음 배포 후보 공개·운영 채널 활성화는 후속 사용자 지시 범위다.

## M8과 Store 인계

### 설치가 실패했을 때

설치 프로그램에 오류가 표시되면 오류 문구를 남기고 설치 창을 닫습니다. 실행 중인 앱도 정상 종료한 뒤, 기존에 받은 신뢰할 수 있는 동일 버전 또는 더 높은 버전의 설치판으로 다시 설치합니다. 받을 설치판은 해당 버전의 실제 게시된 릴리스에서 확인합니다. 첫0.1.0→1.0.0은 수동 설치이며 이후 GitHub stable feed와 Store의 별도 업데이트 경로를 따릅니다.

재설치할 때 프로젝트 폴더와 보관 입력을 삭제하지 않습니다. 설치가 끝나면 앱을 다시 열어 기존 프로젝트와 복구 센터의 보관 입력을 확인합니다. 문제가 계속되면 좌하단 실행 기록의 오류 내용과 도움말의 진단 정보를 확인해 전달합니다. 앱은 설치 실패를 자동으로 복구하거나 이전 버전으로 되돌리지 않습니다.

M8은 GitHub 설치판의 서명된 trusted comment `version:`/metadata 일치, 내부 updater의 설치 인계·취소/실패 경계, 공개0.1.0에서 동일 identity 비공개1.0.0으로 수동 전환하는 표본, 실제 local MSIX의 팀 정책/기준 미달 차단을 검증했다. Store 사용자 안내의 원 실물 정상 판정은 사용자의 정정으로 철회했다. 후속 표시 수정은 실제 `controller.start` 회귀와 수정/기존 MSIX의 embedded frontend 대응으로 수용됐지만 수정 패키지를 새 OS에 설치해 사람이 GUI를 확인한 것은 아니다.

이 검증은 운영 키로 서명한 공개 updater·활성 운영 feed·새 public release나 실제 Store 서비스/상품 제출을 의미하지 않는다. 시험 키/identity와 최종 운영 키·Store identity는 구분한다. 공개0.1.0에는 updater가 없으므로 첫 지원판으로 수동 설치해야 하며, 앱/외부 SVN 도구가 지원하지 않는 서버 원격 강제 기능을 제공한다고 안내하지 않는다. 프로젝트 폴더와 보관 입력은 설치 실패 복구 중에도 사용자가 보존해야 한다. 신뢰할 수 있는 동일/상위 설치판의 재설치는 안내할 수 있지만 앱 다운그레이드나 SVN rollback UI는 제공하지 않는다.

Store 상품 소개 초안: **dreamrugi worldbuild tool — 세계관 개발 및 관리 도구**. 한국어 UI로 Template 기반 문서, 관계, 이미지/리소스와 프로젝트를 관리하고 설치된 TortoiseSVN 및 개인 HTTPS 계정으로 잠금·선택 커밋·명시 갱신을 지원한다. SVN 서버/계정·TortoiseSVN은 별도 준비이며 앱 자체 Revert/롤백은 제공하지 않는다. GPL-3.0-only 및 원래 제3자 고지와 정확한 대응 소스를 제공한다. 프로젝트/보관 입력은 사용자가 관리한다.

실제 Store identity와 최초 제출 이력은 확인됐고 정책·최종 스크린샷·사용자 사업/가격 결정은 제출 전에 확정한다. 정식 family는 첫 설치이므로 없는 과거 정식 버전의 업그레이드를 만들지 않는다. 격리 환경에서 실제 package identity의 시작·저장·맞춤법·백업/복원·보관 입력 생성·제거/재설치 보존을 검증한다. unpackaged 실행과 WACK 전체판정·선택 검사 설명은 OS 설치 결과와 구별한다. 예정 Store URL은 설치 링크로 쓰지 않는다. `WB-DEFER-0011/0014/0017`의 비차단 잔여는 후속 추적하며 새 전수 미관 승인을 배포 마감의 필수 과제로 넘기지 않는다.
