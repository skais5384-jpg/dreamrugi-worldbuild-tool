//! 큐 밖의 원자적 제어 신호. OS read가 끝난 파일 경계에서 취소를 관측한다.
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        Arc,
    },
};
#[derive(Default)]
pub(crate) struct Progress {
    cancel: AtomicBool,
    files: AtomicU64,
    phase: AtomicU8,
    #[cfg(test)]
    gate: std::sync::Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
}
impl Progress {
    #[cfg(test)]
    pub(crate) fn hold_checkpoint(
        &self,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (entered, observe) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        *self.gate.lock().unwrap() = Some((entered, resume));
        (observe, release)
    }
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub(crate) fn snapshot(&self) -> (bool, u64, u8) {
        (
            self.cancel.load(Ordering::Acquire),
            self.files.load(Ordering::Acquire),
            self.phase.load(Ordering::Acquire),
        )
    }
}
thread_local! {static CURRENT:RefCell<Option<Arc<Progress>>>=const {RefCell::new(None)};}
pub(crate) struct Scope;
pub(crate) fn scope(p: Arc<Progress>) -> Scope {
    p.phase.store(1, Ordering::Release);
    CURRENT.with(|c| *c.borrow_mut() = Some(p));
    Scope
}
impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT.with(|c| {
            if let Some(p) = c.borrow_mut().take() {
                if p.phase.load(Ordering::Acquire) != 5 {
                    p.phase.store(4, Ordering::Release);
                }
            }
        });
    }
}
pub(crate) fn checkpoint(file: bool) -> Result<(), super::RepositoryError> {
    CURRENT.with(|c| {
        if let Some(p) = &*c.borrow() {
            #[cfg(test)]
            if let Some((entered, resume)) = p.gate.lock().unwrap().take() {
                entered.send(()).unwrap();
                resume
                    .recv_timeout(std::time::Duration::from_secs(20))
                    .unwrap();
            }
            if p.cancel.load(Ordering::Acquire) && p.phase.load(Ordering::Acquire) < 3 {
                p.phase.store(5, Ordering::Release);
                return Err(super::RepositoryError::new(
                    super::RepositoryCategory::Cancelled,
                    super::RepositoryOperation::ScanDocuments,
                    super::RepositoryStage::Complete,
                    Some(crate::data::artifact::ArtifactType::Document),
                ));
            }
            if file {
                p.files.fetch_add(1, Ordering::AcqRel);
                if p.phase.load(Ordering::Acquire) < 3 {
                    p.phase.store(2, Ordering::Release);
                }
            }
        }
        Ok(())
    })
}
pub(crate) fn preparing() -> Result<(), super::RepositoryError> {
    checkpoint(false)?;
    CURRENT.with(|c| {
        if let Some(p) = &*c.borrow() {
            p.phase.store(3, Ordering::Release);
        }
    });
    Ok(())
}
