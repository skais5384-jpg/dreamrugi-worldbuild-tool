//! 기존 M1 checkpoint를 실제 실행 thread에서 관측한다. 저장/복구 결과나 권한은 만들지 않는다.
use super::{CommitTestPoint, RecoveryTestPoint};
use crate::data::transaction::PrepareFailPoint;
use std::{cell::RefCell, io, marker::PhantomData, rc::Rc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Point<'a> {
    Prepare(PrepareFailPoint),
    Commit(CommitTestPoint),
    Recovery(RecoveryTestPoint),
    Owned(&'a str),
}
pub(crate) type Observer = Box<dyn Fn(Point<'_>, Option<u32>) -> io::Result<()> + Send>;
thread_local! { static OBSERVER: RefCell<Option<Observer>> = const { RefCell::new(None) }; }
pub(crate) struct Guard(Option<Observer>, PhantomData<Rc<()>>);
pub(crate) fn install(observer: Observer) -> Guard {
    Guard(
        OBSERVER.with(|slot| slot.replace(Some(observer))),
        PhantomData,
    )
}

#[test]
fn g14_boundary_observer_restores_nested_and_unwind_scopes() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    let outer = install(Box::new(move |_, _| {
        observed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    check(Point::Owned("outer"), Some(0)).unwrap();
    let failure = std::panic::catch_unwind(|| {
        let _inner = install(Box::new(|_, _| {
            Err(io::Error::other("controlled observer cause"))
        }));
        assert!(check(Point::Owned("inner"), Some(1)).is_err());
        panic!("controlled G14 observer unwind");
    });
    assert!(failure.is_err());
    check(Point::Owned("restored"), Some(2)).unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 2);
    drop(outer);
    check(Point::Owned("removed"), Some(3)).unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 2);
}
impl Drop for Guard {
    fn drop(&mut self) {
        OBSERVER.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}
pub(crate) fn check(point: Point<'_>, operation: Option<u32>) -> io::Result<()> {
    OBSERVER.with(|slot| match slot.borrow().as_ref() {
        Some(observer) => observer(point, operation),
        None => Ok(()),
    })
}
