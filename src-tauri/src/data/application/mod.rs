//! G5는 이미 Editing인 세션 안에서 읽기·계산·단일 저장을 완결한다.
//! G6~G10 업무와 편집 원본 보관을 연결하며 UI·worker는 후속 gate의 책임이다.
pub(crate) mod bulk_replace;
pub(crate) mod composite;
pub(crate) mod diagnostics;
pub(crate) mod documents;
pub(crate) mod format;
pub(crate) mod layout;
pub(crate) mod recovery_handoff;
pub(crate) mod scheduler;
pub(crate) mod shutdown;
pub(crate) mod templates;
pub(crate) mod worker;
mod write;

#[cfg(all(test, windows))]
mod tests;
