//! Project the discovery sample at the same verified run identity as its cached
//! state. Never serialize an instance record, behavior document or watchdog record.
use super::{Delivery, Fault, HistoryCode, Job, Projection, R, Row, Run, SafeUint, Token};
use crate::{enumerate, redact, view, wire};
use serde::Serialize;
use std::collections::BTreeMap;

/// Supported operations after applying source-redaction and liveness policy.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reads {
    /// History pages and a usable feed.
    pub history_feed: bool,
    /// Request metadata on demand.
    pub detail: bool,
    /// Bounded body reads on demand.
    pub body: bool,
}

/// Only fields of a verified membership snapshot relevant to an instance card.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardMeta {
    /// Redacted listening address for display.
    pub listen: String,
    /// Masked upstream URL.
    pub base_url: String,
    /// Redacted display path for a file-backed run.
    pub config_path: Option<String>,
    /// Registered aliases for the resolved file.
    pub aliases: Vec<String>,
    /// Actual runtime delivery mode.
    pub delivery: Delivery,
    /// Runtime disk cache setting.
    pub disk_cache: bool,
    /// Runtime heartbeat interval in seconds.
    pub keepalive_interval_secs: SafeUint,
    /// Classified runtime trigger.
    pub keepalive_trigger: Word,
    /// Number of effective retry rules.
    pub retry_rule_count: u32,
}

// Only compile-time literals are ever assigned to classification fields.
type Word = &'static str;

/// Last classified failure with its optional numeric status, never raw error details.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureAt {
    /// Stable localization key.
    pub code: HistoryCode,
    /// Unix time in milliseconds.
    pub at: SafeUint,
    /// HTTP status if supplied as a number.
    pub status: Option<u16>,
}

/// Rolling activity counters in the source’s oldest-first order.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Buckets {
    /// Bucket width in seconds.
    pub width_secs: SafeUint,
    /// Receipts in each bucket.
    pub requests: Vec<SafeUint>,
    /// Retry attempts per bucket.
    pub retries: Vec<SafeUint>,
    /// Failed attempts, not rescued requests.
    pub failures: Vec<SafeUint>,
}

/// Counters describe just one run and reset when that process is replaced.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    /// Most recent receipt time.
    pub last_request_at: SafeUint,
    /// Received requests.
    pub requests_total: SafeUint,
    /// Attempts after first.
    pub retries_total: SafeUint,
    /// Successes after retry.
    pub rescued_total: SafeUint,
    /// Forward requests switched to retry.
    pub upgraded_total: SafeUint,
    /// Unfinished deliveries.
    pub in_flight: SafeUint,
    /// In-flight requests currently retrying.
    pub retrying: SafeUint,
    /// Failures by known localization key, combining all unknown names.
    pub by_code: BTreeMap<HistoryCode, SafeUint>,
    /// Suppressed for peers without source redaction.
    pub last_error: Option<FailureAt>,
    /// Measured counts only.
    pub buckets: Buckets,
}

/// One current live, unresponsive or restore-pending port. All action keys are preconditions.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    /// Listening port, including stopped ports with a pending desire.
    pub port: u16,
    /// Trustworthy recorded generation if known.
    pub run: Option<Run>,
    /// Recorded PID for display only.
    pub pid: Option<u32>,
    /// Scrubbed recorded version.
    pub version: Option<String>,
    /// Recorded start in Unix milliseconds.
    pub started_at: Option<SafeUint>,
    /// Recorded reason for this start, including unrecognized spellings; null without a trusted record.
    pub launch_reason: Option<String>,
    /// Display-scrubbed note from that same start, if one was supplied.
    pub launch_note: Option<String>,
    /// Discovery result, not history-feed health.
    pub liveness: view::Presence,
    /// Whether the port wants to run again.
    pub restore_pending: bool,
    /// Classified reason when known.
    pub restore_reason: Option<Word>,
    /// Observation-specific token, never the private spec.
    pub expected_restore: Option<Token>,
    /// Subscription health.
    pub feed: view::Feed,
    /// Policy restriction or unsupported feed.
    pub history_unavailable: Option<Word>,
    /// Effective capabilities.
    pub reads: Reads,
    /// Classified lifecycle only on a verified live run.
    pub state: Option<Word>,
    /// Current run counters if sampled.
    pub activity: Option<Activity>,
    /// Same-run allowlisted membership details.
    pub metadata: Option<CardMeta>,
}

/// Observed result of one watchdog restart.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Restart {
    /// Instance port.
    pub port: u16,
    /// Unix milliseconds at completion.
    pub at: SafeUint,
    /// Classified crash or hang recovery.
    pub kind: Word,
    /// True when the restart succeeded.
    pub ok: bool,
}

/// State of the verified watchdog owner, independent of the enabled setting.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Watchdog {
    /// Current owner.
    pub pid: u32,
    /// Running, stale or unknown.
    pub state: Word,
    /// Completion time of the last scan.
    pub last_scan_at: SafeUint,
    /// Most recent observed outcomes.
    pub restarts: Vec<Restart>,
}

/// Memory-only view of the window’s current home.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// Current membership by port.
    pub instances: Vec<Card>,
    /// At most fifty newest available summaries.
    pub recent: Vec<Row>,
    /// Sampled alongside discovery.
    pub watchdog: Option<Watchdog>,
    /// The window’s current or last observed job.
    pub update: Option<Job>,
}

/// Issued only for the verified run from this discovery sample. Kept together to
/// prevent accidentally joining metadata to a replacement listening on the same port.
#[derive(Clone, Debug, PartialEq)]
pub struct RunMeta(Run, CardMeta);
impl Projection<'_> {
    /// Enumeration splits a verified reply into its record and status. Keep its run
    /// alongside the projection so later samples cannot join it to a successor.
    pub fn metadata(&self, item: &enumerate::Enumerated, aliases: &[String]) -> Option<RunMeta> {
        let r = item.instance.as_ref()?;
        let status = item.status.as_ref()?;
        if item.liveness != enumerate::Liveness::Live
            || !status
                .features
                .iter()
                .any(|s| s == wire::FEATURE_REDACTED_CLIENT_FIELDS)
        {
            return None;
        }
        let run = self.run(&view::RunKey {
            port: item.port,
            instance_id: r.instance_id.clone(),
        })?;
        let b = &status.behavior;
        Some(RunMeta(
            run,
            CardMeta {
                listen: self.text(&r.listen_addr),
                base_url: self.text(&redact::url(&r.base_url)),
                config_path: r.config_path.as_deref().map(|s| self.text(s)),
                aliases: aliases.iter().map(|s| self.text(s)).collect(),
                delivery: Delivery::classify(&b.delivery),
                disk_cache: b.disk_cache,
                keepalive_interval_secs: SafeUint::saturating(b.keepalive.interval_secs),
                keepalive_trigger: match b.keepalive.trigger.as_str() {
                    "any" => "any",
                    "accept" => "accept",
                    "body_stream" => "body-stream",
                    _ => "unknown",
                },
                retry_rule_count: b.retry_rules,
            },
        ))
    }
    /// Join live state to already sampled metadata and a host-verified desire. A
    /// hidden identity disables every run action; never use its scrubbed string as a key.
    pub fn card(
        &self,
        r: view::InstanceSummary,
        metadata: Option<&RunMeta>,
        restore: Option<Token>,
    ) -> R<Card> {
        let live = r.liveness == view::Presence::Live;
        let run = r.run.as_ref().and_then(|k| self.run(k));
        let blocked = r.history_unavailable.is_some();
        let usable = live && run.is_some() && !blocked;
        let meta = metadata
            .filter(|m| usable && run.as_ref() == Some(&m.0))
            .map(|m| m.1.clone());
        let history_unavailable = if blocked {
            Some("source-redaction-missing")
        } else if live && !r.reads.history_feed {
            Some("unsupported")
        } else {
            None
        };
        let state = r.state.filter(|_| live);
        Ok(Card {
            port: r.port,
            run,
            pid: r.pid,
            version: r.version.as_deref().map(|s| self.text(s)),
            started_at: r.started_at.map(epoch).transpose()?,
            launch_reason: r.launch_reason,
            launch_note: r.launch_note.as_deref().map(|s| self.text(s)),
            liveness: r.liveness,
            restore_pending: r.restore_pending,
            restore_reason: r.restore_reason.as_deref().map(|s| {
                if s == enumerate::RESTORE_REASON_CONFIG_MISSING {
                    "config-missing"
                } else {
                    "unknown"
                }
            }),
            expected_restore: restore
                .filter(|_| r.restore_pending && r.liveness == view::Presence::Down),
            feed: r.feed,
            history_unavailable,
            reads: Reads {
                history_feed: usable && r.reads.history_feed,
                detail: usable && r.reads.detail,
                body: usable && r.reads.body,
            },
            state: state.as_ref().map(|s| match s.state.as_str() {
                "starting" => "starting",
                "serving" => "serving",
                "swap_prepared" => "swap-prepared",
                "stopping" => "stopping",
                _ => "unknown",
            }),
            activity: state
                .map(|s| Activity::project(s.activity, usable))
                .transpose()?,
            metadata: meta,
        })
    }
}
fn epoch(n: u64) -> R<SafeUint> {
    SafeUint::try_from(n).map_err(|_| Fault::DesktopUnavailable {})
}
impl Activity {
    fn project(r: view::RunActivity, safe: bool) -> R<Self> {
        let sat = SafeUint::saturating;
        let mut counts = BTreeMap::<_, SafeUint>::new();
        for (code, n) in r.by_code {
            let v = counts.entry(HistoryCode::classify(&code)).or_default();
            *v = sat(v.get().saturating_add(n));
        }
        let last_error = r
            .last_error
            .filter(|_| safe)
            .map(|e| -> R<FailureAt> {
                Ok(FailureAt {
                    code: HistoryCode::classify(&e.code),
                    at: epoch(e.at)?,
                    status: e.details["status"]
                        .as_u64()
                        .and_then(|n| u16::try_from(n).ok()),
                })
            })
            .transpose()?;
        let b = r.buckets;
        let counts_vec = |items: Vec<_>| items.into_iter().map(sat).collect();
        Ok(Self {
            last_request_at: epoch(r.last_request_at)?,
            requests_total: sat(r.requests_total),
            retries_total: sat(r.retries_total),
            rescued_total: sat(r.rescued_total),
            upgraded_total: sat(r.upgraded_total),
            in_flight: sat(r.in_flight),
            retrying: sat(r.retrying),
            by_code: counts,
            last_error,
            buckets: Buckets {
                width_secs: sat(b.width_secs),
                requests: counts_vec(b.requests),
                retries: counts_vec(b.retries),
                failures: counts_vec(b.failures),
            },
        })
    }
}
impl Watchdog {
    /// Drop record text and classify names; a stale heartbeat is not the enabled preference.
    pub fn project(w: enumerate::WatchdogView) -> R<Self> {
        Ok(Self {
            pid: w.pid,
            state: match w.state.as_str() {
                "running" => "running",
                "stale" => "stale",
                _ => "unknown",
            },
            last_scan_at: epoch(w.last_scan_at)?,
            restarts: w
                .restarts
                .into_iter()
                .map(|r| {
                    Ok(Restart {
                        port: r.port,
                        at: epoch(r.at)?,
                        kind: match r.kind.as_str() {
                            "crash" => "crash",
                            "hang" => "hang",
                            _ => "unknown",
                        },
                        ok: r.ok,
                    })
                })
                .collect::<R<_>>()?,
        })
    }
}

/// Rust-only facts resolved against the host's explicit home. They can contain clear
/// secrets for substitution; unlike bridge DTOs, these types never implement Serialize.
pub struct LocalFacts {
    /// All source and private override values needed to scrub this port's display.
    pub secrets: std::sync::Arc<redact::Secrets>,
    /// Alias names already joined by file identity, not by displayed text.
    pub aliases: Vec<String>,
    /// Opaque comparison with this observed desired record, when safe to act on it.
    pub restore: Option<Token>,
}
/// Bulk file observation on discovery's blocking worker. Absence means that a port's
/// local strings cannot be certified, not permission to send them unsanitized.
pub type InspectHome =
    std::sync::Arc<dyn Fn(&[enumerate::Enumerated]) -> BTreeMap<u16, LocalFacts> + Send + Sync>;
#[derive(PartialEq)]
struct Sample {
    history_allowed: bool,
    run: Option<view::RunKey>,
    pid: Option<u32>,
    secrets: std::sync::Arc<redact::Secrets>,
    meta: Option<RunMeta>,
    restore: Option<Token>,
}
/// One replaceable memory sample for the whole window; never read files on snapshot
/// or look up old secrets by port after the run has been replaced.
#[derive(Default, PartialEq)]
pub struct Survey {
    ports: BTreeMap<u16, Sample>,
    watchdog: Option<Watchdog>,
}
impl Survey {
    /// Cursor pages must not reuse display strings from a superseded privacy context.
    pub fn source_status(&self, run: &Run, other: &Self) -> (bool, bool) {
        let key = run.key();
        let current = other.ports.get(&key.port);
        let display = current.is_none_or(|s| Projection::new(&s.secrets).run(&key).is_some());
        let same = self
            .ports
            .get(&key.port)
            .zip(current)
            .is_some_and(|(a, b)| {
                a.run.as_ref() == Some(&key)
                    && a.run == b.run
                    && b.history_allowed
                    && a.pid == b.pid
                    && a.secrets == b.secrets
            });
        (same, display)
    }

    /// Produce a full replacement; no handles for retired ports grow unbounded.
    pub fn observe(
        members: &[enumerate::Enumerated],
        watchdog: Option<enumerate::WatchdogView>,
        mut local: BTreeMap<u16, LocalFacts>,
    ) -> Self {
        Self {
            ports: members
                .iter()
                .filter_map(|m| {
                    let f = local.remove(&m.port)?;
                    let p = Projection::new(&f.secrets);
                    let meta = p.metadata(m, &f.aliases);
                    Some((
                        m.port,
                        Sample {
                            history_allowed: m.status.as_ref().is_none_or(|s| {
                                s.features
                                    .iter()
                                    .any(|f| f == crate::wire::FEATURE_REDACTED_CLIENT_FIELDS)
                            }),
                            run: m.instance.as_ref().map(|r| view::RunKey {
                                port: m.port,
                                instance_id: r.instance_id.clone(),
                            }),
                            pid: m.instance.as_ref().map(|r| r.pid),
                            secrets: f.secrets,
                            meta,
                            restore: f.restore,
                        },
                    ))
                })
                .collect(),
            watchdog: watchdog.and_then(|w| Watchdog::project(w).ok()),
        }
    }
    /// Current credentials for an exact generation, never serialized or returned to JS.
    pub fn secrets(&self, key: &view::RunKey) -> Option<std::sync::Arc<redact::Secrets>> {
        self.ports
            .get(&key.port)
            .filter(|p| p.run.as_ref() == Some(key))
            .map(|p| p.secrets.clone())
    }
    /// Memory-only card projection. On missing/mismatched local facts suppress all
    /// local strings and capabilities, but preserve non-sensitive membership and counts.
    pub fn card(&self, mut item: view::InstanceSummary) -> R<Card> {
        if let Some(p) = self
            .ports
            .get(&item.port)
            .filter(|p| p.run == item.run && p.pid == item.pid)
        {
            return Projection::new(&p.secrets).card(item, p.meta.as_ref(), p.restore.clone());
        }
        item.run = None;
        item.version = None;
        item.launch_reason = None;
        item.launch_note = None;
        Projection::new(&redact::Secrets::default()).card(item, None, None)
    }
    /// Latest watchdog projection; an absent owner is distinct from a disabled setting.
    pub fn watchdog(&self) -> Option<Watchdog> {
        self.watchdog.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn legacy_peer_is_identity_and_counters_only() {
        let source = json!({"port":80,"run":{"port":80,"instance_id":"1234567890abcdef"},"pid":5,"version":"2.0.0","liveness":"live","restore_pending":false,"reads":{"history_feed":true,"body":true,"detail":true},"history_unavailable":"source_redaction_missing","feed":"polling","state":{"state":"serving","activity":{"last_request_at":0,"requests_total":1,"retries_total":0,"by_code":{"foo secret":3,"bar secret":4},"last_error":{"code":"internal","details":{"status":500},"message":"private peer text","at":42}}}});
        let known = redact::Secrets::default();
        let p = Projection::new(&known);
        let card = p
            .card(serde_json::from_value(source).unwrap(), None, None)
            .unwrap();
        assert!(card.run.is_some());
        assert_eq!(card.history_unavailable, Some("source-redaction-missing"));
        assert!(!card.reads.detail && !card.reads.body && !card.reads.history_feed);
        let out = serde_json::to_value(card).unwrap();
        assert!(out["activity"]["lastError"].is_null());
        assert_eq!(out["activity"]["byCode"], json!({"unknown":7}));
        assert!(!out.to_string().contains("private peer text"));
    }
}
