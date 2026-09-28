//! 실제 경계의 실패/교체만 주입한다. production 생성자나 성공 provider는 열지 않는다.
use super::RepositoryStage;
use std::{
    cell::RefCell,
    io,
    path::Path,
    sync::{Mutex, OnceLock},
};

type Action = Box<dyn FnMut(&Path) -> io::Result<()>>;
struct Hook {
    stage: RepositoryStage,
    remaining: usize,
    action: Action,
}
#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct Counts {
    pub(crate) decoded: usize,
    pub(crate) iterations: usize,
    pub(crate) hooks: usize,
    pub(crate) bytes_read: usize,
    pub(crate) document_scans: usize,
    pub(crate) template_scans: usize,
    pub(crate) reference_scans: usize,
    pub(crate) assessments: usize,
    pub(crate) read_calls: usize,
    pub(crate) hashes: usize,
}
#[derive(Default)]
struct State {
    hook: Option<Hook>,
    repeat: bool,
    counts: Counts,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
static GLOBAL_COUNTS: OnceLock<Mutex<Option<Counts>>> = OnceLock::new();

fn observe_global(change: impl FnOnce(&mut Counts)) {
    if let Some(counts) = GLOBAL_COUNTS
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("measurement count lock")
        .as_mut()
    {
        change(counts);
    }
}

pub(super) fn point(stage: RepositoryStage, path: &Path) -> io::Result<()> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if stage == RepositoryStage::Iterate {
            state.counts.iterations += 1;
            observe_global(|counts| counts.iterations += 1);
        }
        if let Some(hook) = &mut state.hook {
            if hook.stage == stage {
                if hook.remaining > 0 {
                    hook.remaining -= 1;
                } else {
                    let mut hook = state.hook.take().expect("hook was present");
                    state.counts.hooks += 1;
                    return (hook.action)(path);
                }
            }
        }
        Ok(())
    })
}
pub(super) fn decoded() {
    STATE.with(|state| state.borrow_mut().counts.decoded += 1);
    observe_global(|counts| counts.decoded += 1);
}
// 관측은 실행 thread의 scoped 상태에만 쌓는다. 제품 I/O나 권한에는 영향을 주지 않는다.
pub(super) fn read_bytes(bytes: usize) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.counts.bytes_read += bytes;
        state.counts.read_calls += 1;
    });
    observe_global(|counts| {
        counts.bytes_read += bytes;
        counts.read_calls += 1;
    });
}
pub(super) fn hashed() {
    STATE.with(|state| state.borrow_mut().counts.hashes += 1);
    observe_global(|counts| counts.hashes += 1);
}
pub(super) fn scan(operation: super::RepositoryOperation) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        match operation {
            super::RepositoryOperation::ScanDocuments => state.counts.document_scans += 1,
            super::RepositoryOperation::ScanTemplates => state.counts.template_scans += 1,
            _ => unreachable!("only namespace scans are observed"),
        }
    });
    observe_global(|counts| match operation {
        super::RepositoryOperation::ScanDocuments => counts.document_scans += 1,
        super::RepositoryOperation::ScanTemplates => counts.template_scans += 1,
        _ => unreachable!("only namespace scans are observed"),
    });
}

// 별도 worker thread에서 일어나는 규모 시험 I/O를 파일별 경로 없이 합계만 수집한다.
pub(crate) fn begin_global() {
    *GLOBAL_COUNTS
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("measurement count lock") = Some(Counts::default());
}

pub(crate) fn take_global() -> Counts {
    GLOBAL_COUNTS
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("measurement count lock")
        .take()
        .expect("global measurement was started")
}
pub(super) fn references(assessment: bool) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if assessment {
            state.counts.assessments += 1;
        } else {
            state.counts.reference_scans += 1;
        }
    });
}
pub(super) fn repeat_entry() -> bool {
    STATE.with(|state| std::mem::take(&mut state.borrow_mut().repeat))
}

pub(crate) fn scoped<T>(
    hook: Option<(RepositoryStage, usize, Action)>,
    repeat: bool,
    run: impl FnOnce() -> T,
) -> (T, Counts) {
    struct Restore(Option<State>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(previous) = self.0.take() {
                STATE.with(|state| *state.borrow_mut() = previous);
            }
        }
    }
    let _restore = Restore(Some(STATE.with(|state| {
        state.replace(State {
            hook: hook.map(|(stage, remaining, action)| Hook {
                stage,
                remaining,
                action,
            }),
            repeat,
            counts: Counts::default(),
        })
    })));
    let result = run();
    let counts = STATE.with(|state| state.borrow().counts);
    (result, counts)
}
