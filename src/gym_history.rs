//! Persistent score history, regression detection, and promotion logic for gym benchmarks.
//!
//! Backs score records with SQLite (via `rusqlite`) so the OODA loop can detect
//! regressions and promotions across runs without in-memory state.

use crate::error::{SimardError, SimardResult};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A single recorded benchmark score.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoreRecord {
    pub suite_id: String,
    pub scenario_id: String,
    pub score: f64,
    /// Unix-epoch seconds.
    pub timestamp: i64,
    pub commit_hash: Option<String>,
}

/// Signal emitted by comparing recent score history for a scenario.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GymSignal {
    Improvement { delta: f64 },
    Regression { delta: f64 },
    Stable,
    Promoted,
}

impl std::fmt::Display for GymSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Improvement { delta } => write!(f, "improvement(+{delta:.4})"),
            Self::Regression { delta } => write!(f, "regression({delta:.4})"),
            Self::Stable => f.write_str("stable"),
            Self::Promoted => f.write_str("promoted"),
        }
    }
}

/// Signal paired with the scenario that produced it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScenarioSignal {
    pub scenario_id: String,
    pub signal: GymSignal,
}

// ── Pure functions ───────────────────────────────────────────────────

/// Returns `true` when `current` has regressed from `previous` by more than
/// `threshold` (absolute drop).
pub fn detect_regression(current: f64, previous: f64, threshold: f64) -> bool {
    previous - current > threshold
}

/// Returns `true` when the last `consecutive_improvements` scores in `history`
/// show strictly increasing values. Requires at least `consecutive_improvements + 1`
/// records so that `N` deltas can be computed.
pub fn check_promotion(history: &[ScoreRecord], consecutive_improvements: usize) -> bool {
    if consecutive_improvements == 0 {
        return false;
    }
    let needed = consecutive_improvements + 1;
    if history.len() < needed {
        return false;
    }
    let tail = &history[history.len() - needed..];
    tail.windows(2).all(|w| w[1].score > w[0].score)
}

/// Persistent score store backed by a single SQLite database.
pub struct ScoreHistory {
    conn: Connection,
}

impl ScoreHistory {
    /// Open (or create) the score-history database at `db_path`.
    pub fn open<P: AsRef<Path>>(db_path: P) -> SimardResult<Self> {
        let conn = Connection::open(db_path).map_err(|e| SimardError::GymHistoryDb {
            action: "open".into(),
            reason: e.to_string(),
        })?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS score_records (
                 id          INTEGER PRIMARY KEY AUTOINCREMENT,
                 suite_id    TEXT    NOT NULL,
                 scenario_id TEXT    NOT NULL,
                 score       REAL    NOT NULL,
                 timestamp   INTEGER NOT NULL,
                 commit_hash TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_suite_scenario
                 ON score_records (suite_id, scenario_id, timestamp DESC);",
        )
        .map_err(|e| SimardError::GymHistoryDb {
            action: "initialize_schema".into(),
            reason: e.to_string(),
        })?;
        Ok(Self { conn })
    }

    /// Persist a new score record.
    pub fn record(&self, rec: &ScoreRecord) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO score_records (suite_id, scenario_id, score, timestamp, commit_hash)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                rec.suite_id,
                rec.scenario_id,
                rec.score,
                rec.timestamp,
                rec.commit_hash,
            ],
        )?;
        Ok(())
    }

    /// Return the most-recent record for a (suite, scenario) pair, if any.
    pub fn latest(&self, suite_id: &str, scenario_id: &str) -> Option<ScoreRecord> {
        self.conn
            .query_row(
                "SELECT suite_id, scenario_id, score, timestamp, commit_hash
                 FROM score_records
                 WHERE suite_id = ?1 AND scenario_id = ?2
                 ORDER BY timestamp DESC
                 LIMIT 1",
                params![suite_id, scenario_id],
                |row| {
                    Ok(ScoreRecord {
                        suite_id: row.get(0)?,
                        scenario_id: row.get(1)?,
                        score: row.get(2)?,
                        timestamp: row.get(3)?,
                        commit_hash: row.get(4)?,
                    })
                },
            )
            .ok()
    }

    /// Return the last `limit` records for a (suite, scenario) pair, ordered
    /// oldest-first (ascending timestamp).
    pub fn history(
        &self,
        suite_id: &str,
        scenario_id: &str,
        limit: usize,
    ) -> SimardResult<Vec<ScoreRecord>> {
        // Sub-select the most recent N, then flip to ascending order.
        let mut stmt = self
            .conn
            .prepare(
                "SELECT suite_id, scenario_id, score, timestamp, commit_hash
                 FROM (
                     SELECT suite_id, scenario_id, score, timestamp, commit_hash
                     FROM score_records
                     WHERE suite_id = ?1 AND scenario_id = ?2
                     ORDER BY timestamp DESC
                     LIMIT ?3
                 ) sub
                 ORDER BY timestamp ASC",
            )
            .map_err(|e| SimardError::GymHistoryDb {
                action: "prepare_history".into(),
                reason: e.to_string(),
            })?;

        let rows = stmt
            .query_map(params![suite_id, scenario_id, limit as i64], |row| {
                Ok(ScoreRecord {
                    suite_id: row.get(0)?,
                    scenario_id: row.get(1)?,
                    score: row.get(2)?,
                    timestamp: row.get(3)?,
                    commit_hash: row.get(4)?,
                })
            })
            .map_err(|e| SimardError::GymHistoryDb {
                action: "query_history".into(),
                reason: e.to_string(),
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// List distinct scenario IDs that have at least one record for `suite_id`.
    pub fn scenario_ids(&self, suite_id: &str) -> SimardResult<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT scenario_id FROM score_records WHERE suite_id = ?1 ORDER BY scenario_id",
            )
            .map_err(|e| SimardError::GymHistoryDb {
                action: "prepare_scenario_ids".into(),
                reason: e.to_string(),
            })?;

        let rows = stmt
            .query_map(params![suite_id], |row| row.get(0))
            .map_err(|e| SimardError::GymHistoryDb {
                action: "query_scenario_ids".into(),
                reason: e.to_string(),
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }
}

// ── Signal generation ────────────────────────────────────────────────

const REGRESSION_THRESHOLD: f64 = 0.01;
const PROMOTION_STREAK: usize = 3;

/// Produce a signal for each scenario in the given suite based on recent history.
pub fn generate_signals(
    history: &ScoreHistory,
    suite_id: &str,
) -> SimardResult<Vec<ScenarioSignal>> {
    let scenario_ids = history.scenario_ids(suite_id)?;
    let signals = scenario_ids
        .into_iter()
        .filter_map(|sid| {
            let records = history.history(suite_id, &sid, PROMOTION_STREAK + 1).ok()?;
            if records.len() < 2 {
                return None;
            }
            let current = records.last()?.score;
            let previous = records[records.len() - 2].score;

            let signal = if check_promotion(&records, PROMOTION_STREAK) {
                GymSignal::Promoted
            } else if detect_regression(current, previous, REGRESSION_THRESHOLD) {
                GymSignal::Regression {
                    delta: current - previous,
                }
            } else if current - previous > REGRESSION_THRESHOLD {
                GymSignal::Improvement {
                    delta: current - previous,
                }
            } else {
                GymSignal::Stable
            };

            Some(ScenarioSignal {
                scenario_id: sid,
                signal,
            })
        })
        .collect();
    Ok(signals)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(scenario: &str, score: f64, ts: i64) -> ScoreRecord {
        ScoreRecord {
            suite_id: "progressive".into(),
            scenario_id: scenario.into(),
            score,
            timestamp: ts,
            commit_hash: None,
        }
    }

    fn mem_history() -> ScoreHistory {
        ScoreHistory::open(":memory:").unwrap()
    }

    #[test]
    fn score_record_construction() {
        let r = ScoreRecord {
            suite_id: "s1".into(),
            scenario_id: "sc1".into(),
            score: 0.85,
            timestamp: 1_700_000_000,
            commit_hash: Some("abc123".into()),
        };
        assert_eq!(r.suite_id, "s1");
        assert_eq!(r.score, 0.85);
        assert_eq!(r.commit_hash.as_deref(), Some("abc123"));
    }

    #[test]
    fn open_creates_schema() {
        let _h = mem_history();
    }

    #[test]
    fn record_and_latest() {
        let h = mem_history();
        h.record(&rec("L1", 0.9, 100)).unwrap();
        let got = h.latest("progressive", "L1").unwrap();
        assert_eq!(got.score, 0.9);
        assert_eq!(got.timestamp, 100);
    }

    #[test]
    fn latest_returns_newest() {
        let h = mem_history();
        h.record(&rec("L1", 0.5, 1)).unwrap();
        h.record(&rec("L1", 0.9, 2)).unwrap();
        assert_eq!(h.latest("progressive", "L1").unwrap().score, 0.9);
    }

    #[test]
    fn latest_missing() {
        let h = mem_history();
        assert!(h.latest("progressive", "nonexistent").is_none());
    }

    #[test]
    fn history_order_and_limit() {
        let h = mem_history();
        for i in 1..=5 {
            h.record(&rec("L1", i as f64 * 0.1, i)).unwrap();
        }
        let rows = h.history("progressive", "L1", 3).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].timestamp < rows[1].timestamp);
        assert!(rows[1].timestamp < rows[2].timestamp);
        assert_eq!(rows[2].score, 0.5);
    }

    #[test]
    fn detect_regression_cases() {
        assert!(detect_regression(0.5, 0.8, 0.1));
        assert!(!detect_regression(0.8, 0.5, 0.1));
        assert!(!detect_regression(0.79, 0.8, 0.1));
        assert!(detect_regression(0.69, 0.8, 0.1));
        // Exact threshold boundary: drop of exactly 0.25, threshold 0.25 → not >
        assert!(!detect_regression(0.75, 1.0, 0.25));
    }

    #[test]
    fn check_promotion_consecutive() {
        let recs: Vec<ScoreRecord> = (1..=4)
            .map(|i| rec("L1", 0.5 + i as f64 * 0.1, i))
            .collect();
        assert!(check_promotion(&recs, 3));
    }

    #[test]
    fn check_promotion_broken_streak() {
        let recs = vec![
            rec("L1", 0.5, 1),
            rec("L1", 0.6, 2),
            rec("L1", 0.55, 3), // regression breaks streak
            rec("L1", 0.7, 4),
        ];
        assert!(!check_promotion(&recs, 3));
    }

    #[test]
    fn check_promotion_insufficient_history() {
        let recs = vec![rec("L1", 0.5, 1), rec("L1", 0.6, 2)];
        assert!(!check_promotion(&recs, 3));
    }

    #[test]
    fn check_promotion_zero_consecutive() {
        let recs = vec![rec("L1", 0.5, 1)];
        assert!(!check_promotion(&recs, 0));
    }

    #[test]
    fn generate_signals_types() {
        let h = mem_history();
        // Scenario A: improving
        h.record(&rec("A", 0.5, 1)).unwrap();
        h.record(&rec("A", 0.7, 2)).unwrap();

        // Scenario B: regressing
        h.record(&rec("B", 0.8, 1)).unwrap();
        h.record(&rec("B", 0.5, 2)).unwrap();

        // Scenario C: stable
        h.record(&rec("C", 0.8, 1)).unwrap();
        h.record(&rec("C", 0.805, 2)).unwrap();

        let sigs = generate_signals(&h, "progressive").unwrap();
        assert_eq!(sigs.len(), 3);

        let find = |id: &str| sigs.iter().find(|s| s.scenario_id == id).unwrap();
        assert!(matches!(find("A").signal, GymSignal::Improvement { .. }));
        assert!(matches!(find("B").signal, GymSignal::Regression { .. }));
        assert!(matches!(find("C").signal, GymSignal::Stable));
    }

    #[test]
    fn generate_signals_promoted() {
        let h = mem_history();
        for i in 1..=5 {
            h.record(&rec("P", 0.5 + i as f64 * 0.05, i)).unwrap();
        }
        let sigs = generate_signals(&h, "progressive").unwrap();
        let sig = sigs.iter().find(|s| s.scenario_id == "P").unwrap();
        assert_eq!(sig.signal, GymSignal::Promoted);
    }

    #[test]
    fn scenario_ids_listing() {
        let h = mem_history();
        h.record(&rec("X", 0.1, 1)).unwrap();
        h.record(&rec("Y", 0.2, 1)).unwrap();
        h.record(&rec("X", 0.3, 2)).unwrap();
        let ids = h.scenario_ids("progressive").unwrap();
        assert_eq!(ids, vec!["X", "Y"]);
    }

    #[test]
    fn gym_signal_display() {
        assert_eq!(format!("{}", GymSignal::Stable), "stable");
        assert_eq!(format!("{}", GymSignal::Promoted), "promoted");
        assert!(format!("{}", GymSignal::Improvement { delta: 0.1 }).starts_with("improvement"));
    }

    #[test]
    fn generate_signals_skips_single_record() {
        let h = mem_history();
        h.record(&rec("solo", 0.5, 1)).unwrap();
        let sigs = generate_signals(&h, "progressive").unwrap();
        assert!(sigs.is_empty());
    }
}
