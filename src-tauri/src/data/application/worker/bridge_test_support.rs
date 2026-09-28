//! G13의 실제 초기화/결과 공개 순서만 고정한다. runtime/custody/종료 상태를 주입하지 않는다.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{mpsc, Mutex, OnceLock},
    time::Duration,
};
pub(crate) struct Gate {
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}
pub(crate) struct Owner {
    entered: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
}
pub(crate) fn gate() -> (Gate, Owner) {
    let (tx, entered) = mpsc::channel();
    let (release, rx) = mpsc::channel();
    (
        Gate {
            entered: tx,
            release: rx,
        },
        Owner { entered, release },
    )
}
impl Owner {
    pub(crate) fn entered(&self) {
        self.entered.recv_timeout(Duration::from_secs(20)).unwrap();
    }
    pub(crate) fn release(self) {
        self.release.send(()).unwrap();
    }
}
impl Gate {
    fn block(self) {
        self.entered.send(()).unwrap();
        self.release.recv_timeout(Duration::from_secs(20)).unwrap();
    }
}
static INITIALIZATIONS: OnceLock<Mutex<BTreeMap<PathBuf, Gate>>> = OnceLock::new();
use crate::data::transaction::test_support::boundary;
static OBSERVERS: OnceLock<Mutex<BTreeMap<PathBuf, boundary::Observer>>> = OnceLock::new();
pub(crate) fn observe_io(root: PathBuf, observer: boundary::Observer) {
    assert!(OBSERVERS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(root, observer)
        .is_none());
}
pub(super) fn install_io(root: &Path) -> Option<boundary::Guard> {
    OBSERVERS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(root)
        .map(boundary::install)
}
pub(crate) fn hold_init(root: PathBuf, gate: Gate) {
    assert!(INITIALIZATIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(root, gate)
        .is_none());
}
pub(super) fn before_init(root: &Path) {
    let gate = INITIALIZATIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(root);
    if let Some(gate) = gate {
        gate.block();
    }
}
thread_local! { static AFTER:RefCell<Option<(usize,Gate)>>=const { RefCell::new(None) }; }
pub(crate) fn hold_after_publish(skip: usize, gate: Gate) {
    AFTER.with(|s| assert!(s.replace(Some((skip, gate))).is_none()));
}
pub(super) fn after_publish() {
    let gate = AFTER.with(|s| {
        let mut s = s.borrow_mut();
        if let Some((skip, _)) = s.as_mut() {
            if *skip > 0 {
                *skip -= 1;
                return None;
            }
        }
        s.take().map(|(_, gate)| gate)
    });
    if let Some(gate) = gate {
        gate.block();
    }
}
