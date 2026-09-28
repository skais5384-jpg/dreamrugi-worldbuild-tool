# Dreamrugi Worldbuild Tool

한국어 Windows용 세계관 개발 및 관리 도구입니다. 개인 프로젝트에서 Template을 만들고 문서, 관계, 이미지와 리소스, 배치를 관리할 수 있습니다. TortoiseSVN을 사용하는 팀은 SVN 작업 사본에서 문서 잠금, 로컬 저장, 선택 커밋과 명시적 갱신을 사용할 수 있습니다.

## 시작하기

**첫 0.1.0 시험판 배포를 준비 중입니다.** 아직 일반 사용자가 다운로드할 수 있는 설치 파일은 없습니다. 배포가 공개되면 [Releases](https://github.com/skais5384-jpg/dreamrugi-worldbuild-tool/releases)에 설치 파일과 대응 소스를 제공합니다.

- [사용 안내 Wiki](https://github.com/skais5384-jpg/dreamrugi-worldbuild-tool/wiki)
- [시작하기](https://github.com/skais5384-jpg/dreamrugi-worldbuild-tool/wiki/시작하기)
- [자주 묻는 질문](https://github.com/skais5384-jpg/dreamrugi-worldbuild-tool/wiki/FAQ)

## 제공하는 기능

- Template 기반 문서 작성과 프로젝트별 관리
- 문서 관계, 검색, 글로서리, 이미지와 리소스 관리
- 개인 프로젝트의 로컬 편집·저장과 입력 복구
- SVN 작업 사본의 읽기, 잠금 기반 편집, 선택 커밋, 수동 상태 확인과 갱신
- 실행 기록과 복구 센터에서 결과 및 보관된 입력 확인

Windows 10 19041 이상 또는 Windows 11 x64와 Microsoft Edge WebView2 Runtime이 필요합니다. WebView2가 없다면 설치 과정에 인터넷 연결이 필요합니다. 오프라인 PC에서는 Microsoft의 WebView2 Evergreen Standalone Installer를 먼저 준비하십시오. SVN 협업에는 [TortoiseSVN](https://tortoisesvn.net/)의 명령줄 도구, HTTPS SVN 서버 계정과 올바른 작업 사본이 별도로 필요합니다.

프로젝트는 사용자가 선택한 폴더에 저장합니다. 로컬 저장은 SVN 서버 커밋과 다른 작업입니다. 저장된 미커밋 변경이 있으면 편집을 종료해도 잠금이 유지될 수 있습니다. 강제 잠금 획득 전에는 상대방과 작업 상태를 확인하십시오. 중요한 프로젝트와 복구 입력은 별도로 백업하십시오.

현재 앱 자체의 SVN Revert/rollback UI와 자동 업데이트는 제공하지 않습니다. Microsoft Store에는 아직 게시하지 않았습니다. 0.x 시험판의 Windows Authenticode는 미서명이며 updater 서명은 별도의 파일 검증 수단입니다.

## 소스 빌드와 배포 검증

[배포 안내](docs/release-guide.md)와 [Windows 패키징 안내](docs/m7-windows-packaging.md)를 참고하십시오. 잠긴 Node.js·Rust 및 npm/Cargo 의존성으로 빌드합니다. 수동 Actions는 검토한 공개 main commit에서 설치 파일·updater 서명·대응 소스를 만들고 **draft/prerelease**로 보관합니다. 일반 사용자에게 게시하는 단계는 별도의 사람 확인을 거칩니다.

## 라이선스

애플리케이션 자체 코드는 [GNU GPL version 3 only](LICENSE), `GPL-3.0-only`로 제공합니다. 이 라이선스가 사용자가 작성한 프로젝트 문서에 자동 적용되는 것은 아닙니다. Hunspell, 한글 사전, Lexical, Fluent UI, 폰트와 테마의 원래 라이선스·고지는 각각 유지합니다. 동봉된 고지와 `src-tauri/about-notices/`, `src-tauri/spellcheck/`, `src/assets/fonts/LICENSE`를 확인하십시오.

배포 시 `corresponding-source.zip`에는 정확한 제품 소스·lockfile·빌드 자료·고지를, `third-party-source.zip`에는 잠긴 실행 의존 소스를 제공합니다. 개인 키, 인증정보와 사용자 프로젝트는 공개 소스에 포함하지 않습니다.
