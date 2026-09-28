#[expect(
    dead_code,
    reason = "G6 Template backend는 연결됐으며 G13 Tauri 호출 경계는 후속 구현한다"
)]
pub(crate) mod application;
#[expect(
    dead_code,
    unused_imports,
    reason = "M2-2 artifact 모델과 bytes codec은 M2-4~M2-6 application 흐름에서 연결한다"
)]
pub(crate) mod artifact;
pub(crate) mod atomic_file;
#[expect(
    dead_code,
    reason = "M1-5F2 협업 잠금 기반은 F3 transaction 결합 전에 실제 흐름에 연결하지 않는다"
)]
pub(crate) mod collaboration_lock;
#[expect(
    dead_code,
    reason = "M1-6 호환성 판정은 후속 프로젝트 열기 계층에서 연결한다"
)]
pub(crate) mod compatibility;
pub(crate) mod edit_input;
pub(crate) mod edit_recovery;
#[expect(
    dead_code,
    reason = "M2-1 동기 편집 세션은 M2-6 Tauri application boundary에서 production 흐름에 연결한다"
)]
pub(crate) mod edit_session;
#[expect(
    dead_code,
    reason = "M2-3 Field Engine의 편집 normalization과 bound 검증 API는 M2-4~M2-5에서 연결한다"
)]
pub(crate) mod field_engine;
pub mod json;
#[expect(
    dead_code,
    reason = "M1-7 migration engine은 M1-8 파일 적용 계층에서 연결한다"
)]
pub(crate) mod migration;
#[expect(
    dead_code,
    reason = "M1-8A migration preflight는 M1-8B filesystem 적용 계층에서 연결한다"
)]
pub(crate) mod migration_batch;
#[expect(
    dead_code,
    reason = "M1-8B prepare API는 M1-8C commit 연결 전까지 production 흐름에 연결하지 않는다"
)]
pub(crate) mod migration_prepare;
#[expect(
    dead_code,
    reason = "M1-8C recovery API는 후속 프로젝트 열기 계층에서 연결한다"
)]
pub(crate) mod migration_recovery;
pub(crate) mod project_backup;
pub(crate) mod project_file;
#[expect(
    dead_code,
    reason = "M1-4 잠금 API는 후속 프로젝트 열기 계층에서 연결한다"
)]
pub(crate) mod project_lock;
pub(crate) mod project_relative_path;
#[expect(
    dead_code,
    reason = "G2 runtime은 G3 repository와 후속 application에서 연결한다"
)]
pub(crate) mod project_runtime;
pub(crate) mod project_settings;
#[expect(
    dead_code,
    reason = "G3 repository는 G4 이후 저장 및 application에서 연결한다"
)]
pub(crate) mod repository;
pub mod schema;
pub(crate) mod storage_estimate;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "M1-5A 모델은 후속 prepare/apply/recovery 단계에서 연결한다"
    )
)]
pub(crate) mod transaction;
pub(crate) mod utc_time;

pub(crate) mod asset_maintenance;
pub(crate) mod assets;
pub(crate) mod media;
