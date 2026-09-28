//! 프로젝트 없이 실행하는 복구 작업도 결과를 먼저 인계한 뒤 UI에 알린다.
use super::*;
use std::sync::mpsc;

pub(super) struct LocalTask {
    result: mpsc::Receiver<Completed>,
    thread: std::thread::JoinHandle<Option<Completed>>,
}
impl LocalTask {
    pub(super) fn start(
        run: impl FnOnce() -> Completed + Send + 'static,
        wake: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> std::io::Result<Self> {
        let (sender, result) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("recovery-center".into())
            .spawn(move || {
                let completed = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
                    Ok(result) => result,
                    Err(panic) => failed(panic),
                };
                // is_finished()는 wake 직후에도 false일 수 있다. 채널에 소유권을 넘긴 시점이
                // 완료 기준이며 이후에는 알림만 실행한다. 수신자가 사라져도 원 결과를 남긴다.
                if let Err(error) = sender.send(completed) {
                    return Some(error.0);
                }
                if let Some(wake) = wake {
                    wake();
                }
                None
            })?;
        Ok(Self { result, thread })
    }
    pub(super) fn completed(&self) -> Option<Completed> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) if self.thread.is_finished() => {
                Some(failed("local result channel disconnected"))
            }
            Err(mpsc::TryRecvError::Disconnected) => None,
        }
    }
    pub(super) fn retain(self, mut result: Completed) -> Completed {
        // 작업 본문은 이미 끝났다. 알림 꼬리를 UI mutex 안에서 join하지 않고 결과 owner가
        // handle을 함께 보관한다. transport ack 전 원 결과의 수명은 줄어들지 않는다.
        result.original = Box::new((result.original, self.thread));
        result
    }
}
fn failed(cause: impl Send + 'static) -> Completed {
    Completed::new(
        cause,
        Ok(ResultDto::Rejected {
            error: Code::Unavailable.into(),
            input_retained: true,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn result_is_observable_while_wake_callback_has_not_returned() {
        let (entered_tx, entered) = mpsc::channel();
        let (resume, resume_rx) = mpsc::channel();
        let resume_rx = Mutex::new(resume_rx);
        let task = LocalTask::start(
            || failed("retained original"),
            Some(Arc::new(move || {
                entered_tx.send(()).unwrap();
                resume_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(20))
                    .unwrap();
            })),
        )
        .unwrap();
        entered.recv_timeout(Duration::from_secs(20)).unwrap();
        assert!(!task.thread.is_finished());
        let result = task.completed();
        resume.send(()).unwrap();
        assert!(
            result.is_some(),
            "wake must never precede result publication"
        );
        assert_eq!(
            result.unwrap().original.downcast_ref::<&str>(),
            Some(&"retained original")
        );
        assert!(task.thread.join().unwrap().is_none());
    }
    #[test]
    fn panic_cause_is_published_and_wakes_the_owner() {
        let (sender, receiver) = mpsc::channel();
        let task = LocalTask::start(
            || panic!("test private cause"),
            Some(Arc::new(move || {
                sender.send(()).unwrap();
            })),
        )
        .unwrap();
        receiver.recv_timeout(Duration::from_secs(20)).unwrap();
        let result = task.completed().unwrap();
        assert!(matches!(
            result.dto,
            Ok(ResultDto::Rejected {
                input_retained: true,
                ..
            })
        ));
        assert!(task.thread.join().unwrap().is_none());
    }
}
