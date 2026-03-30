//! Database schema and connection management.

use std::path::Path;

use rusqlite::Connection;

use crate::migrations;

/// Errors that can occur during database operations.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// An error from the underlying `SQLite` library.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),

    /// A migration failed to apply.
    #[error("migration failed: {0}")]
    Migration(String),

    /// A required record was not found.
    #[error("not found: {0}")]
    NotFound(String),
}

/// The database handle. Wraps a `SQLite` connection with schema management.
#[derive(Debug)]
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open (or create) the database at the given path and run migrations.
    ///
    /// # Errors
    ///
    /// Returns `DbError` if the file cannot be opened or migrations fail.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        let conn = Connection::open(path)?;

        // Enable WAL mode for better concurrent read performance
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;

        let db = Self { conn };
        migrations::run(&db.conn)?;
        Ok(db)
    }

    /// Open an in-memory database (for testing).
    ///
    /// # Errors
    ///
    /// Returns `DbError` if migrations fail.
    pub fn open_in_memory() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let db = Self { conn };
        migrations::run(&db.conn)?;
        Ok(db)
    }

    /// Returns a reference to the underlying connection.
    ///
    /// Used internally for queries and by tests for assertions.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_succeeds() {
        let db = Db::open_in_memory();
        assert!(db.is_ok());
    }

    #[test]
    fn open_in_memory_runs_migrations() {
        let db = Db::open_in_memory().unwrap();
        // Verify the sessions table exists by querying it
        let count: i64 =
            db.conn().query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0);
    }
}
