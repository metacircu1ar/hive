//! Frozen S5 usage projection, for partial-update comparisons.
use super::*;
impl Usages {
    pub(super) fn original_codex(
        &self,
        auth: &str,
        generation: uuid::Uuid,
        snapshot: &Value,
        now: u64,
    ) {
        match snapshot.get("limitId") {
            None | Some(Value::Null) => {}
            Some(id) if id == "codex" => {}
            Some(_) => return,
        }
        let windows = ["primary", "secondary"]
            .into_iter()
            .filter_map(|slot| {
                let window = snapshot.get(slot)?;
                let minutes = window
                    .get("windowDurationMins")
                    .and_then(Value::as_u64)
                    .filter(|minutes| *minutes > 0);
                let used = window.get("usedPercent").and_then(Value::as_f64)?;
                Some((
                    slot.to_owned(),
                    original_measured(used, window.get("resetsAt"), minutes, now)?,
                ))
            })
            .collect();
        let credits = snapshot.get("credits").and_then(|credits| {
            Some(CodexCredits {
                has_credits: credits.get("hasCredits")?.as_bool()?,
                unlimited: credits.get("unlimited")?.as_bool()?,
                balance: credits
                    .get("balance")
                    .and_then(Value::as_str)
                    .filter(|balance| {
                        (1..=BALANCE_CHARS).contains(&balance.len())
                            && balance
                                .chars()
                                .all(|c| c.is_ascii_digit() || ".,-".contains(c))
                    })
                    .map(str::to_owned),
            })
        });
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
    pub(super) fn original_claude(
        &self,
        auth: &str,
        generation: uuid::Uuid,
        info: &Value,
        now: u64,
    ) {
        let windows = info
            .get("unifiedWindows")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter(|(name, _)| {
                (1..=NAME_CHARS).contains(&name.len())
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            })
            .filter_map(|(name, window)| {
                // A fraction; two decimals of percent keep `0.07` from reading
                // as `7.000000000000001`.
                let used =
                    (window.get("utilization").and_then(Value::as_f64)? * 10_000.0).round() / 100.0;
                Some((
                    name.clone(),
                    original_measured(used, window.get("resetsAt"), None, now)?,
                ))
            })
            .collect();
        let status = match info.get("overageStatus").and_then(Value::as_str) {
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
                in_use: info.get("isUsingOverage") == Some(&Value::Bool(true)),
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
}

fn original_measured(
    used: f64,
    reset: Option<&Value>,
    minutes: Option<u64>,
    now: u64,
) -> Option<Window> {
    measured(used, reset.and_then(Value::as_u64), minutes, now)
}
