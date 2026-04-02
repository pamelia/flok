//! Schema migrations for the flok database.
//!
//! Migrations are applied in order. Each migration is idempotent (uses
//! `IF NOT EXISTS`). The current schema version is tracked in a
//! `schema_version` pragma.

use rusqlite::Connection;

use crate::schema::DbError;

/// All migrations in order. Each entry is `(version, description, sql)`.
const MIGRATIONS: &[(i32, &str, &str)] = &[
    (
        1,
        "Initial schema: projects, sessions, messages",
        r"
        CREATE TABLE IF NOT EXISTS projects (
            id          TEXT PRIMARY KEY,
            path        TEXT NOT NULL UNIQUE,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS sessions (
            id              TEXT PRIMARY KEY,
            project_id      TEXT NOT NULL REFERENCES projects(id),
            parent_id       TEXT REFERENCES sessions(id),
            title           TEXT NOT NULL DEFAULT '',
            model_id        TEXT NOT NULL DEFAULT '',
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_project
            ON sessions(project_id, updated_at DESC);

        CREATE TABLE IF NOT EXISTS messages (
            id          TEXT PRIMARY KEY,
            session_id  TEXT NOT NULL REFERENCES sessions(id),
            role        TEXT NOT NULL CHECK(role IN ('system', 'user', 'assistant')),
            parts       TEXT NOT NULL DEFAULT '[]',
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_messages_session
            ON messages(session_id, created_at ASC);
        ",
    ),
    (
        2,
        "Permission rules: per-project persistent always-allow decisions",
        r"
        CREATE TABLE IF NOT EXISTS permission_rules (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id  TEXT NOT NULL REFERENCES projects(id),
            permission  TEXT NOT NULL,
            pattern     TEXT NOT NULL,
            action      TEXT NOT NULL CHECK(action IN ('allow', 'deny', 'ask')),
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(project_id, permission, pattern)
        );

        CREATE INDEX IF NOT EXISTS idx_perm_rules_project
            ON permission_rules(project_id);
        ",
    ),
    (
        3,
        "Tree sessions: branch metadata and session labels",
        r"
        ALTER TABLE sessions ADD COLUMN branch_from_message_id TEXT;
        ALTER TABLE sessions ADD COLUMN branch_snapshot_hash TEXT;

        CREATE TABLE IF NOT EXISTS session_labels (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  TEXT NOT NULL REFERENCES sessions(id),
            label       TEXT NOT NULL,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(session_id)
        );

        CREATE INDEX IF NOT EXISTS idx_session_labels_session
            ON session_labels(session_id);
        ",
    ),
];

/// Run all pending migrations.
///
/// # Errors
///
/// Returns `DbError::Migration` if a migration fails.
pub(crate) fn run(conn: &Connection) -> Result<(), DbError> {
    let current_version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

    for &(version, description, sql) in MIGRATIONS {
        if version > current_version {
            tracing::debug!(version, description, "applying migration");
            conn.execute_batch(sql)
                .map_err(|e| DbError::Migration(format!("v{version} ({description}): {e}")))?;
            conn.pragma_update(None, "user_version", version)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();

        // Run twice — should not error
        run(&conn).unwrap();
        run(&conn).unwrap();

        let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        assert_eq!(version, 3);
    }
}
