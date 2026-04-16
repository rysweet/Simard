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
            .filter_map(Result::ok)
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
            .filter_map(Result::ok)
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

#[cfg(test)]
mod tests;
