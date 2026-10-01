# M7 Windows 패키징과 키 운영

## 현행 F 구현 후보

E 반영 완료, 공식 **7/9·커밋5/6**. F는 고정 소스/수동 draft 경로·암호화 배포 키의 독립 백업 복원·정식 identity 후보를 준비한다. [배포 안내](release-guide.md)의 로컬 공통 경로와 공개 목적지 비교를 따른다. 실제 키 입력/사용자 확인·원격 Actions/draft·명시 공개는 결과별로 구별한다. Release 키는 검증 receipt가 필요하며 시험 marker를 바꾸는 방식으로 승격하지 않는다.

## 두 채널

같은 소스에서 Windows x64 NSIS(`Github`)와 로컬 MSIX(`Store`)를 빌드한다. `scripts/package-windows.ps1`이 `package.json`, `package-lock.json`, `Cargo.toml`, `tauri.conf.json`의 제품 버전과 `GPL-3.0-only`를 먼저 대조한다. 기본 버전은 0.1.0이다. 다음 버전 시험은 `-Mode Test -VersionOverride 0.1.1`처럼 명시한다. Store용 네 자리 버전은 `major+1.minor.patch.0`이며 세 부분은 65534 이하로 제한한다. 이 대응은 MSIX 첫 자리를 0으로 두지 않고 Store용 넷째 자리를 0으로 고정하기 위한 것이다.

`Github/Test`는 기존 설치본과 충돌하지 않는 `com.dreamrugi.worldbuildtool.e.localtest` 식별자 및 `Dreamrugi Worldbuild Tool E Local Test` 이름을 사용한다. 기본 공개 식별자 `com.dreamrugi.worldbuildtool`은 변경하지 않는다. `Store/Test`의 `Dreamrugi.WorldbuildTool.ELocalTest`와 시험용 게시자 인증서는 Partner Center에서 배정한 실제 상품 identity가 아니다. 실제 Store 상품 연결 전에는 Release 모드로 MSIX를 만들 수 없다. 빌드 시 채널은 앱 정보에도 표시되고, 잘못된 빌드 채널/모드 조합은 Cargo build script가 거절한다. 자동 업데이트 코드는 현재 앱에 없으며 Store 채널은 GitHub 업데이트 키 입력을 거절한다.

PowerShell 7, Node/npm, Rust/Cargo, Tauri CLI와 Windows SDK의 MakeAppx/SignTool이 필요하다. NSIS는 Tauri CLI가 관리한다. `npm ci` 후 저장소 루트 또는 다른 현재 디렉터리에서 다음을 실행한다.

```powershell
pwsh -File C:\dev\worldbuild-tool\scripts\package-windows.ps1 -Channel Github -Mode Test -UpdaterKeyPath <복원된 시험 키의 절대 경로>
pwsh -File C:\dev\worldbuild-tool\scripts\package-windows.ps1 -Channel Store -Mode Test -MsixThumbprint <시험 인증서 지문>
```

각 명령은 `logs/M7-7-IMPLEMENTATION-001/packages/<채널>-<모드>-<버전>/`에 설치 파일, 서명 파일(해당 채널), 메타데이터, 빌드 설정을 둔다. 실제 배포 파일이나 개인 키를 Git에 추가하지 않는다. NSIS는 현재 사용자 범위와 WebView2 다운로드 부트스트래퍼를 사용한다. MSIX는 Windows SDK로 전체 실행 파일·리소스·원문 고지를 포장하고 설치용 시험 인증서로 서명한다. 사용자 설치/제거 시험은 이 명확한 로컬 identity로만 수행한다.

## 업데이트 서명 시험 키

Tauri CLI 2.11.4의 `signer generate`는 암호 입력을 명령줄 인자로 받을 수 있지만, 명령줄에 비밀을 남기지 않기 위해 로컬 시험은 `scripts/new-local-updater-test-key.ps1`로 생성한 키를 현재 Windows 사용자 DPAPI로 보호한다. 이 시험용 키는 공개용 키가 아니다. `Generate`는 키쌍을 만든 직후 개인 키를 DPAPI 백업으로 봉인하고 평문 원본을 제거한다. `Restore`는 서명 시험 직전에 동일 Windows 사용자 계정으로 복원하고, `Seal`은 평문을 다시 제거한다. DPAPI 백업만 다른 PC로 복사하면 복원할 수 없으므로 실제 운영 키의 오프라인 백업을 대신하지 않는다.

```powershell
$keyDir = Join-Path $env:LOCALAPPDATA 'WorldbuildTool-M7-E-ProtectedKeys-20260927'
pwsh -File scripts/new-local-updater-test-key.ps1 -Directory $keyDir -Action Generate
pwsh -File scripts/new-local-updater-test-key.ps1 -Directory $keyDir -Action Restore
try { pwsh -File scripts/package-windows.ps1 -Channel Github -Mode Test -UpdaterKeyPath (Join-Path $keyDir 'updater.key') }
finally { pwsh -File scripts/new-local-updater-test-key.ps1 -Directory $keyDir -Action Seal }
```

실제 공개 전에 사용자가 소유한 **별도의 암호 보호된 Tauri 업데이트 키**를 안전한 로컬 대화형 절차로 생성하고, 개인 키·암호·복구 자료를 접근 제한된 서로 다른 보관소에 백업한 뒤 복원 시험을 해야 한다. 공개키는 앱의 업데이트 설정에 고정하고 해당 파일/서명 쌍을 검증한다. 개인 키 또는 암호 분실 시 기존 설치본이 신뢰하는 키로 새 업데이트를 서명할 수 없고, 노출 시 악성 업데이트 서명 위험이 있다. 이때 배포를 중단하고 키 교체·사용자 안내 경로를 검토한다. `TAURI_SIGNING_PRIVATE_KEY`/`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`는 빌드 프로세스 범위에서만 제공하고 로그, Git, 채팅, workflow 원문에 넣지 않는다. 공개용 키·실제 백업·GitHub secret 등록은 아직 완료되지 않았다.

MSIX 시험용 인증서는 `Cert:\CurrentUser\My`의 별도 자체 서명 CodeSigningCert다. `SignTool verify`는 현재 사용자 신뢰 저장소에서 성공해도 AppX 설치 서비스는 이를 받아들이지 않을 수 있다. 실제 MSIX 설치에는 이 공개 인증서 한 개를 관리자 승인 아래 `Cert:\LocalMachine\TrustedPeople`에 등록해야 한다. 기기 전체 사용자에게 영향을 주므로 시험 종료 뒤 정확한 지문을 확인해 제거한다. 개인 키를 기기 신뢰 저장소로 복사하지 않는다. 이 인증서는 Tauri 업데이트 키나 향후 Store 게시자 자격을 대체하지 않는다. Windows Authenticode 서명 없는 GitHub 시험판 정책과도 별개다.

## 설치본 데이터 위치

프로젝트의 `templates/`, `documents/`, `resources/`와 내부 `.worldbuild/`는 사용자가 선택한 프로젝트 폴더에 남는다. GitHub NSIS의 복구 Store와 앱의 진단·프로젝트 잠금·WebView2 데이터는 해당 앱의 현재 사용자 LocalAppData 아래에 둔다. MSIX의 패키지별 LocalCache는 제거할 때 삭제되므로, **Store 빌드의 보관 편집 복구본만** `%USERPROFILE%\.worldbuild-tool\store-local-test\edit-recovery`(시험본) 또는 `%USERPROFILE%\.worldbuild-tool\store\edit-recovery`(향후 실제 Store identity)의 별도 사용자 프로필 경로에 둔다. 이 폴더에는 민감한 미저장 내용이 있으므로 백업·기기 이동 시 프로젝트 폴더와 함께 관리하며, 앱 제거로 자동 삭제하지 않는다. 앱을 재설치하면 같은 사용자와 채널의 보관본을 다시 읽는다.

설치본에서 Store가 Windows 패키지 별칭 경로를 받으면 물리 디렉터리와의 동일성 검사가 실패할 수 있어, `app_paths::local_data`와 `recovery_root`가 경로를 만든 뒤 정규화한다. 설정과 SVN 인증 캐시는 사용자의 기존 TortoiseSVN/Windows 위치를 따르며 설치 프로그램이 삭제하지 않는다. MSIX의 기존 패키지 내부 복구본은 제거 뒤 OS가 지우므로 이 수정 이전의 손실된 보관본은 자동 복원할 수 없다. 정본 프로젝트 파일은 영향을 받지 않는다.

FIX002에서는 알려진 이전 시험 MSIX의 패키지 내부 `edit-recovery`가 존재하면 새 앱의 시작 setup에서 외부 Store로 동기 인계한다. 복구 센터를 열지 않고 홈에서 바로 정상 종료해도 인계 완료 또는 실패 상태를 확정한다. 충돌·손상·쓰기 실패는 구본을 보존하고 홈의 주의와 `보관 재확인`으로 알린다. 이 단계는 초안을 보관할 뿐 프로젝트에 자동 복원하지 않는다. 새 앱을 업그레이드 후 **한 번도 실행하지 않고** OS에서 제거하면 앱 시작 코드가 동작하지 않으므로 구 패키지 내부 자료의 보존을 보장하지 않는다. 제거 전 새 앱 실행·보관 완료 확인이 필요한 이유다.

## 동봉 자료와 대응 소스

앱 실행 파일 외에 Hunspell runner·한글 사전·각 원문 라이선스·Hunspell/사전 소스 archive·runner 코드/빌드 배치·프런트엔드에 포함된 폰트/고지·루트 GPLv3/README가 필요하다. `tauri.conf.json`의 `bundle.resources`가 두 패키지 공통 입력이며 MSIX 스크립트가 같은 목록을 복사한다. `LICENSE`는 앱 자체 GPLv3이며 제3자 원래 조건은 `src-tauri/about-notices/`, `src-tauri/spellcheck/`, `src/assets/fonts/LICENSE`에서 별도로 유지된다. 각 공개 바이너리에는 정확히 해당 소스 revision, npm/Cargo lockfile, 패키징 스크립트, 적용한 변경과 제3자 필요한 소스를 대응시켜야 한다. E는 feature commit 557c9304e144835b80999f0e315ce125a839af75로 반영됐다. F 구현 후보는 이 commit과 명시 overlay를 함께 식별하며 E commit만의 archive를 F 대응 소스로 제시하지 않는다.

WebView2 런타임과 TortoiseSVN은 앱 설치 파일의 일부가 아니다. Windows의 기존 설치와 별도 배포 규칙을 따른다. TortoiseSVN의 전역 인증 캐시를 패키지 설치/제거가 수정하지 않는다.

`python scripts/export-corresponding-source.py <출력 ZIP> --ref <40자리 commit> --overlay <명시 manifest>`는 고정 commit과 bytes/SHA-256으로 검증한 파일만 개발/감사용 ZIP에 모은다. 공개 배포에는 `release-candidate.py snapshot`의 제품/빌드 allowlist를 사용한다. 무관한 작업 트리·미추적 자료와 내부 실행 기록을 자동 수집하지 않는다. `python scripts/export-third-party-source.py <출력 ZIP>`은 잠긴 Cargo registry 패키지 원본과 현재 설치된 npm 실행 의존 패키지 원본을 별도 ZIP/manifest로 모은다. 두 ZIP 모두 공개 전에 빌드 입력을 고정한 뒤 다시 실행해 해당 바이너리 해시와 연결한다. 출력은 보통 무시되는 `logs/` 아래에 두며 로그, 개인 키와 계정 파일을 포함하지 않는다. 두 번째 ZIP은 빌드 도구 자체나 Windows 시스템 라이브러리 소스의 대체물이 아니다.

현재 lockfile·설치 트리 조사에서는 Cargo 비루트 패키지 455개와 npm 실행 의존 패키지 129개 모두 `license` 메타데이터가 있었다. 이는 라이선스 적합성의 자동 승인 근거가 아니며, 실제 포함 바이너리·고지·폰트/사전의 조건은 별도로 검토한다. 공개 시에는 E/F의 수용 커밋·태그를 기준으로 두 ZIP과 고지를 다시 생성하고 실제 설치 파일별 대응 관계를 확인한다.
