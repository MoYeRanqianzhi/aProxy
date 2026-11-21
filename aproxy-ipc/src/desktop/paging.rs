//! Run-local sequence ordering survives a backwards wall clock. Across runs only the
//! available heads compete by time, then by ascending port and binary instance ID.
use super::{Row, Run, Token};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};

/// A run that could not supply its part of the page; no successor is substituted.
#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Missing {
    /// Scope that was part of the frozen traversal.
    pub run: Run,
    /// Localized reason; no transport message or PID.
    pub reason: MissingReason,
}
/// Incomplete reads distinguish a replaced run from an offline cache.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MissingReason {
    /// Identity no longer matches.
    Changed,
    /// Peer refused or could not supply a verified page.
    Unavailable,
    /// Only the safe cached rows are still available.
    Disconnected,
}
/// A bounded response and its single-use continuation, never an IPC cursor string.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    /// Newest rows by the merge order.
    pub entries: Vec<Row>,
    /// Window-owned continuation, absent when all sources are exhausted.
    pub next: Option<Token>,
    /// Observed sources which could not be read completely.
    pub missing: Vec<Missing>,
}
struct Source {
    rows: VecDeque<Row>,
    archive: bool,
    before: Option<u64>,
}
/// Rust-only traversal owned by one window and scope. Remote IO happens outside
/// this type and only on a caller's continuation, never to fill the first page.
#[derive(Default)]
pub struct Traversal {
    sources: BTreeMap<Run, Source>,
    missing: Vec<Missing>,
}
impl Traversal {
    /// Seed from one frozen recent window. Active rows are preferentially retained;
    /// do not skip a hole by paging below an old active outlier. A continuation fills
    /// the gap from the last contiguous head on a live run.
    pub fn seed(&mut self, run: Run, mut rows: Vec<Row>, archive: bool) {
        rows.sort_by_key(|r| std::cmp::Reverse((r.seq, r.ended_at.is_some(), r.revision)));
        rows.dedup_by_key(|r| r.seq);
        if archive
            && let Some(i) = rows
                .windows(2)
                .position(|pair| pair[0].seq.get().checked_sub(1) != Some(pair[1].seq.get()))
        {
            rows.truncate(i + 1);
        }
        let before = rows.last().map(|r| r.seq.get());
        self.sources.insert(
            run,
            Source {
                rows: rows.into(),
                archive,
                before,
            },
        );
    }
    /// Record a genuinely missing scope once. Any cached head can still be consumed.
    pub fn missing(&mut self, run: Run, reason: MissingReason) {
        if let Some(s) = self.sources.get_mut(&run) {
            s.archive = false;
        }
        if !self.missing.iter().any(|m| m.run == run) {
            self.missing.push(Missing { run, reason });
        }
    }
    /// Which archive head must be fetched to make the *next* merge decision. None
    /// as an exclusive bound means this live run never supplied an in-memory head.
    pub fn needs_head(&self) -> Option<(Run, Option<u64>)> {
        self.sources
            .iter()
            .find(|(_, s)| s.rows.is_empty() && s.archive)
            .map(|(r, s)| (r.clone(), s.before))
    }
    /// Supply the identity-checked descending page returned for `needs_head`.
    pub fn supply(&mut self, run: &Run, rows: Vec<Row>, more: bool) {
        if let Some(s) = self.sources.get_mut(run) {
            s.before = rows.last().map(|r| r.seq.get());
            s.archive = more;
            s.rows = rows.into();
        }
    }
    /// Take one available head without IO. Do this directly for memory-only reads;
    /// on continuations, fill `needs_head` first so older runs cannot jump the queue.
    pub fn pop(&mut self) -> Option<Row> {
        let run = self
            .sources
            .iter()
            .filter_map(|(r, s)| Some((r, s.rows.front()?.started_at)))
            .max_by(|(a, at), (b, bt)| at.cmp(bt).then_with(|| b.cmp(a)))?
            .0
            .clone();
        self.sources.get_mut(&run)?.rows.pop_front()
    }
    /// Whether any buffered rows or an unexamined archive tail remain.
    pub fn more(&self) -> bool {
        self.sources
            .values()
            .any(|s| s.archive || !s.rows.is_empty())
    }
    /// Format a page without transferring private traversal state or peer errors.
    pub fn page(&self, entries: Vec<Row>, next: Option<Token>) -> LogPage {
        LogPage {
            entries,
            next,
            missing: self.missing.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{desktop::Projection, redact::Secrets, view};
    use serde_json::json;
    fn row(port: u16, seq: u64, at: u64) -> Row {
        let id = format!("{port:016x}");
        let raw = json!({"request_id":format!("{id}-{seq}"),"request_seq":seq,"started_at":at,"method":"GET","path":"/","stream":false,"delivery":"retry","upgraded":false,"outcome":"in_flight","phase":"attempting","attempts":1,"tries_omitted":0,"tries":[],"keepalive":false,"heartbeats":0,"request_bytes":0,"response_bytes":0});
        Projection::new(&Secrets::default())
            .row(
                &view::RunKey {
                    port,
                    instance_id: id,
                },
                serde_json::from_value(raw).unwrap(),
            )
            .unwrap()
    }
    #[test]
    fn ties_clock_reversal_and_sparse_recent_windows_keep_per_run_order() {
        let mut t = Traversal::default();
        let a = row(80, 4, 100);
        let b = row(81, 9, 100);
        let ar = a.r#ref.run.clone();
        let br = b.r#ref.run.clone();
        t.seed(ar.clone(), vec![a, row(80, 3, 200), row(80, 1, 500)], true);
        t.seed(br.clone(), vec![b], false);
        assert_eq!(t.pop().unwrap().r#ref.run, ar);
        assert_eq!(t.pop().unwrap().seq.get(), 3);
        assert_eq!(t.needs_head(), Some((ar.clone(), Some(3))));
        t.supply(&ar, vec![row(80, 2, 20), row(80, 1, 500)], false);
        assert_eq!(t.pop().unwrap().r#ref.run, br);
        assert_eq!(t.pop().unwrap().seq.get(), 2);
        assert_eq!(t.pop().unwrap().seq.get(), 1);
        assert!(!t.more());
    }
}
