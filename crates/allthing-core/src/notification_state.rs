// SPDX-License-Identifier: AGPL-3.0-or-later
//! Monotonic notification read state. Local progress works offline; only an
//! acknowledged write or the caller's private subscription advances `remote`.

/// A corrupt cache or provider clock cannot mark arbitrary future events read.
pub fn valid_seen(at: i64, now: i64) -> i64 {
    if at < 0 || at > now.saturating_add(60) { 0 } else { at.min(now.max(0)) }
}

#[derive(Clone, Debug, Default)]
pub struct ReadState {
    seen: i64,
    remote: i64,
    baseline: Option<i64>,
}

impl ReadState {
    pub fn new(local: i64, now: i64) -> Self {
        Self { seen: valid_seen(local, now), ..Self::default() }
    }

    pub fn seen(&self) -> i64 {
        self.seen
    }

    pub fn baseline(&self) -> Option<i64> {
        self.baseline
    }

    pub fn observe_local(&mut self, at: i64, now: i64) {
        self.seen = self.seen.max(valid_seen(at, now));
    }

    pub fn observe_remote(&mut self, at: i64, now: i64) {
        self.remote = self.remote.max(valid_seen(at, now));
        self.seen = self.seen.max(self.remote);
    }

    /// Capture the prior server state before uploading this visit's pending
    /// reads. It lets a cold notification page correct its initial row tint.
    pub fn hydrated(&mut self) {
        self.baseline.get_or_insert(self.remote);
    }

    pub fn pending(&self) -> Option<i64> {
        (self.baseline.is_some() && self.seen > self.remote).then_some(self.seen)
    }

    pub fn acknowledged(&mut self, at: i64) {
        self.remote = self.remote.max(at.min(self.seen));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_offline_progress_without_overwriting_a_newer_session() {
        let mut state = ReadState::new(100, 1000);
        assert_eq!(state.pending(), None);
        state.observe_local(300, 1000);
        state.observe_remote(200, 1000);
        state.hydrated();
        assert_eq!(state.baseline(), Some(200));
        assert_eq!(state.pending(), Some(300));
        state.observe_remote(400, 1000);
        state.acknowledged(300);
        state.observe_remote(50, 1000);
        assert_eq!(state.seen(), 400);
        assert_eq!(state.pending(), None);
        assert_eq!(state.baseline(), Some(200));
    }

    #[test]
    fn failed_writes_keep_progress_and_an_old_ack_cannot_clear_a_new_read() {
        let mut state = ReadState::new(100, 1000);
        state.hydrated();
        let sent = state.pending().unwrap();
        assert_eq!(state.pending(), Some(sent)); // no acknowledgement on failure
        state.observe_local(500, 1000);
        state.acknowledged(sent);
        assert_eq!(state.pending(), Some(500));
        state.acknowledged(500);
        assert_eq!(state.pending(), None);
    }

    #[test]
    fn delayed_initial_snapshot_does_not_include_this_visits_pending_mark() {
        let mut state = ReadState::new(0, 1000);
        state.observe_local(900, 1000);
        state.observe_remote(700, 1000);
        state.hydrated();
        assert_eq!(state.baseline(), Some(700));
        assert_eq!(state.pending(), Some(900));
        state.observe_remote(950, 1000);
        state.hydrated();
        assert_eq!(state.baseline(), Some(700));
    }

    #[test]
    fn untrusted_timestamps_cannot_poison_a_valid_marker() {
        for at in [-1, i64::MIN, i64::MAX, 1061] {
            assert_eq!(valid_seen(at, 1000), 0);
        }
        assert_eq!(valid_seen(1060, 1000), 1000);
        let mut state = ReadState::new(i64::MAX, 1000);
        state.observe_remote(900, 1000);
        state.observe_local(i64::MAX, 1000);
        assert_eq!(state.seen(), 900);
    }
}
