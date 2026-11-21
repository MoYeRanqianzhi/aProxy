//! Bounded producer state for one page attachment. The window drives this with a
//! monotonic clock and delivers the resulting frames; this module owns no task or IO.
use super::{Job, Ref, Run, SafeUint};
use serde::Serialize;
use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

const STALL: Duration = Duration::from_secs(10);
const RATE: Duration = Duration::from_millis(250);
const LIMIT: usize = 128;

/// A replaceable invalidation, projected job or ephemeral live completion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum DesktopEvent {
    /// Re-read every visible domain; no queued delta is needed afterward.
    Resync,
    /// Replace cards and watchdog from memory.
    InstancesChanged,
    /// Replace this run's current window; the port alone is insufficient.
    RequestsChanged { run: Run },
    /// Best-effort animation, never persisted or replayed from a page.
    Rescued { r#ref: Ref, at: SafeUint },
    /// Replace the job snapshot, including all per-port outcomes collected so far.
    UpdateProgress { job: Box<Job> },
    /// Re-read the preferences and bookmarks.
    DesktopStateChanged,
    /// Re-read global settings after a successful write.
    SettingsChanged,
}
impl DesktopEvent {
    fn supersedes(&self, old: &Self) -> bool {
        match (self, old) {
            (Self::RequestsChanged { run: a }, Self::RequestsChanged { run: b }) => a == b,
            (Self::Rescued { r#ref: a, .. }, Self::Rescued { r#ref: b, .. }) => a == b,
            _ => std::mem::discriminant(self) == std::mem::discriminant(old),
        }
    }
}
/// Exactly one event in one attachment generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    /// Compare against the result of attach before applying anything.
    pub session: String,
    /// Numeric acknowledgement key, independent of peer revisions.
    pub seq: SafeUint,
    /// Replacement or bounded invalidation.
    pub event: DesktopEvent,
}
/// One acknowledged stream with a single forced recovery frame per stalled receiver.
pub struct Frames {
    session: String,
    seq: SafeUint,
    outstanding: Option<(Instant, bool)>,
    pending: VecDeque<DesktopEvent>,
    instances_due: Option<Instant>,
    run_due: BTreeMap<Run, Instant>,
}
impl Frames {
    /// A fresh attachment starts with resync even if there has been no discovery yet.
    pub fn new(session: String) -> Self {
        Self {
            session,
            seq: SafeUint::default(),
            outstanding: None,
            pending: [DesktopEvent::Resync].into(),
            instances_due: None,
            run_due: BTreeMap::new(),
        }
    }
    /// Ignore stale, duplicate and cross-attachment acknowledgements.
    pub fn ack(&mut self, session: &str, seq: SafeUint) {
        if self.session == session && self.seq == seq {
            self.outstanding = None;
        }
    }
    /// Offer an event without sending or blocking. Collapse overflows and lag to a
    /// single resync and discard superseded celebrations rather than replaying them.
    pub fn offer(&mut self, event: DesktopEvent) {
        if matches!(event, DesktopEvent::Resync) {
            self.pending.clear();
            self.pending.push_back(event);
            return;
        }
        if matches!(self.pending.front(), Some(DesktopEvent::Resync)) {
            return;
        }
        if let Some(old) = self.pending.iter_mut().find(|old| event.supersedes(old)) {
            *old = event;
        } else if self.pending.len() >= LIMIT {
            self.offer(DesktopEvent::Resync);
        } else {
            self.pending.push_back(event);
        }
    }
    fn due(&self, event: &DesktopEvent, now: Instant) -> Instant {
        match event {
            DesktopEvent::InstancesChanged => self.instances_due.unwrap_or(now),
            DesktopEvent::RequestsChanged { run } => self.run_due.get(run).copied().unwrap_or(now),
            _ => now,
        }
    }
    /// Next instant at which polling can emit without another input. `None` means
    /// wait for an incoming event or ack; never spin on an unresponsive webview.
    pub fn deadline(&self, now: Instant) -> Option<Instant> {
        if let Some((sent, forced)) = self.outstanding {
            return (!forced).then_some(sent + STALL);
        }
        self.pending.iter().map(|e| self.due(e, now)).min()
    }
    /// Advance at the supplied monotonic time. Rate limits are independent by run;
    /// a throttled invalidation does not block a terminal update or another run.
    pub fn poll(&mut self, now: Instant) -> Option<Frame> {
        let forced = if let Some((sent, forced)) = self.outstanding {
            if forced || now < sent + STALL {
                return None;
            }
            self.offer(DesktopEvent::Resync);
            true
        } else {
            false
        };
        self.run_due.retain(|_, due| *due > now);
        let index = self.pending.iter().position(|e| self.due(e, now) <= now)?;
        let event = self.pending.remove(index)?;
        match &event {
            DesktopEvent::InstancesChanged => self.instances_due = Some(now + RATE),
            DesktopEvent::RequestsChanged { run } => {
                self.run_due.insert(run.clone(), now + RATE);
            }
            _ => {}
        }
        self.seq = SafeUint::saturating(self.seq.get() + 1);
        self.outstanding = Some((now, forced));
        Some(Frame {
            session: self.session.clone(),
            seq: self.seq,
            event,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn change(port: u16) -> DesktopEvent {
        DesktopEvent::RequestsChanged {
            run: Run {
                port,
                instance_id: format!("{port:016x}"),
            },
        }
    }
    #[test]
    fn stall_supersedes_old_ack_once_and_receives_subsequent_changes() {
        let now = Instant::now();
        let mut f = Frames::new("s".into());
        let first = f.poll(now).unwrap();
        f.offer(change(80));
        assert_eq!(f.deadline(now), Some(now + STALL));
        assert!(f.poll(now + STALL - Duration::from_millis(1)).is_none());
        let forced = f.poll(now + STALL).unwrap();
        assert_eq!(forced.event, DesktopEvent::Resync);
        f.ack("s", first.seq);
        f.ack("obsolete", forced.seq);
        f.offer(change(81));
        assert!(f.poll(now + STALL * 3).is_none());
        assert_eq!(f.deadline(now + STALL * 3), None);
        f.ack("s", forced.seq);
        assert_eq!(f.poll(now + STALL * 3).unwrap().event, change(81));
    }
    #[test]
    fn rate_limits_do_not_block_unrelated_runs_and_lag_forgets_celebrations() {
        let now = Instant::now();
        let mut f = Frames::new("s".into());
        let next = |f: &mut Frames, now| {
            let frame = f.poll(now).unwrap();
            f.ack("s", frame.seq);
            frame.event
        };
        assert_eq!(next(&mut f, now), DesktopEvent::Resync);
        f.offer(change(80));
        f.offer(DesktopEvent::InstancesChanged);
        assert_eq!(next(&mut f, now), change(80));
        assert_eq!(next(&mut f, now), DesktopEvent::InstancesChanged);
        for _ in 0..500 {
            f.offer(change(80));
            f.offer(DesktopEvent::InstancesChanged);
        }
        f.offer(change(81));
        assert_eq!(next(&mut f, now), change(81));
        assert!(f.poll(now).is_none());
        assert_eq!(f.deadline(now), Some(now + RATE));
        assert_eq!(next(&mut f, now + RATE), change(80));
        assert_eq!(next(&mut f, now + RATE), DesktopEvent::InstancesChanged);
        assert!(f.poll(now + RATE).is_none());
        for p in 1..=130 {
            f.offer(change(p));
        }
        assert_eq!(next(&mut f, now + RATE), DesktopEvent::Resync);
        assert!(f.poll(now + RATE).is_none());
    }
}
