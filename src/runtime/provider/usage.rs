//! The newest quota usage each login's provider reported, kept in memory for
//! `/usage`. Only typed, allowlisted fields are kept; nothing is persisted, so a
//! restarted daemon reports nothing until the next provider report.
mod incoming;
use crate::runtime::domain::NativeSessionUsage;
use crate::runtime::locks::MutexExt;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Mutex;

// The same plausibility bound availability applies to native reset times.
const MAX_RESET_DELAY_SECONDS: u64 = 32 * 86_400;
// Larger used percentages are implausible and dropped.
const MAX_PERCENT: f64 = 1000.0;
// Windows kept per login; a provider never reports more than a few.
const MAX_WINDOWS: usize = 8;
// Characters of a window name or a credit balance kept.
const NAME_CHARS: usize = 40;
const BALANCE_CHARS: usize = 32;
const FIVE_HOURS: u64 = 300;
const SEVEN_DAYS: u64 = 10_080;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Provider {
    Codex,
    Claude,
}

/// Snapshots by login and credential generation: a repaired login runs its
/// old and new generations side by side, each with its own account.
#[derive(Default)]
pub(crate) struct Usages(Mutex<BTreeMap<(String, uuid::Uuid), Snapshot>>);

#[derive(Clone, Debug)]
struct Snapshot {
    provider: Provider,
    observed_at: u64,
    /// Codex windows by slot (`primary`, `secondary`); Claude windows by their
    /// native name.
    windows: BTreeMap<String, Window>,
    credits: Option<CodexCredits>,
    extra_usage: Option<ExtraUsage>,
    native_session: Option<NativeSessionUsage>,
    native_sequence: u64,
}

#[derive(Clone, Debug, PartialEq)]
struct Window {
    used_percent: f64,
    resets_at: Option<u64>,
    minutes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CodexCredits {
    pub has_credits: bool,
    pub unlimited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExtraStatus {
    Allowed,
    AllowedWarning,
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ExtraUsage {
    pub status: ExtraStatus,
    pub in_use: bool,
}

/// One login's usage, as the `usage` operation reports it.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Report {
    pub auth: String,
    pub provider: Provider,
    pub generation: String,
    pub observed_at: u64,
    pub windows: Vec<WindowReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<CodexCredits>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_usage: Option<ExtraUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_session: Option<NativeSessionUsage>,
}

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct WindowReport {
    pub name: String,
    pub used_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minutes: Option<u64>,
}

/// What one provider report contributes.
struct Observed {
    windows: Vec<(String, Window)>,
    credits: Option<CodexCredits>,
    extra_usage: Option<ExtraUsage>,
}

struct Source<'a> {
    auth: &'a str,
    provider: Provider,
    generation: uuid::Uuid,
}

impl Usages {
    /// Called only after the execution-fenced usage event and agent save commit.
    pub(crate) fn native(
        &self,
        auth: &str,
        generation: uuid::Uuid,
        source: NativeSessionUsage,
        sequence: u64,
    ) {
        if !source.usage.successful() {
            return;
        }
        let mut snapshots = self.0.lock_or_panic();
        let snapshot = snapshots
            .entry((auth.to_owned(), generation))
            .or_insert_with(|| Snapshot::new(Provider::Claude));
        if snapshot.provider == Provider::Claude && sequence > snapshot.native_sequence {
            snapshot.native_session = Some(source);
            snapshot.native_sequence = sequence;
        }
    }
    /// A Codex `RateLimitSnapshot`, from the start read or a sparse update.
    /// Only the account's `codex` limit is kept; a model-specific bucket would
    /// overwrite it.
    pub(crate) fn codex(&self, auth: &str, generation: uuid::Uuid, snapshot: &Value, now: u64) {
        let snapshot = incoming::Codex::read(snapshot);
        if !snapshot.account_limit() {
            return;
        }
        let windows = [
            ("primary", &snapshot.primary),
            ("secondary", &snapshot.secondary),
        ]
        .into_iter()
        .filter_map(|(slot, window)| {
            Some((
                slot.to_owned(),
                measured(
                    window.used_percent?,
                    window.resets_at,
                    window.window_duration_mins.filter(|minutes| *minutes > 0),
                    now,
                )?,
            ))
        })
        .collect();
        let credits = snapshot.credits.report();
        let observed = Observed {
            windows,
            credits,
            extra_usage: None,
        };
        self.observe(
            Source {
                auth,
                provider: Provider::Codex,
                generation,
            },
            observed,
            now,
        );
    }

    /// A Claude `rate_limit_event`'s `rate_limit_info`.
    pub(crate) fn claude(&self, auth: &str, generation: uuid::Uuid, info: &Value, now: u64) {
        let info = incoming::Claude::read(info);
        let windows = info
            .unified_windows
            .into_iter()
            .filter(|(name, _)| {
                (1..=NAME_CHARS).contains(&name.len())
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            })
            .filter_map(|(name, window)| {
                let used = (window.0.utilization? * 10_000.0).round() / 100.0;
                Some((
                    name.to_owned(),
                    measured(used, window.0.resets_at, None, now)?,
                ))
            })
            .collect();
        let status = match info.overage_status.as_deref() {
            Some("allowed") => Some(ExtraStatus::Allowed),
            Some("allowed_warning") => Some(ExtraStatus::AllowedWarning),
            Some("rejected") => Some(ExtraStatus::Rejected),
            _ => None,
        };
        let observed = Observed {
            windows,
            credits: None,
            extra_usage: status.map(|status| ExtraUsage {
                status,
                in_use: info.is_using_overage == Some(true),
            }),
        };
        self.observe(
            Source {
                auth,
                provider: Provider::Claude,
                generation,
            },
            observed,
            now,
        );
    }

    /// Merge a report into its login generation's snapshot. A report from
    /// another provider starts a new snapshot; one that carries nothing usable
    /// changes nothing.
    fn observe(&self, source: Source<'_>, observed: Observed, now: u64) {
        let Source {
            auth,
            provider,
            generation,
        } = source;
        if observed.windows.is_empty()
            && observed.credits.is_none()
            && observed.extra_usage.is_none()
        {
            return;
        }
        let mut snapshots = self.0.lock_or_panic();
        let snapshot = snapshots
            .entry((auth.to_owned(), generation))
            .and_modify(|snapshot| {
                if snapshot.provider != provider {
                    *snapshot = Snapshot::new(provider);
                }
            })
            .or_insert_with(|| Snapshot::new(provider));
        for (name, window) in observed.windows {
            if snapshot.windows.len() < MAX_WINDOWS || snapshot.windows.contains_key(&name) {
                snapshot.windows.insert(name, window);
            }
        }
        if observed.credits.is_some() {
            snapshot.credits = observed.credits;
        }
        if observed.extra_usage.is_some() {
            snapshot.extra_usage = observed.extra_usage;
        }
        snapshot.observed_at = now;
    }

    /// Every login's reports, or only `auth`'s: by login, newest first.
    pub(crate) fn reports(&self, auth: Option<&str>) -> Vec<Report> {
        let mut reports: Vec<Report> = self
            .0
            .lock_or_panic()
            .iter()
            .filter(|((login, _), _)| auth.is_none_or(|auth| auth == login.as_str()))
            .map(|((login, generation), snapshot)| snapshot.report(login, *generation))
            .collect();
        reports.sort_by(|a, b| {
            (&a.auth, b.newest_observation()).cmp(&(&b.auth, a.newest_observation()))
        });
        reports
    }
}

impl Snapshot {
    fn new(provider: Provider) -> Self {
        Self {
            provider,
            observed_at: 0,
            windows: BTreeMap::new(),
            credits: None,
            extra_usage: None,
            native_session: None,
            native_sequence: 0,
        }
    }

    fn report(&self, auth: &str, generation: uuid::Uuid) -> Report {
        let mut windows: Vec<WindowReport> = self
            .windows
            .iter()
            .map(|(key, window)| WindowReport {
                name: match (self.provider, window.minutes) {
                    (Provider::Codex, Some(FIVE_HOURS)) => String::from("five_hour"),
                    (Provider::Codex, Some(SEVEN_DAYS)) => String::from("seven_day"),
                    _ => key.clone(),
                },
                used_percent: window.used_percent,
                resets_at: window.resets_at,
                minutes: window.minutes,
            })
            .collect();
        windows.sort_by(|a, b| (rank(&a.name), &a.name).cmp(&(rank(&b.name), &b.name)));
        Report {
            auth: auth.to_owned(),
            provider: self.provider,
            generation: generation.to_string(),
            observed_at: self.observed_at,
            windows,
            credits: self.credits.clone(),
            extra_usage: self.extra_usage,
            native_session: self.native_session.clone(),
        }
    }
}
impl Report {
    fn newest_observation(&self) -> u64 {
        self.native_session
            .as_ref()
            .map_or(self.observed_at, |source| {
                self.observed_at.max(source.usage.observed_at)
            })
    }
}

/// The short window first, then the week, then the rest by name.
fn rank(name: &str) -> u8 {
    match name {
        "five_hour" | "primary" => 0,
        "seven_day" | "secondary" => 1,
        _ => 2,
    }
}

/// A plausible window: a finite percentage and, when present, a reset time
/// after the epoch and within the bound.
fn measured(used: f64, reset: Option<u64>, minutes: Option<u64>, now: u64) -> Option<Window> {
    if !used.is_finite() || !(0.0..=MAX_PERCENT).contains(&used) {
        return None;
    }
    Some(Window {
        used_percent: used,
        resets_at: reset.filter(|at| *at > 0 && *at <= now.saturating_add(MAX_RESET_DELAY_SECONDS)),
        minutes,
    })
}

#[cfg(test)]
mod tests;
