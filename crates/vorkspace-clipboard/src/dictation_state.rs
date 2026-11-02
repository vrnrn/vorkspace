//! Admission and publication fence. Cancellation keeps the worker slot until joined.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Recording,
    Transcribing,
    Cancelling,
}
#[derive(Debug)]
pub struct Attempt {
    pub epoch: u64,
    pub phase: Phase,
    pub enabled: bool,
}
impl Default for Attempt {
    fn default() -> Self {
        Self {
            epoch: 0,
            phase: Phase::Idle,
            enabled: false,
        }
    }
}
impl Attempt {
    pub fn begin(&mut self) -> Option<u64> {
        if !self.enabled || self.phase != Phase::Idle {
            return None;
        }
        self.epoch += 1;
        self.phase = Phase::Recording;
        Some(self.epoch)
    }
    pub fn stop(&mut self) -> Option<u64> {
        if self.phase != Phase::Recording {
            return None;
        }
        self.phase = Phase::Transcribing;
        Some(self.epoch)
    }
    pub fn cancel(&mut self) {
        self.epoch += 1;
        self.phase = if matches!(self.phase, Phase::Transcribing | Phase::Cancelling) {
            Phase::Cancelling
        } else {
            Phase::Idle
        };
    }
    /// Consume permission to publish before the external effect. Never replay.
    pub fn complete(&mut self, epoch: u64) -> bool {
        if self.phase == Phase::Cancelling {
            self.phase = Phase::Idle;
            return false;
        }
        if !self.enabled || self.epoch != epoch || self.phase != Phase::Transcribing {
            return false;
        }
        self.phase = Phase::Idle;
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_joins_before_reuse_and_rejects_stale_output() {
        let mut s = Attempt {
            enabled: true,
            ..Default::default()
        };
        let old = s.begin().unwrap();
        s.stop();
        s.cancel();
        assert!(s.begin().is_none());
        assert!(!s.complete(old));
        let new = s.begin().unwrap();
        s.stop();
        assert!(!s.complete(old));
        assert!(s.complete(new));
        assert!(!s.complete(new));
    }
    #[test]
    fn disabling_and_reenabling_does_not_resurrect_work() {
        let mut s = Attempt {
            enabled: true,
            ..Default::default()
        };
        let e = s.begin().unwrap();
        s.stop();
        s.enabled = false;
        s.cancel();
        s.enabled = true;
        assert!(!s.complete(e));
        assert!(s.begin().is_some());
    }
    #[test]
    fn duplicate_stop_is_not_another_job() {
        let mut s = Attempt {
            enabled: true,
            ..Default::default()
        };
        s.begin();
        assert!(s.stop().is_some());
        assert!(s.stop().is_none());
    }
}

/// One model worker through cancellation/join. IDs fence UI publication independently
/// of the OS file lock, which serializes acquisition across helper processes.
#[derive(Default, Debug)]
pub struct DownloadSlot {
    active: Option<(u64, bool)>,
}
impl DownloadSlot {
    pub fn begin(&mut self, id: u64) -> bool {
        if self.active.is_some() {
            return false;
        }
        self.active = Some((id, false));
        true
    }
    pub fn cancel(&mut self, id: u64) -> bool {
        if let Some((current, cancelled)) = &mut self.active
            && *current == id
        {
            *cancelled = true;
            return true;
        }
        false
    }
    pub fn accepts(&self, id: u64) -> bool {
        self.active == Some((id, false))
    }
    pub fn id(&self) -> Option<u64> {
        self.active.map(|v| v.0)
    }
    pub fn finish(&mut self, id: u64) -> Option<bool> {
        let (current, cancelled) = self.active?;
        if current != id {
            return None;
        }
        self.active = None;
        Some(!cancelled)
    }
}
#[cfg(test)]
mod download_tests {
    use super::*;
    #[test]
    fn cancelled_job_keeps_admission_until_join_and_cannot_publish_into_successor() {
        let mut s = DownloadSlot::default();
        assert!(s.begin(1));
        assert!(s.cancel(1));
        assert!(!s.accepts(1));
        assert!(!s.begin(2));
        assert_eq!(s.finish(99), None);
        assert_eq!(s.finish(1), Some(false));
        assert!(s.begin(2));
        assert!(!s.cancel(1));
        assert!(!s.accepts(1));
        assert_eq!(s.finish(1), None);
        assert!(s.accepts(2));
        assert_eq!(s.finish(2), Some(true));
        assert!(!s.accepts(2));
    }
}
