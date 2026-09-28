//! 이벤트는 권한이 아니라 예약이다. 상태 변경과 mailbox 삽입은 worker의 같은 짧은 lock 안에서 한다.
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TriggerReason {
    Periodic,
    Foreground,
    OsResume,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Reasons(u8);
impl Reasons {
    fn insert(&mut self, reason: TriggerReason) {
        self.0 |= match reason {
            TriggerReason::Periodic => 1,
            TriggerReason::Foreground => 2,
            TriggerReason::OsResume => 4,
        };
    }
    pub(crate) fn contains(self, reason: TriggerReason) -> bool {
        let mut bit = Self::default();
        bit.insert(reason);
        self.0 & bit.0 != 0
    }
    fn is_empty(self) -> bool {
        self.0 == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TriggerAccepted {
    Queued,
    Coalesced,
    FollowUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Idle,
    Queued,
    Running,
}

pub(super) struct Reservation {
    pub(super) phase: Phase,
    reasons: Reasons,
    follow_up: Reasons,
}
impl Default for Reservation {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            reasons: Reasons::default(),
            follow_up: Reasons::default(),
        }
    }
}
impl Reservation {
    /// 등록마다 예약 공간을 먼저 확보하므로 Queued로 바꾼 뒤 queue full이 되는 틈이 없다.
    pub(super) fn trigger(&mut self, reason: TriggerReason) -> TriggerAccepted {
        match self.phase {
            Phase::Idle => {
                self.phase = Phase::Queued;
                self.reasons.insert(reason);
                TriggerAccepted::Queued
            }
            Phase::Queued => {
                self.reasons.insert(reason);
                TriggerAccepted::Coalesced
            }
            Phase::Running => {
                self.follow_up.insert(reason);
                TriggerAccepted::FollowUp
            }
        }
    }
    pub(super) fn start(&mut self) -> Reasons {
        self.phase = Phase::Running;
        std::mem::take(&mut self.reasons)
    }
    /// 실행 시작 뒤 온 이벤트는 현재 성공에 흡수하지 않고 반드시 큐 뒤의 별도 예약으로 남긴다.
    pub(super) fn finish(&mut self) -> bool {
        if self.follow_up.is_empty() {
            self.phase = Phase::Idle;
            false
        } else {
            self.reasons = std::mem::take(&mut self.follow_up);
            self.phase = Phase::Queued;
            true
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IntervalError {
    Zero,
    DeadlineOverflow,
}

pub(super) struct Periodic {
    interval: Duration,
    deadline: Instant,
}
impl Periodic {
    pub(super) fn new(now: Instant, interval: Duration) -> Result<Self, IntervalError> {
        if interval.is_zero() {
            return Err(IntervalError::Zero);
        }
        let deadline = now
            .checked_add(interval)
            .ok_or(IntervalError::DeadlineOverflow)?;
        Ok(Self { interval, deadline })
    }
    pub(super) fn wait(&self, now: Instant) -> Duration {
        self.deadline.saturating_duration_since(now)
    }
    pub(super) fn due(&mut self, now: Instant) -> Result<bool, IntervalError> {
        if now < self.deadline {
            return Ok(false);
        }
        // suspend/busy 동안 지난 tick 수를 재생하지 않는다. 다음 실제 관측 시각부터 한 주기를 둔다.
        self.deadline = now
            .checked_add(self.interval)
            .ok_or(IntervalError::DeadlineOverflow)?;
        Ok(true)
    }
}
