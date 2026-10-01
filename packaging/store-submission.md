# Microsoft Store 1.0.0 제출 준비 — 로컬 초안

상품 소개: **Dreamrugi Worldbuild Tool — 세계관 개발 및 관리 도구**. 한국어로 Template 기반 문서와 관계·이미지를 정리하고 개인 프로젝트를 백업/새 폴더로 복원합니다. 별도 TortoiseSVN과 사용자 서버/계정이 있으면 잠금·선택 커밋·명시 수신을 지원합니다. 작성한 문서의 라이선스를 앱 라이선스로 바꾸지 않습니다.

실제 상품 식별값은 사용자가 제공한 Partner Center 화면과 답변으로 확인했습니다. 상품 등록과 패키지 제출·게시를 구별하며, 이 문서는 제출 완료를 뜻하지 않습니다.

| 항목                       | 확인값                                  |
| -------------------------- | --------------------------------------- |
| Package/Identity/Name      | Dreamrugi.DreamrugiWorldbuildTool       |
| Package/Identity/Publisher | CN=A712EF40-3715-4F9F-B8C6-0BCFD42E89E1 |
| PublisherDisplayName       | Dreamrugi                               |
| Store ID                   | 9N6FCVJ83ZLN                            |
| 같은 family 제출 이력      | 사용자 확인: 제출 이력 없음 — 새 상품   |
| 앱 / 최초 MSIX 버전        | 1.0.0 / 1.0.0.0                         |

외부 identity JSON은 source=PartnerCenter, verified=true, historyVerified=true, 위 실제 name/publisher/publisherDisplayName/storeId/checkedUtc, noPublishedPackages=true, latestPackageVersion=null, packageVersion=1.0.0.0을 담습니다. 빈 값·시험 identity·미확인 이력은 거절합니다. 이후 같은 family에 더 높은 제출 이력이 생기면 그보다 높은 명시 packageVersion을 정하며 ELocalTest의 이력은 별도입니다. 미게시 Store URL을 설치 링크로 쓰지 않습니다.

지원 범위: Windows.Desktop x64, MinVersion10.0.19041.0. MaxVersionTested10.0.26200.0만으로 해당 OS 실제 시험을 주장하지 않습니다. WebView2 Evergreen Runtime이 필요하며 MSIX에 런타임 설치기를 넣지 않습니다. 제출 시 Store 정책·OS 지원을 다시 대조합니다. GPL-3.0-only 및 고지·대응 소스를 제공합니다.

runFullTrust 사유: 사용자 폴더의 프로젝트 JSON/이미지·백업·복구 자료를 읽고 쓰는 데스크톱 앱이며 동봉 Hunspell과 설치된 SVN/TortoiseSVN을 실행합니다. WebView에는 제한된 IPC만 제공합니다. Microsoft 공식 [identity](https://learn.microsoft.com/en-us/windows/apps/publish/view-app-identity-details), [MSIX 요구](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msix/app-package-requirements)를 따릅니다. [Tauri Store 안내](https://v2.tauri.app/distribute/microsoft-store/)는 EXE/MSI 경로이며 MSIX 제출 조건으로 혼용하지 않습니다.

제출용 MSIX는 updater 키 없이 만들고 Store가 인증 후 재서명합니다. 같은 manifest/EXE/리소스 payload의 로컬 서명 사본은 별도 지문으로 관리하며 인증서 신뢰를 자동 변경하지 않습니다. 실제 identity의 MSIX, WACK·설치·Store 시작 안내·제거/재설치 자료 보존을 확인하기 전 제출 준비 완료로 표시하지 않습니다.

## 개인정보·네트워크 안내 초안

프로젝트는 사용자 선택 폴더, 설정·실행 기록·보관 입력은 로컬 사용자 데이터 위치에 저장합니다. 프로젝트 본문을 개발자 서버에 자동 업로드하는 기능은 확인되지 않았습니다. 모든 네트워크 요청이나 플랫폼 진단 수집이 없다는 뜻은 아닙니다.

GitHub 채널은 시작 시 공개 업데이트 metadata/Release를 조회하고 동의 후 설치물을 받으며 서버에는 통상 IP·요청 정보가 전달됩니다. Store 채널은 GitHub updater를 실행하지 않으며 Store/WebView2/Windows 정책이 적용됩니다. 외부 이미지/미디어·YouTube·링크를 열면 해당 서비스로 요청이 갈 수 있습니다. SVN은 사용자 서버에 계정/선택 파일을 전송하며 인증 저장은 SVN/TortoiseSVN/Windows 기존 경로를 사용합니다. 문의에 첨부하는 진단/원문은 사용자 확인이 필요합니다.

제출 전 실제 지원·개인정보 정책 URL, 운영자 정보, 연령 등급·분류·시장·가격·권한 설명을 확정합니다. 미확인 URL이나 ‘수집 없음’을 게시하지 않습니다. 합성 프로젝트의 실제 패키지 화면을 사용하며 계정·실제 경로·원문은 포함하지 않습니다. 스크린샷은 패키지/채널/창 크기/DPI receipt와 연결합니다.
