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
    /// [`Self::to_persisted_string`] percent-escapes the structural delimiters
    /// (`%`, `/`, `:`, `@`) inside every field, and `parse_str` reverses that
    /// escaping, so a *structured* [`EvidenceRef`] survives a
    /// `to_persisted_string` → `parse_str` round trip exactly — even when a
    /// field value itself contains a delimiter or one of the literal markers.
    /// Two documented normalisations apply: an optional field that is
    /// `Some("")` round-trips to `None` (an empty suffix carries no
    /// information), and surrounding whitespace in a field is trimmed. Legacy,
    /// unescaped evidence strings (as produced by today's reviewer / proposal
    /// pipelines) are still accepted on a best-effort basis; anything that
    /// matches no structured shape is preserved verbatim as
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
                                Some(unescape_segment(label))
                            },
                        )
                    }
                    None => (rest, None),
                };
                if !review_id.is_empty() {
                    return Self::Review {
                        review_id: unescape_segment(review_id),
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
                    suite_id: unescape_segment(suite.trim()),
                    scenario_id: unescape_segment(scenario.trim()),
                    session_id: unescape_segment(session.trim()),
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
                            Some(unescape_segment(sid))
                        },
                    )
                }
                None => (scenario_and_meta, None),
            };
            if !scenario.trim().is_empty() {
                return Self::BenchmarkScenario {
                    suite_id: unescape_segment(suite.trim()),
                    scenario_id: unescape_segment(scenario.trim()),
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
                suite_id: unescape_segment(suite.trim()),
                scenario_id: unescape_segment(scenario.trim()),
                timestamp_unix_s: ts,
            };
        }

        if let Some(rest) = strip_prefix_ci(trimmed, "weak-dimension:")
            && let Some((name, deficit_part)) = rest.trim().split_once('@')
            && let Ok(deficit) = deficit_part.trim().parse::<f64>()
            && deficit.is_finite()
            && !name.trim().is_empty()
        {
            return Self::WeakDimension {
                dimension: unescape_segment(name.trim()),
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
                    suite_id: unescape_segment(parts[0].trim()),
                    scenario_id: unescape_segment(parts[1].trim()),
                    check_id: unescape_segment(parts[2].trim()),
                    detail: unescape_segment(detail.trim()),
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
                    session_id: unescape_segment(session.trim()),
                    signal_id: unescape_segment(signal.trim()),
                    detail: if detail.is_empty() {
                        None
                    } else {
                        Some(unescape_segment(detail))
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
                Some(sid) if !sid.is_empty() => format!(
                    "benchmark:{}/{}@session={}",
                    escape_segment(suite_id),
                    escape_segment(scenario_id),
                    escape_segment(sid)
                ),
                _ => format!(
                    "benchmark:{}/{}",
                    escape_segment(suite_id),
                    escape_segment(scenario_id)
                ),
            },
            Self::BenchmarkRunReport {
                suite_id,
                scenario_id,
                session_id,
                run_started_at_unix_ms,
            } => format!(
                "benchmark-run:{}/{}@session={}@ms={run_started_at_unix_ms}",
                escape_segment(suite_id),
                escape_segment(scenario_id),
                escape_segment(session_id)
            ),
            Self::BenchmarkCheckFailure {
                suite_id,
                scenario_id,
                check_id,
                detail,
            } => format!(
                "check-failure:{}/{}/{}:{}",
                escape_segment(suite_id),
                escape_segment(scenario_id),
                escape_segment(check_id),
                escape_segment(detail)
            ),
            Self::ScoreRecord {
                suite_id,
                scenario_id,
                timestamp_unix_s,
            } => format!(
                "score:{}/{}@{timestamp_unix_s}",
                escape_segment(suite_id),
                escape_segment(scenario_id)
            ),
            Self::WeakDimension { dimension, deficit } => {
                format!("weak-dimension:{}@{deficit}", escape_segment(dimension))
            }
            Self::Review {
                review_id,
                target_label,
            } => match target_label {
                Some(label) if !label.is_empty() => format!(
                    "review:{}@target={}",
                    escape_segment(review_id),
                    escape_segment(label)
                ),
                _ => format!("review:{}", escape_segment(review_id)),
            },
            Self::SessionFailure {
                session_id,
                signal_id,
                detail,
            } => match detail {
                Some(d) if !d.is_empty() => format!(
                    "session-failure:{}/{}:{}",
                    escape_segment(session_id),
                    escape_segment(signal_id),
                    escape_segment(d)
                ),
                _ => format!(
                    "session-failure:{}/{}",
                    escape_segment(session_id),
                    escape_segment(signal_id)
                ),
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

/// Percent-escape the four structural delimiters used by
/// [`EvidenceRef::to_persisted_string`] (`%`, `/`, `:`, `@`) inside a field so
/// the value can contain any of them without colliding with the format
/// separators. The escape character `%` is encoded first so the transform is
/// unambiguous and exactly reversible by [`unescape_segment`].
fn escape_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '/' => out.push_str("%2F"),
            ':' => out.push_str("%3A"),
            '@' => out.push_str("%40"),
            _ => out.push(ch),
        }
    }
    out
}

/// Reverse [`escape_segment`]. The four exact uppercase tokens this module
/// emits (`%25 %2F %3A %40`) are reserved and always decoded; any other `%`
/// sequence (e.g. a literal "50%") is preserved verbatim. As a result a string
/// produced by [`escape_segment`] always decodes back exactly, and legacy
/// agent-authored prose — which does not contain those exact uppercase tokens —
/// is preserved unchanged. (A legacy *structured* string that happens to embed
/// one of the reserved tokens in a field is decoded; this is the deliberate
/// cost of a reversible scheme and is exercised by the tests below.)
fn unescape_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while !rest.is_empty() {
        if let Some(stripped) = rest.strip_prefix("%25") {
            out.push('%');
            rest = stripped;
        } else if let Some(stripped) = rest.strip_prefix("%2F") {
            out.push('/');
            rest = stripped;
        } else if let Some(stripped) = rest.strip_prefix("%3A") {
            out.push(':');
            rest = stripped;
        } else if let Some(stripped) = rest.strip_prefix("%40") {
            out.push('@');
            rest = stripped;
        } else {
            let mut chars = rest.chars();
            let ch = chars.next().expect("rest is non-empty");
            out.push(ch);
            rest = chars.as_str();
        }
    }
    out
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

    #[test]
    fn round_trip_survives_reserved_delimiters_in_every_field() {
        // Each field embeds the structural delimiters (`/ : @`), the escape
        // char (`%`), and the literal markers (`@session= @ms= @target=`); a
        // regression in escaping would silently corrupt or downgrade-to-Raw.
        // Distinct per-field sentinels also catch any field-position swap.
        let f = |tag: &str| format!("{tag}/a:b@c%d@session=e@ms=9@target=g");
        let cases = vec![
            EvidenceRef::Review {
                review_id: f("rev-id"),
                target_label: Some(f("rev-target")),
            },
            EvidenceRef::BenchmarkScenario {
                suite_id: f("bs-suite"),
                scenario_id: f("bs-scenario"),
                session_id: Some(f("bs-session")),
            },
            EvidenceRef::BenchmarkRunReport {
                suite_id: f("brr-suite"),
                scenario_id: f("brr-scenario"),
                session_id: f("brr-session"),
                run_started_at_unix_ms: 1,
            },
            EvidenceRef::ScoreRecord {
                suite_id: f("sr-suite"),
                scenario_id: f("sr-scenario"),
                timestamp_unix_s: -5,
            },
            EvidenceRef::WeakDimension {
                dimension: f("wd-dim"),
                deficit: 0.25,
            },
            EvidenceRef::BenchmarkCheckFailure {
                suite_id: f("cf-suite"),
                scenario_id: f("cf-scenario"),
                check_id: f("cf-check"),
                detail: f("cf-detail"),
            },
            EvidenceRef::SessionFailure {
                session_id: f("sf-session"),
                signal_id: f("sf-signal"),
                detail: Some(f("sf-detail")),
            },
        ];
        for ev in cases {
            let s = ev.to_persisted_string();
            let parsed = EvidenceRef::parse_str(&s);
            assert_eq!(parsed, ev, "round trip failed for persisted form {s}");
        }
    }

    #[test]
    fn empty_optional_fields_normalise_to_none_on_round_trip() {
        // `Some("")` carries no information; the persisted form omits the
        // suffix and re-parses as `None`. Documented normalisation, not loss.
        let review = EvidenceRef::parse_str(
            &EvidenceRef::Review {
                review_id: "rev-1".into(),
                target_label: Some(String::new()),
            }
            .to_persisted_string(),
        );
        assert_eq!(
            review,
            EvidenceRef::Review {
                review_id: "rev-1".into(),
                target_label: None,
            }
        );

        let scenario = EvidenceRef::parse_str(
            &EvidenceRef::BenchmarkScenario {
                suite_id: "gym".into(),
                scenario_id: "echo".into(),
                session_id: Some(String::new()),
            }
            .to_persisted_string(),
        );
        assert_eq!(
            scenario,
            EvidenceRef::BenchmarkScenario {
                suite_id: "gym".into(),
                scenario_id: "echo".into(),
                session_id: None,
            }
        );

        let session = EvidenceRef::parse_str(
            &EvidenceRef::SessionFailure {
                session_id: "s".into(),
                signal_id: "sig".into(),
                detail: Some(String::new()),
            }
            .to_persisted_string(),
        );
        assert_eq!(
            session,
            EvidenceRef::SessionFailure {
                session_id: "s".into(),
                signal_id: "sig".into(),
                detail: None,
            }
        );
    }

    #[test]
    fn parse_rejects_non_finite_weak_dimension_deficit() {
        // A NaN/inf deficit would make `EvidenceRef` (and any GoalRecord that
        // carries it) non-reflexive under `PartialEq`, so a non-finite deficit
        // must never enter the evidence graph via the string path.
        for input in [
            "weak-dimension:specificity@NaN",
            "weak-dimension:specificity@inf",
            "weak-dimension:specificity@-inf",
        ] {
            assert!(
                matches!(EvidenceRef::parse_str(input), EvidenceRef::Raw { .. }),
                "non-finite deficit must not parse to WeakDimension: {input}"
            );
        }
        assert!(matches!(
            EvidenceRef::parse_str("weak-dimension:specificity@0.5"),
            EvidenceRef::WeakDimension { .. }
        ));
    }

    #[test]
    fn legacy_non_token_percent_text_is_preserved() {
        // A bare `%` that is not one of our four uppercase tokens is preserved
        // verbatim — both when the string falls through to Raw and when it sits
        // in a structured field.
        assert_eq!(
            EvidenceRef::parse_str("CPU spiked to 50% during run"),
            EvidenceRef::Raw {
                label: "CPU spiked to 50% during run".to_string(),
            }
        );
        match EvidenceRef::parse_str("check-failure:a/b/c:saw 50% drop") {
            EvidenceRef::BenchmarkCheckFailure { detail, .. } => {
                assert_eq!(detail, "saw 50% drop");
            }
            other => panic!("expected check failure, got {other:?}"),
        }
    }

    #[test]
    fn legacy_structured_string_with_reserved_token_is_decoded() {
        // Pin the documented tradeoff: a legacy structured string whose field
        // happens to contain one of the four reserved uppercase tokens is
        // decoded (this is exactly what makes to_persisted_string -> parse_str a
        // true inverse). Realistic legacy evidence is agent-authored prose that
        // does not contain exact uppercase percent-tokens, so impact is nil.
        match EvidenceRef::parse_str("check-failure:gym/echo/c:GET /a%2Fb returned 404") {
            EvidenceRef::BenchmarkCheckFailure { detail, .. } => {
                assert_eq!(detail, "GET /a/b returned 404");
            }
            other => panic!("expected check failure, got {other:?}"),
        }
    }
}
