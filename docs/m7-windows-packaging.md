# Windows 패키징과 키 운영

공개 소스는 [dreamrugi-worldbuild-tool](https://github.com/skais5384-jpg/dreamrugi-worldbuild-tool)의 main에서 관리합니다. 실제 시험판 배포 절차는 [배포 안내](release-guide.md)를 참고하십시오.

## 요구 환경

Windows x64, PowerShell 7, Node.js 24.20.0, Rust 1.98.0, Tauri CLI 및 Windows Tauri 사전 요구 사항을 준비합니다. MSIX 시험에는 Windows SDK의 MakeAppx와 SignTool도 필요합니다. `npm ci`는 npm lockfile의 의존성을 설치합니다. Cargo lockfile·제품 버전·GPL-3.0-only와 동봉 리소스·고지를 유지합니다.

## GitHub와 Store 시험 채널

GitHub 채널은 Windows x64 NSIS를 생성합니다. 정식 앱 식별자는 `com.dreamrugi.worldbuildtool`, 기본 버전은 `0.1.0`입니다. 현재 사용자 범위로 설치하고 필요할 때 WebView2를 다운로드합니다. 오프라인 PC는 WebView2를 미리 준비하십시오.

`Github/Test`와 `Store/Test`는 공식 설치본과 다른 시험 identity를 사용합니다. 시험 키나 marker 변경으로 배포 파일을 승격하지 않습니다. `Store/Test`는 로컬 적합성 확인용 MSIX이며 실제 Store 상품 identity가 아닙니다. Partner Center 상품 identity와 제출 후보가 확정되기 전 Store Release 빌드는 거절됩니다. MSIX 버전은 `major+1.minor.patch.0`으로 대응하며, 세 부분은 65534 이하입니다.

```powershell
pwsh -File scripts/package-windows.ps1 -Channel Github -Mode Test -OutputDirectory <새 시험 출력 폴더> -UpdaterKeyPath <시험 키 절대 경로>
pwsh -File scripts/package-windows.ps1 -Channel Store -Mode Test -OutputDirectory <새 시험 출력 폴더> -MsixThumbprint <시험 인증서 지문>
```

GitHub 공개 후보는 [고정 source 빌드 절차](release-guide.md#고정-소스에서-빌드하기)로 생성합니다. 개인 키와 설치 산출물은 Git source에 넣지 않습니다. updater 서명은 파일 검증용이며 Windows Authenticode나 MSIX 게시자 인증서를 대신하지 않습니다. 자동 업데이트는 현재 앱에 없습니다.

## 소스·키·설치 자료의 보존

`package.json`, `package-lock.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`의 버전과 라이선스가 같아야 합니다. 공개 후보는 source와 package/asset manifest의 크기·SHA-256 및 공개키로 검증합니다. 실행에 필요한 폰트·사전·리소스와 제3자 원문 고지는 정확한 source에 포함합니다.

배포 키는 암호화해 source 밖에 보관하고 별도 백업에서 복원·표본 서명·공개키 일치를 확인합니다. 키 암호를 명령줄 인자나 로그에 남기지 않습니다. 사용자의 프로젝트·복구 입력과 유일 자료는 설치·제거 시험에 사용하지 않습니다.

Store 상품 제출은 완성된 1.0.0 후보에서 다룹니다. 현재 Store 다운로드 링크는 없습니다. 일반 사용 안내는 [Wiki](https://github.com/skais5384-jpg/dreamrugi-worldbuild-tool/wiki)를 참고하십시오.
