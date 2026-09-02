use rusqlite::{Connection, TransactionBehavior, params};

const SCHEMA_VERSION: i64 = 1;

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
