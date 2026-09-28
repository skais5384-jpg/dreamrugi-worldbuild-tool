# Windows 0.x 시험판 배포 안내

공개 저장소는 [skais5384-jpg/dreamrugi-worldbuild-tool](https://github.com/skais5384-jpg/dreamrugi-worldbuild-tool)이며 기본 브랜치는 `main`입니다. 현재 첫 0.1.0 시험판 배포를 준비 중입니다. 일반 사용자 다운로드는 게시된 [Releases](https://github.com/skais5384-jpg/dreamrugi-worldbuild-tool/releases)에서 제공하며, draft는 일반 사용자 설치 링크로 안내하지 않습니다.

## 고정 소스에서 빌드하기

PowerShell 7, Python 3.10 이상, Node.js 24.20.0, Rust 1.98.0과 Tauri 2 Windows 사전 요구 사항을 준비합니다. 버전은 `0.1.0`, 앱 식별자는 `com.dreamrugi.worldbuildtool`입니다. npm/Cargo lockfile과 원래 GPLv3·제3자 고지를 유지합니다.

```powershell
python scripts/release-candidate.py snapshot --ref <40자리 공개 commit SHA> --version 0.1.0 --output <새 소스 폴더>
pwsh -File scripts/manage-release-key.ps1 -Action Build -Directory <배포 키 폴더> -Snapshot <새 소스 폴더> -OutputDirectory <새 asset 폴더>
python scripts/release-candidate.py draft --folder <asset 폴더> --repo skais5384-jpg/dreamrugi-worldbuild-tool --target <같은 공개 commit SHA> --tag v0.1.0
```

마지막 명령은 미리보기입니다. 원격 draft 실행에는 `--execute --expected-public-key-sha256 <신뢰한 공개키 지문>`이 필요합니다. 새·재개 draft 모두 정확한 예정 tag ref가 없어야 하며 target, candidate marker와 모든 asset을 확인합니다. 다른 후보·기존 tag·다른 asset은 덮어쓰지 않습니다. 같은 후보의 동일 hash asset만 재사용합니다. 업로드 후에도 metadata·ref·asset을 다시 확인하며, 실패한 중간 결과는 보존합니다.

`SOURCE.json`과 `release-manifest.json`은 실제 빌드의 공개 commit/tree, 버전, candidate와 파일 크기·SHA-256을 연결합니다. 과거 후보의 provenance나 서명을 새 commit의 결과처럼 사용하지 않습니다. 공개 source는 `packaging/public-source-policy.json`의 허용 파일에서 생성합니다.

## 배포 키와 보호 환경

배포 키는 사용자가 암호화해 저장소 밖에서 관리합니다. `packaging/updater-public.key.pub`와 비밀 없는 receipt만 source에 포함됩니다. 기존 공개키 packet SHA-256은 `550ed07e613b88b7dfdd10c88c51183210f1a407b6b0cf5e43bd7dce231cd0f7`입니다. 키 암호는 채팅·명령 인자·공개 로그에 넣지 않습니다. 키와 별도 백업·암호를 안전하게 보관하고 분실·노출 시 배포를 중단해 신뢰 이동 절차를 검토합니다.

GitHub Actions의 `prerelease-signing`과 `prerelease-draft` 환경은 소유 사용자의 required reviewer, main branch만 허용, 관리자 우회 금지로 구성합니다. 한 사람 운영에서는 self-review를 허용하되 매 단계의 사람 승인을 유지합니다. 환경 보호 규칙을 내려 실행을 통과시키지 않습니다.

| 설정                                   | 위치                                             |
| -------------------------------------- | ------------------------------------------------ |
| `WORLDBUILD_UPDATER_PRIVATE_KEY`       | `prerelease-signing` 환경 secret: 기존 암호화 키 |
| `WORLDBUILD_UPDATER_KEY_PASSWORD`      | 같은 환경 secret: 키 암호                        |
| `WORLDBUILD_UPDATER_PUBLIC_KEY`        | repository variable: 전체 공개키 문자열          |
| `WORLDBUILD_UPDATER_PUBLIC_KEY_SHA256` | repository variable: 공개키 packet 지문          |

## 수동 Actions와 게시

`Windows prerelease draft` workflow를 main에서 수동 실행합니다. `source_sha`는 검토한 공개 main HEAD의 40자리 SHA, `version`은 `0.1.0`입니다. validate는 비밀 없이 소스·버전을 확인하고, signing 환경 승인 뒤 build가 설치 파일·서명·소스 묶음을 생성합니다. draft 환경의 별도 승인 뒤 업로드합니다. validate/build는 contents read, draft job만 contents write를 갖습니다.

업로드하는 10개 asset은 설치 EXE, `.sig`, `corresponding-source.zip`, `third-party-source.zip`, `third-party-source.manifest.json`, `LICENSE`, `README.md`, `release-notes.md`, `updater-public.key.pub`, `release-manifest.json`입니다. `SOURCE.json`과 package metadata 등 빌드 추적 자료는 workflow artifact에서 확인합니다.

draft의 target/candidate·예정 tag ref 부재·각 API digest와 실제 다운로드, 대응 소스·서명·작은 설치 표본을 검증한 뒤 사람이 게시 여부를 판단합니다. draft 생성은 Release 게시나 stable/latest 승격이 아닙니다. Windows Authenticode 미서명과 updater 서명을 구별합니다. 앱 내 자동 업데이트와 Store 배포는 현재 제공하지 않습니다.

## 사용자 안내

일반 사용법은 [Wiki](https://github.com/skais5384-jpg/dreamrugi-worldbuild-tool/wiki)에 있습니다. 개인 프로젝트와 SVN 작업 사본, 로컬 저장과 서버 커밋을 구별하십시오. 외부 SVN Revert/rollback을 수행하려면 입력을 먼저 보존하고 앱을 정상 종료한 뒤 외부 도구에서 변경하고 재열어 현재 원본을 확인하십시오.
