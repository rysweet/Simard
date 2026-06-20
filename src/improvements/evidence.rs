//! Typed evidence references that flow from benchmark/session/review signals
//! through improvement hypotheses, promotion, and durable goals.
//!
//! Per `Specs/ProductArchitecture.md` line 696, every promoted change must
//! link to evidence. `EvidenceRef` is the typed link that survives the path
//! `ImprovementProposal → ImprovementPromotionPlan → GoalUpdate → GoalRecord`.
//!
//! The enum is serde-tagged on a `kind` field so JSON-stored goal records can
//! be re-loaded across schema versions, and provides a `parse_str` that
//! recognises the legacy `Vec<String>` shape produced by today's review and
//! proposal pipelines. Strings that do not match a structured shape are kept
//! as [`EvidenceRef::Raw`] so no information is lost.
//!
//! This is distinct from [`crate::evidence::EvidenceRecord`], which is a
//! runtime-produced record of evidence captured during a session. An
//! [`EvidenceRef`] is a *pointer* to evidence (a benchmark report, a review
//! artifact, a score record, …) that justifies an improvement.

use serde::{Deserialize, Serialize};

/// A typed reference to a piece of evidence supporting an improvement
/// hypothesis or a promoted goal update.
///
/// Variants are intentionally narrow so callers can pattern-match without
/// stringly-typed parsing. New evidence sources should add a variant rather
/// than smuggling structure inside [`EvidenceRef::Raw`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EvidenceRef {
    /// A specific benchmark scenario (suite + scenario id).
    BenchmarkScenario {
        suite_id: String,
        scenario_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    /// A specific benchmark run with timestamp + session id.
    BenchmarkRunReport {
        suite_id: String,
        scenario_id: String,
        session_id: String,
        /// Unix epoch milliseconds. Stored as `u64` so the value serialises
        /// through `serde_json` (which does not support `u128`); a `u64` of
        /// milliseconds covers timestamps up to year 584_942_417.
        run_started_at_unix_ms: u64,
    },
    /// A failed benchmark correctness check.
    BenchmarkCheckFailure {
        suite_id: String,
        scenario_id: String,
        check_id: String,
        detail: String,
    },
    /// A persisted gym score record.
    ScoreRecord {
        suite_id: String,
        scenario_id: String,
        timestamp_unix_s: i64,
    },
    /// A weak scoring dimension surfaced by analysis.
    WeakDimension { dimension: String, deficit: f64 },
    /// A persisted review artifact (by review id).
    Review {
        review_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_label: Option<String>,
    },
    /// A failed review signal observed during reflection.
    SessionFailure {
        session_id: String,
        signal_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// A raw string evidence label that could not be parsed into a structured
    /// variant. Preserves source data verbatim so promotion never silently
    /// drops a reference.
    Raw { label: String },
}

impl EvidenceRef {
    /// Build a [`EvidenceRef::Raw`] evidence ref from a label.
    pub fn raw(label: impl Into<String>) -> Self {
        Self::Raw {
            label: label.into(),
        }
    }

    /// Parse a free-form evidence string into a structured [`EvidenceRef`].
    ///
    /// Recognised shapes (kept narrow on purpose — the goal is back-compat
    /// with the current `Vec<String>` produced by reviewers and benchmark
    /// pipelines):
    ///
    /// - `review:<review-id>` or `review-id=<review-id>` / `review-id:<id>`
    ///   (with optional `@target=<label>` suffix)
    /// - `benchmark:<suite>/<scenario>` or
    ///   `benchmark-scenario:<suite>/<scenario>` (with optional
    ///   `@session=<session-id>` suffix)
    /// - `benchmark-run:<suite>/<scenario>@session=<session-id>@ms=<u64>`
    /// - `score:<suite>/<scenario>@<timestamp_unix_s>`
    /// - `weak-dimension:<name>@<deficit-as-f64>`
    /// - `check-failure:<suite>/<scenario>/<check_id>:<detail>`
    /// - `session-failure:<session_id>/<signal_id>` (with optional `:<detail>`)
    ///
    /// These shapes are the exact inverse of [`Self::to_persisted_string`], so
    /// every structured variant survives a `to_persisted_string` →
    /// `parse_str` round trip. Anything else is preserved as
    /// [`EvidenceRef::Raw`].
    pub fn parse_str(raw: &str) -> Self {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Self::Raw {
                label: String::new(),
            };
        }

        if let Some(rest) = strip_prefix_ci(trimmed, "review:")
            .or_else(|| strip_prefix_ci(trimmed, "review-id="))
            .or_else(|| strip_prefix_ci(trimmed, "review-id:"))
        {
            let rest = rest.trim();
            if !rest.is_empty() {
                let (review_id, target_label) = match rest.split_once("@target=") {
                    Some((id, label)) => {
                        let label = label.trim();
                        (
                            id.trim(),
                            if label.is_empty() {
                                None
                            } else {
                                Some(label.to_string())
                            },
                        )
                    }
                    None => (rest, None),
                };
                if !review_id.is_empty() {
                    return Self::Review {
                        review_id: review_id.to_string(),
                        target_label,
                    };
                }
            }
        }

        if let Some(rest) = strip_prefix_ci(trimmed, "benchmark-run:") {
            let rest = rest.trim();
            if let Some((id_part, meta)) = rest.split_once("@session=")
                && let Some((suite, scenario)) = id_part.split_once('/')
                && let Some((session, ms_part)) = meta.split_once("@ms=")
                && let Ok(ms) = ms_part.trim().parse::<u64>()
                && !suite.trim().is_empty()
                && !scenario.trim().is_empty()
                && !session.trim().is_empty()
            {
                return Self::BenchmarkRunReport {
                    suite_id: suite.trim().to_string(),
                    scenario_id: scenario.trim().to_string(),
                    session_id: session.trim().to_string(),
                    run_started_at_unix_ms: ms,
                };
            }
        }

        if let Some(rest) = strip_prefix_ci(trimmed, "benchmark-scenario:")
            .or_else(|| strip_prefix_ci(trimmed, "benchmark:"))
            && let Some((suite, scenario_and_meta)) = rest.trim().split_once('/')
            && !suite.trim().is_empty()
        {
            let (scenario, session_id) = match scenario_and_meta.split_once("@session=") {
                Some((scenario, sid)) => {
                    let sid = sid.trim();
                    (
                        scenario,
                        if sid.is_empty() {
                            None
                        } else {
                            Some(sid.to_string())
                        },
                    )
                }
                None => (scenario_and_meta, None),
            };
            if !scenario.trim().is_empty() {
                return Self::BenchmarkScenario {
                    suite_id: suite.trim().to_string(),
                    scenario_id: scenario.trim().to_string(),
                    session_id,
                };
            }
        }

        if let Some(rest) = strip_prefix_ci(trimmed, "score:")
            && let Some((id_part, ts_part)) = rest.trim().split_once('@')
            && let Some((suite, scenario)) = id_part.split_once('/')
            && let Ok(ts) = ts_part.trim().parse::<i64>()
            && !suite.trim().is_empty()
            && !scenario.trim().is_empty()
        {
            return Self::ScoreRecord {
                suite_id: suite.trim().to_string(),
                scenario_id: scenario.trim().to_string(),
                timestamp_unix_s: ts,
            };
        }

        if let Some(rest) = strip_prefix_ci(trimmed, "weak-dimension:")
            && let Some((name, deficit_part)) = rest.trim().split_once('@')
            && let Ok(deficit) = deficit_part.trim().parse::<f64>()
            && !name.trim().is_empty()
        {
            return Self::WeakDimension {
                dimension: name.trim().to_string(),
                deficit,
            };
        }

        if let Some(rest) = strip_prefix_ci(trimmed, "check-failure:") {
            let rest = rest.trim();
            let (path, detail) = rest.split_once(':').unwrap_or((rest, ""));
            let parts: Vec<&str> = path.splitn(3, '/').collect();
            if parts.len() == 3
                && !parts[0].trim().is_empty()
                && !parts[1].trim().is_empty()
                && !parts[2].trim().is_empty()
            {
                return Self::BenchmarkCheckFailure {
                    suite_id: parts[0].trim().to_string(),
                    scenario_id: parts[1].trim().to_string(),
                    check_id: parts[2].trim().to_string(),
                    detail: detail.trim().to_string(),
                };
            }
        }

        if let Some(rest) = strip_prefix_ci(trimmed, "session-failure:") {
            let rest = rest.trim();
            let (path, detail_part) = rest.split_once(':').unwrap_or((rest, ""));
            if let Some((session, signal)) = path.split_once('/')
                && !session.trim().is_empty()
                && !signal.trim().is_empty()
            {
                let detail = detail_part.trim();
                return Self::SessionFailure {
                    session_id: session.trim().to_string(),
                    signal_id: signal.trim().to_string(),
                    detail: if detail.is_empty() {
                        None
                    } else {
                        Some(detail.to_string())
                    },
                };
            }
        }

        Self::Raw {
            label: trimmed.to_string(),
        }
    }

    /// Render the evidence ref back into a deterministic string form so it
    /// can be persisted in stringly-typed slots (e.g. the legacy `evidence`
    /// directive segment in
    /// [`crate::improvements::ImprovementPromotionPlan`]).
    pub fn to_persisted_string(&self) -> String {
        match self {
            Self::BenchmarkScenario {
                suite_id,
                scenario_id,
                session_id,
            } => match session_id {
                Some(sid) => format!("benchmark:{suite_id}/{scenario_id}@session={sid}"),
                None => format!("benchmark:{suite_id}/{scenario_id}"),
            },
            Self::BenchmarkRunReport {
                suite_id,
                scenario_id,
                session_id,
                run_started_at_unix_ms,
            } => format!(
                "benchmark-run:{suite_id}/{scenario_id}@session={session_id}@ms={run_started_at_unix_ms}"
            ),
            Self::BenchmarkCheckFailure {
                suite_id,
                scenario_id,
                check_id,
                detail,
            } => format!("check-failure:{suite_id}/{scenario_id}/{check_id}:{detail}"),
            Self::ScoreRecord {
                suite_id,
                scenario_id,
                timestamp_unix_s,
            } => format!("score:{suite_id}/{scenario_id}@{timestamp_unix_s}"),
            Self::WeakDimension { dimension, deficit } => {
                format!("weak-dimension:{dimension}@{deficit}")
            }
            Self::Review {
                review_id,
                target_label,
            } => match target_label {
                Some(label) => format!("review:{review_id}@target={label}"),
                None => format!("review:{review_id}"),
            },
            Self::SessionFailure {
                session_id,
                signal_id,
                detail,
            } => match detail {
                Some(d) => format!("session-failure:{session_id}/{signal_id}:{d}"),
                None => format!("session-failure:{session_id}/{signal_id}"),
            },
            Self::Raw { label } => label.clone(),
        }
    }

    /// Parse a collection of free-form evidence strings, preserving order.
    pub fn parse_all<I, S>(items: I) -> Vec<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        items
            .into_iter()
            .map(|s| Self::parse_str(s.as_ref()))
            .collect()
    }
}

fn strip_prefix_ci<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    // `str::get(..n)` returns `None` both when `n` is past the end and when it
    // lands inside a multibyte UTF-8 character, so it is panic-safe at a byte
    // boundary that splits a char (e.g. an em-dash or emoji in agent-authored
    // evidence). `str::split_at` would panic in that case.
    let head = value.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) {
        Some(&value[prefix.len()..])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_review_evidence() {
        let ev = EvidenceRef::parse_str("review:rev-001");
        assert_eq!(
            ev,
            EvidenceRef::Review {
                review_id: "rev-001".to_string(),
                target_label: None,
            }
        );
    }

    #[test]
    fn parse_review_id_equals_alias() {
        let ev = EvidenceRef::parse_str("review-id=rev-002");
        assert_eq!(
            ev,
            EvidenceRef::Review {
                review_id: "rev-002".to_string(),
                target_label: None,
            }
        );
    }

    #[test]
    fn parse_benchmark_scenario_evidence() {
        let ev = EvidenceRef::parse_str("benchmark:gym-suite/echo-1");
        assert_eq!(
            ev,
            EvidenceRef::BenchmarkScenario {
                suite_id: "gym-suite".to_string(),
                scenario_id: "echo-1".to_string(),
                session_id: None,
            }
        );
    }

    #[test]
    fn parse_score_record_evidence() {
        let ev = EvidenceRef::parse_str("score:gym/echo@1700000000");
        assert_eq!(
            ev,
            EvidenceRef::ScoreRecord {
                suite_id: "gym".to_string(),
                scenario_id: "echo".to_string(),
                timestamp_unix_s: 1_700_000_000,
            }
        );
    }

    #[test]
    fn parse_weak_dimension_evidence() {
        let ev = EvidenceRef::parse_str("weak-dimension:specificity@0.4");
        assert_eq!(
            ev,
            EvidenceRef::WeakDimension {
                dimension: "specificity".to_string(),
                deficit: 0.4,
            }
        );
    }

    #[test]
    fn parse_check_failure_evidence() {
        let ev = EvidenceRef::parse_str("check-failure:gym/echo/exit-code:expected 0 got 1");
        assert_eq!(
            ev,
            EvidenceRef::BenchmarkCheckFailure {
                suite_id: "gym".to_string(),
                scenario_id: "echo".to_string(),
                check_id: "exit-code".to_string(),
                detail: "expected 0 got 1".to_string(),
            }
        );
    }

    #[test]
    fn parse_session_failure_evidence() {
        let ev = EvidenceRef::parse_str("session-failure:sess-1/signal-a:something broke");
        assert_eq!(
            ev,
            EvidenceRef::SessionFailure {
                session_id: "sess-1".to_string(),
                signal_id: "signal-a".to_string(),
                detail: Some("something broke".to_string()),
            }
        );
    }

    #[test]
    fn parse_session_failure_without_detail() {
        let ev = EvidenceRef::parse_str("session-failure:sess-1/signal-a");
        assert_eq!(
            ev,
            EvidenceRef::SessionFailure {
                session_id: "sess-1".to_string(),
                signal_id: "signal-a".to_string(),
                detail: None,
            }
        );
    }

    #[test]
    fn parse_unrecognised_falls_back_to_raw() {
        let ev = EvidenceRef::parse_str("phase-1");
        assert_eq!(
            ev,
            EvidenceRef::Raw {
                label: "phase-1".to_string(),
            }
        );
    }

    #[test]
    fn parse_empty_yields_empty_raw() {
        let ev = EvidenceRef::parse_str("   ");
        assert_eq!(
            ev,
            EvidenceRef::Raw {
                label: String::new(),
            }
        );
    }

    #[test]
    fn to_persisted_string_round_trip_for_structured_variants() {
        let cases = vec![
            EvidenceRef::Review {
                review_id: "rev-7".into(),
                target_label: None,
            },
            EvidenceRef::Review {
                review_id: "rev-8".into(),
                target_label: Some("operator-review".into()),
            },
            EvidenceRef::BenchmarkScenario {
                suite_id: "gym".into(),
                scenario_id: "echo".into(),
                session_id: None,
            },
            EvidenceRef::BenchmarkScenario {
                suite_id: "gym".into(),
                scenario_id: "echo".into(),
                session_id: Some("sess-1".into()),
            },
            EvidenceRef::BenchmarkRunReport {
                suite_id: "gym".into(),
                scenario_id: "echo".into(),
                session_id: "sess-1".into(),
                run_started_at_unix_ms: 1_700_000_000_000,
            },
            EvidenceRef::ScoreRecord {
                suite_id: "gym".into(),
                scenario_id: "echo".into(),
                timestamp_unix_s: 1_700_000_000,
            },
            EvidenceRef::WeakDimension {
                dimension: "specificity".into(),
                deficit: 0.4,
            },
            EvidenceRef::BenchmarkCheckFailure {
                suite_id: "gym".into(),
                scenario_id: "echo".into(),
                check_id: "exit-code".into(),
                detail: "boom".into(),
            },
            EvidenceRef::SessionFailure {
                session_id: "sess-1".into(),
                signal_id: "signal-a".into(),
                detail: Some("broke".into()),
            },
        ];
        for ev in cases {
            let s = ev.to_persisted_string();
            let parsed = EvidenceRef::parse_str(&s);
            assert_eq!(parsed, ev, "round trip failed for {s}");
        }
    }

    #[test]
    fn parse_str_does_not_panic_on_multibyte_boundary() {
        // An em-dash / emoji / accented char whose bytes straddle one of the
        // probed prefix lengths (e.g. `review:` = 7 bytes) must not panic —
        // `parse_str` is called directly on agent/operator-authored evidence
        // strings, which routinely contain such characters.
        for input in [
            "score—X",
            "emoji 😀 token here",
            "reviewéx",
            "review:rev—1",
            "benchmark😀",
        ] {
            let ev = EvidenceRef::parse_str(input);
            // Whatever it parses to, it must preserve the input as data and
            // never panic.
            let _ = ev;
        }
    }

    #[test]
    fn raw_string_round_trip() {
        let ev = EvidenceRef::raw("free-form");
        let parsed = EvidenceRef::parse_str(&ev.to_persisted_string());
        assert_eq!(parsed, ev);
    }

    #[test]
    fn parse_all_preserves_order_and_handles_mixed_input() {
        let items = vec!["review:rev-1", "free-form note", "benchmark:gym/echo", ""];
        let parsed = EvidenceRef::parse_all(items);
        assert_eq!(parsed.len(), 4);
        assert!(matches!(parsed[0], EvidenceRef::Review { .. }));
        assert!(matches!(parsed[1], EvidenceRef::Raw { .. }));
        assert!(matches!(parsed[2], EvidenceRef::BenchmarkScenario { .. }));
        assert!(matches!(parsed[3], EvidenceRef::Raw { .. }));
    }

    #[test]
    fn serde_round_trip_review() {
        let ev = EvidenceRef::Review {
            review_id: "rev-99".to_string(),
            target_label: Some("operator-review".to_string()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: EvidenceRef = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn serde_round_trip_benchmark_run_report() {
        let ev = EvidenceRef::BenchmarkRunReport {
            suite_id: "gym".into(),
            scenario_id: "echo".into(),
            session_id: "sess-1".into(),
            run_started_at_unix_ms: 1_700_000_000_000,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: EvidenceRef = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn serde_uses_kebab_case_kind_tag() {
        let ev = EvidenceRef::WeakDimension {
            dimension: "specificity".into(),
            deficit: 0.4,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"kind\":\"weak-dimension\""), "json={json}");
    }
}
