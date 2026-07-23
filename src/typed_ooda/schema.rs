use std::time::Duration;

use rusqlite::{Connection, TransactionBehavior, params};

const SCHEMA_VERSION: i64 = 1;

/// Baseline SQLite `busy_timeout` applied to every ledger connection.
///
/// 5s matches the value the live incident exhausted before the WAL +
/// process-wide writer-lock layers were added. Kept as a named constant so the
/// contract is grep-able and single-sourced; once WAL and the writer lock
/// remove the contention that produced `SQLITE_BUSY`, this is a defensive
/// backstop rather than the primary mechanism.
pub(super) const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);

/// Apply the connection-level configuration required to avoid the systemic
/// `database is locked` crash-loop (issue #4483).
///
/// This MUST run on EVERY [`Connection`] open, unconditionally and BEFORE
/// [`initialize`]. [`initialize`] early-returns once
/// `PRAGMA user_version == SCHEMA_VERSION`, so any pragma set only inside it is
/// never re-applied to an already-initialized ledger — which is every ledger
/// after the first run. The live incident's ledgers were therefore left in the
/// default rollback-journal mode (whole-file EXCLUSIVE write locks) with no
/// `busy_timeout` and `synchronous=FULL`.
///
/// The configuration:
/// - `busy_timeout` — wait instead of failing immediately on a momentarily held
///   lock.
/// - `journal_mode = WAL` — readers and a single writer proceed concurrently
///   instead of contending on a whole-file exclusive lock. WAL is a persistent
///   file property, so applying it here also upgrades legacy delete-mode
///   ledgers on open.
/// - `synchronous = NORMAL` — durable under WAL, far cheaper than FULL.
/// - `foreign_keys = ON` — preserve referential integrity (mirrors
///   [`initialize`], which also asserts it).
///
/// Idempotent: re-asserting WAL on an already-WAL connection is a cheap no-op,
/// so calling this on every open is safe.
pub(super) fn configure_connection(connection: &Connection) -> rusqlite::Result<()> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;\n\
         PRAGMA synchronous = NORMAL;\n\
         PRAGMA foreign_keys = ON;",
    )?;
    Ok(())
}

pub(super) fn initialize(connection: &mut Connection, now_millis: i64) -> rusqlite::Result<()> {
    connection.execute_batch("PRAGMA foreign_keys = ON;")?;
    let version = schema_version(connection)?;
    if version == SCHEMA_VERSION {
        return Ok(());
    }
    if version > SCHEMA_VERSION {
        return Err(rusqlite::Error::InvalidQuery);
    }

    connection.execute_batch("PRAGMA journal_mode = WAL;")?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version = schema_version(&transaction)?;
    if version == SCHEMA_VERSION {
        transaction.commit()?;
        return Ok(());
    }
    if version > SCHEMA_VERSION {
        return Err(rusqlite::Error::InvalidQuery);
    }

    transaction.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS terminal_outcomes (
            request_id TEXT PRIMARY KEY,
            request_hash TEXT NOT NULL,
            session_id TEXT NOT NULL,
            cycle_id TEXT NOT NULL,
            outcome_id TEXT NOT NULL UNIQUE,
            outcome_json BLOB NOT NULL,
            UNIQUE(session_id, cycle_id)
        );
        CREATE TABLE IF NOT EXISTS progress_records (
            request_id TEXT PRIMARY KEY,
            request_hash TEXT NOT NULL,
            session_id TEXT NOT NULL,
            cycle_id TEXT NOT NULL,
            progress_json BLOB NOT NULL
        );
        CREATE TABLE IF NOT EXISTS mutation_requests (
            request_id TEXT PRIMARY KEY,
            mutation_type TEXT NOT NULL,
            request_hash TEXT NOT NULL,
            result_json BLOB NOT NULL,
            request_format_version INTEGER NOT NULL DEFAULT 2,
            result_format_version INTEGER NOT NULL DEFAULT 1
        );
        CREATE TABLE IF NOT EXISTS effect_jobs (
            effect_id TEXT PRIMARY KEY,
            outcome_id TEXT NOT NULL UNIQUE,
            request_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            state TEXT NOT NULL,
            action_json BLOB NOT NULL,
            attempt INTEGER NOT NULL DEFAULT 0,
            lease_generation INTEGER NOT NULL DEFAULT 0,
            lease_owner TEXT,
            lease_expires_at INTEGER,
            error TEXT,
            result_json BLOB,
            FOREIGN KEY(outcome_id) REFERENCES terminal_outcomes(outcome_id)
        );
        CREATE TABLE IF NOT EXISTS engineer_claims (
            claim_key TEXT PRIMARY KEY,
            outcome_id TEXT NOT NULL UNIQUE,
            request_id TEXT NOT NULL,
            FOREIGN KEY(outcome_id) REFERENCES terminal_outcomes(outcome_id)
        );
        CREATE TABLE IF NOT EXISTS actor_sessions (
            session_id TEXT PRIMARY KEY,
            cycle_id TEXT NOT NULL,
            goal_id TEXT NOT NULL,
            actor_identity TEXT NOT NULL,
            repository_json BLOB NOT NULL,
            grants_json BLOB NOT NULL,
            engineer_permissions_json BLOB NOT NULL DEFAULT X'5b5d',
            working_directory_json BLOB,
            observe_only INTEGER NOT NULL,
            token_hash TEXT NOT NULL,
            expires_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS process_executions (
            execution_id TEXT PRIMARY KEY,
            request_id TEXT NOT NULL UNIQUE,
            request_hash TEXT NOT NULL,
            session_id TEXT NOT NULL,
            cycle_id TEXT NOT NULL,
            goal_id TEXT NOT NULL,
            status TEXT NOT NULL,
            request_json BLOB NOT NULL,
            result_json BLOB NOT NULL
        );
        CREATE INDEX IF NOT EXISTS process_executions_cycle_idx
            ON process_executions(session_id, cycle_id);
        CREATE TABLE IF NOT EXISTS mutation_scope_counters (
            session_id TEXT NOT NULL,
            cycle_id TEXT NOT NULL,
            goal_id TEXT NOT NULL,
            mutation_type TEXT NOT NULL,
            spent INTEGER NOT NULL,
            PRIMARY KEY(session_id, cycle_id, goal_id, mutation_type)
        );
        CREATE TABLE IF NOT EXISTS authorization_decisions (
            decision_id TEXT PRIMARY KEY,
            effect_id TEXT NOT NULL,
            decision TEXT NOT NULL,
            decision_json BLOB NOT NULL,
            recorded_at INTEGER NOT NULL,
            FOREIGN KEY(effect_id) REFERENCES effect_jobs(effect_id)
        );
        CREATE INDEX IF NOT EXISTS progress_records_cycle_idx
            ON progress_records(session_id, cycle_id);
        CREATE INDEX IF NOT EXISTS effect_jobs_state_lease_idx
            ON effect_jobs(state, lease_expires_at);
        CREATE INDEX IF NOT EXISTS authorization_decisions_effect_idx
            ON authorization_decisions(effect_id, recorded_at);
        ",
    )?;

    ensure_column(
        &transaction,
        "effect_jobs",
        "result_json",
        "ALTER TABLE effect_jobs ADD COLUMN result_json BLOB",
    )?;
    ensure_column(
        &transaction,
        "effect_jobs",
        "lease_generation",
        "ALTER TABLE effect_jobs ADD COLUMN lease_generation INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &transaction,
        "actor_sessions",
        "engineer_permissions_json",
        "ALTER TABLE actor_sessions ADD COLUMN engineer_permissions_json BLOB NOT NULL DEFAULT X'5b5d'",
    )?;
    ensure_column(
        &transaction,
        "actor_sessions",
        "working_directory_json",
        "ALTER TABLE actor_sessions ADD COLUMN working_directory_json BLOB",
    )?;
    ensure_column(
        &transaction,
        "mutation_requests",
        "request_format_version",
        "ALTER TABLE mutation_requests ADD COLUMN request_format_version INTEGER NOT NULL DEFAULT 1",
    )?;
    ensure_column(
        &transaction,
        "mutation_requests",
        "result_format_version",
        "ALTER TABLE mutation_requests ADD COLUMN result_format_version INTEGER NOT NULL DEFAULT 1",
    )?;

    transaction.execute_batch(
        "
        INSERT OR IGNORE INTO mutation_requests(
            request_id, mutation_type, request_hash, result_json,
            request_format_version, result_format_version
        )
            SELECT request_id, 'terminal', request_hash, outcome_json, 1, 1
            FROM terminal_outcomes;
        INSERT OR IGNORE INTO mutation_requests(
            request_id, mutation_type, request_hash, result_json,
            request_format_version, result_format_version
        )
            SELECT request_id, 'progress', request_hash, progress_json, 1, 1
            FROM progress_records;
        ",
    )?;
    transaction.execute(
        "DELETE FROM actor_sessions WHERE expires_at < ?1",
        [now_millis],
    )?;
    transaction.execute_batch(
        "
        DELETE FROM actor_sessions
        WHERE session_id IN (
            SELECT session_id FROM actor_sessions
            GROUP BY session_id HAVING COUNT(*) > 1
        );
        CREATE UNIQUE INDEX IF NOT EXISTS actor_sessions_session_idx
            ON actor_sessions(session_id);
        INSERT INTO mutation_scope_counters(
            session_id, cycle_id, goal_id, mutation_type, spent
        )
        SELECT session_id, cycle_id, goal_id, 'process_exec', COUNT(*)
        FROM process_executions
        GROUP BY session_id, cycle_id, goal_id
        ON CONFLICT(session_id, cycle_id, goal_id, mutation_type)
        DO UPDATE SET spent=MAX(spent, excluded.spent);
        ",
    )?;
    transaction.execute_batch("PRAGMA user_version = 1")?;
    transaction.commit()
}

fn schema_version(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row("PRAGMA user_version", [], |row| row.get(0))
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    migration: &str,
) -> rusqlite::Result<()> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM pragma_table_info(?1) WHERE name=?2
        )",
        params![table, column],
        |row| row.get(0),
    )?;
    if !exists {
        connection.execute(migration, [])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// TDD regression suite for the systemic `database is locked` crash-loop
// (issue #4483).
//
// Root cause layer 1: SQLite pragmas were applied only inside `initialize`,
// which early-returns once `PRAGMA user_version == SCHEMA_VERSION`. On an
// already-initialized ledger (every run after the first) the connection was
// therefore left in the default rollback-journal mode (`delete`) with
// `synchronous=FULL` and NO `busy_timeout`. Rollback-journal mode takes a
// whole-file EXCLUSIVE write lock, so the outbox startup-recovery writer and
// the concurrent per-goal cycle writers collided and failed with
// `SQLITE_BUSY` -> "database is locked".
//
// The fix introduces `configure_connection`, applied UNCONDITIONALLY on every
// `Connection::open` (outside the version gate). These tests pin its contract
// and are written against the intended (not-yet-existing) API surface, so this
// module is compile-red until Step 8 lands `configure_connection` and
// `BUSY_TIMEOUT`.
//
// NOTE on external verifiability: `journal_mode = WAL` is a persistent file
// property and can be re-read from any fresh connection to the file.
// `synchronous` and `busy_timeout` are PER-CONNECTION and are NOT stored in
// the database file, so they can only be asserted on the very connection that
// `configure_connection` was applied to (as done here) — never via an external
// `sqlite3` CLI opening its own connection.
#[cfg(test)]
mod configure_connection_tests {
    use super::*;

    fn open_file_connection() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().expect("tempdir");
        // WAL requires a real on-disk database; an in-memory DB cannot be WAL.
        let connection = Connection::open(dir.path().join("outcomes.sqlite3"))
            .expect("open on-disk sqlite database");
        (dir, connection)
    }

    fn journal_mode(connection: &Connection) -> String {
        connection
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .expect("read journal_mode")
    }

    fn synchronous(connection: &Connection) -> i64 {
        connection
            .query_row("PRAGMA synchronous", [], |row| row.get::<_, i64>(0))
            .expect("read synchronous")
    }

    fn busy_timeout_millis(connection: &Connection) -> i64 {
        connection
            .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .expect("read busy_timeout")
    }

    /// The named baseline `busy_timeout` is 5s, matching the value the incident
    /// exhausted before the WAL + writer-lock layers were added. Kept as a
    /// named constant so the contract is grep-able and single-sourced.
    #[test]
    fn busy_timeout_constant_is_five_seconds() {
        assert_eq!(BUSY_TIMEOUT, std::time::Duration::from_millis(5000));
    }

    /// Core contract: a single `configure_connection` call flips the connection
    /// into WAL journal mode, sets `synchronous=NORMAL` (1), applies the
    /// `busy_timeout`, and leaves `foreign_keys` enforced. Before the fix none
    /// of these held on an already-initialized DB.
    #[test]
    fn configure_connection_sets_wal_normal_busy_timeout_and_foreign_keys() {
        let (_dir, connection) = open_file_connection();

        configure_connection(&connection).expect("configure_connection must succeed");

        assert_eq!(
            journal_mode(&connection),
            "wal",
            "journal_mode must be WAL so readers and one writer proceed concurrently"
        );
        assert_eq!(
            synchronous(&connection),
            1,
            "synchronous must be NORMAL (1) — durable enough under WAL, far cheaper than FULL"
        );
        assert_eq!(
            busy_timeout_millis(&connection),
            BUSY_TIMEOUT.as_millis() as i64,
            "busy_timeout must be applied on every connection, not only in open()"
        );
        let foreign_keys: i64 = connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("read foreign_keys");
        assert_eq!(foreign_keys, 1, "foreign_keys must remain enforced");
    }

    /// `configure_connection` must be idempotent: re-asserting WAL on an
    /// already-WAL connection is a cheap no-op, never an error. Every
    /// `CapabilityHandler::open` calls it, so it runs many times per file.
    #[test]
    fn configure_connection_is_idempotent() {
        let (_dir, connection) = open_file_connection();

        configure_connection(&connection).expect("first configure");
        configure_connection(&connection).expect("second configure must be a no-op success");

        assert_eq!(journal_mode(&connection), "wal");
        assert_eq!(synchronous(&connection), 1);
    }

    /// The legacy-upgrade path: a ledger created by a PRE-WAL build persists
    /// `journal_mode=delete` on disk and has `user_version == SCHEMA_VERSION`,
    /// so `initialize` early-returns and never touches the journal mode.
    /// `configure_connection`, applied on EVERY open, must still upgrade the
    /// file to WAL. This is the exact shape of the already-initialized ledgers
    /// in the live incident.
    #[test]
    fn already_initialized_delete_mode_ledger_is_upgraded_to_wal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("outcomes.sqlite3");

        // Simulate a pre-fix ledger: initialize, then force rollback-journal
        // mode as a pre-WAL build would have left it.
        {
            let mut connection = Connection::open(&path).expect("open legacy ledger");
            initialize(&mut connection, 0).expect("initialize legacy ledger");
            connection
                .pragma_update(None, "journal_mode", "DELETE")
                .expect("force delete journal mode");
            assert_eq!(
                journal_mode(&connection),
                "delete",
                "precondition: legacy ledger is in rollback-journal mode"
            );
        }

        // Reopen exactly as CapabilityHandler::open will: configure BEFORE
        // initialize. `initialize` will early-return (user_version already 1),
        // so only `configure_connection` can perform the WAL upgrade.
        let mut connection = Connection::open(&path).expect("reopen legacy ledger");
        configure_connection(&connection).expect("configure on reopen");
        initialize(&mut connection, 0).expect("initialize is a no-op on reopen");

        assert_eq!(
            journal_mode(&connection),
            "wal",
            "an already-initialized delete-mode ledger MUST be upgraded to WAL on open"
        );

        // And the upgrade is file-persistent: a brand-new connection sees WAL.
        let fresh = Connection::open(&path).expect("fresh connection to upgraded ledger");
        assert_eq!(
            journal_mode(&fresh),
            "wal",
            "WAL is a persistent file property visible to every subsequent connection"
        );
    }
}
