//! Database query operations for projects, sessions, and messages.

use rusqlite::params;

use crate::models::{MessageRow, Project, Session};
use crate::schema::{Db, DbError};

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

impl Db {
    /// Get or create a project for the given path.
    ///
    /// If a project with this path already exists, returns it.
    /// Otherwise, creates a new one with the given ID.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn get_or_create_project(&self, id: &str, path: &str) -> Result<Project, DbError> {
        // Try to find existing
        let existing = self.conn().query_row(
            "SELECT id, path, created_at FROM projects WHERE path = ?1",
            params![path],
            |row| Ok(Project { id: row.get(0)?, path: row.get(1)?, created_at: row.get(2)? }),
        );

        match existing {
            Ok(project) => Ok(project),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                self.conn().execute(
                    "INSERT INTO projects (id, path) VALUES (?1, ?2)",
                    params![id, path],
                )?;
                self.get_project(id)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Get a project by ID.
    ///
    /// # Errors
    ///
    /// Returns `DbError::NotFound` if the project doesn't exist.
    pub fn get_project(&self, id: &str) -> Result<Project, DbError> {
        self.conn()
            .query_row(
                "SELECT id, path, created_at FROM projects WHERE id = ?1",
                params![id],
                |row| Ok(Project { id: row.get(0)?, path: row.get(1)?, created_at: row.get(2)? }),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => DbError::NotFound(format!("project {id}")),
                other => other.into(),
            })
    }
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

impl Db {
    /// Create a new session.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn create_session(
        &self,
        id: &str,
        project_id: &str,
        model_id: &str,
    ) -> Result<Session, DbError> {
        self.conn().execute(
            "INSERT INTO sessions (id, project_id, model_id) VALUES (?1, ?2, ?3)",
            params![id, project_id, model_id],
        )?;
        self.get_session(id)
    }

    /// Get a session by ID.
    ///
    /// # Errors
    ///
    /// Returns `DbError::NotFound` if the session doesn't exist.
    pub fn get_session(&self, id: &str) -> Result<Session, DbError> {
        self.conn()
            .query_row(
                "SELECT id, project_id, parent_id, title, model_id, created_at, updated_at \
                 FROM sessions WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Session {
                        id: row.get(0)?,
                        project_id: row.get(1)?,
                        parent_id: row.get(2)?,
                        title: row.get(3)?,
                        model_id: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => DbError::NotFound(format!("session {id}")),
                other => other.into(),
            })
    }

    /// List sessions for a project, ordered by most recently updated.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn list_sessions(&self, project_id: &str) -> Result<Vec<Session>, DbError> {
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, parent_id, title, model_id, created_at, updated_at \
             FROM sessions WHERE project_id = ?1 ORDER BY updated_at DESC",
        )?;

        let sessions = stmt
            .query_map(params![project_id], |row| {
                Ok(Session {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    parent_id: row.get(2)?,
                    title: row.get(3)?,
                    model_id: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(sessions)
    }

    /// Update a session's title.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn update_session_title(&self, id: &str, title: &str) -> Result<(), DbError> {
        self.conn().execute(
            "UPDATE sessions SET title = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![title, id],
        )?;
        Ok(())
    }

    /// Touch a session's `updated_at` timestamp.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn touch_session(&self, id: &str) -> Result<(), DbError> {
        self.conn().execute(
            "UPDATE sessions SET updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

impl Db {
    /// Insert a message into a session.
    ///
    /// The `parts` parameter should be a JSON-serialized `Vec<MessagePart>`.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn insert_message(
        &self,
        id: &str,
        session_id: &str,
        role: &str,
        parts_json: &str,
    ) -> Result<MessageRow, DbError> {
        self.conn().execute(
            "INSERT INTO messages (id, session_id, role, parts) VALUES (?1, ?2, ?3, ?4)",
            params![id, session_id, role, parts_json],
        )?;
        self.get_message(id)
    }

    /// Get a message by ID.
    ///
    /// # Errors
    ///
    /// Returns `DbError::NotFound` if the message doesn't exist.
    pub fn get_message(&self, id: &str) -> Result<MessageRow, DbError> {
        self.conn()
            .query_row(
                "SELECT id, session_id, role, parts, created_at FROM messages WHERE id = ?1",
                params![id],
                |row| {
                    Ok(MessageRow {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        role: row.get(2)?,
                        parts: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => DbError::NotFound(format!("message {id}")),
                other => other.into(),
            })
    }

    /// List all messages in a session, ordered by creation time.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn list_messages(&self, session_id: &str) -> Result<Vec<MessageRow>, DbError> {
        let mut stmt = self.conn().prepare(
            "SELECT id, session_id, role, parts, created_at \
             FROM messages WHERE session_id = ?1 ORDER BY created_at ASC",
        )?;

        let messages = stmt
            .query_map(params![session_id], |row| {
                Ok(MessageRow {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    role: row.get(2)?,
                    parts: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(messages)
    }

    /// Update a message's parts JSON. Used when streaming appends to a message.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn update_message_parts(&self, id: &str, parts_json: &str) -> Result<(), DbError> {
        self.conn()
            .execute("UPDATE messages SET parts = ?1 WHERE id = ?2", params![parts_json, id])?;
        Ok(())
    }

    /// Delete all messages in a session that were created at or after a given
    /// message's `created_at` timestamp.
    ///
    /// This is used by undo to roll back conversation history. The message
    /// identified by `from_id` is also deleted.
    ///
    /// Returns the number of messages deleted.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn delete_messages_from(&self, session_id: &str, from_id: &str) -> Result<usize, DbError> {
        // Get the rowid of the target message (monotonically increasing)
        let target_rowid: i64 = self
            .conn()
            .query_row(
                "SELECT rowid FROM messages WHERE id = ?1 AND session_id = ?2",
                params![from_id, session_id],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    DbError::NotFound(format!("message {from_id}"))
                }
                other => other.into(),
            })?;

        // Delete this message and all messages inserted after it in this session
        let deleted = self.conn().execute(
            "DELETE FROM messages WHERE session_id = ?1 AND rowid >= ?2",
            params![session_id, target_rowid],
        )?;

        Ok(deleted)
    }

    /// Count messages in a session.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn count_messages(&self, session_id: &str) -> Result<usize, DbError> {
        let count: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Db {
        Db::open_in_memory().unwrap()
    }

    // -- Projects --

    #[test]
    fn create_and_get_project() {
        let db = test_db();
        let project = db.get_or_create_project("p1", "/tmp/test-project").unwrap();
        assert_eq!(project.id, "p1");
        assert_eq!(project.path, "/tmp/test-project");
    }

    #[test]
    fn get_or_create_project_returns_existing() {
        let db = test_db();
        let p1 = db.get_or_create_project("p1", "/tmp/test").unwrap();
        let p2 = db.get_or_create_project("p2", "/tmp/test").unwrap();
        // Same path should return the same project (p1), not create p2
        assert_eq!(p1.id, p2.id);
    }

    #[test]
    fn get_project_not_found() {
        let db = test_db();
        let result = db.get_project("nonexistent");
        assert!(matches!(result, Err(DbError::NotFound(_))));
    }

    // -- Sessions --

    #[test]
    fn create_and_get_session() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        let session = db.create_session("s1", "p1", "claude-sonnet-4").unwrap();
        assert_eq!(session.id, "s1");
        assert_eq!(session.project_id, "p1");
        assert_eq!(session.model_id, "claude-sonnet-4");
    }

    #[test]
    fn list_sessions_returns_most_recent_first() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();
        db.create_session("s2", "p1", "model-b").unwrap();
        // Touch s1 so it becomes the most recent
        db.touch_session("s1").unwrap();

        let sessions = db.list_sessions("p1").unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, "s1"); // Most recently updated
    }

    #[test]
    fn update_session_title() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();
        db.update_session_title("s1", "My Session").unwrap();
        let session = db.get_session("s1").unwrap();
        assert_eq!(session.title, "My Session");
    }

    // -- Messages --

    #[test]
    fn insert_and_list_messages() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();

        db.insert_message("m1", "s1", "user", r#"[{"type":"text","text":"hello"}]"#).unwrap();
        db.insert_message("m2", "s1", "assistant", r#"[{"type":"text","text":"hi there"}]"#)
            .unwrap();

        let messages = db.list_messages("s1").unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].id, "m1");
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].id, "m2");
        assert_eq!(messages[1].role, "assistant");
    }

    #[test]
    fn update_message_parts() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();
        db.insert_message("m1", "s1", "assistant", "[]").unwrap();

        let updated_parts = r#"[{"type":"text","text":"updated content"}]"#;
        db.update_message_parts("m1", updated_parts).unwrap();

        let msg = db.get_message("m1").unwrap();
        assert_eq!(msg.parts, updated_parts);
    }

    #[test]
    fn get_message_not_found() {
        let db = test_db();
        let result = db.get_message("nonexistent");
        assert!(matches!(result, Err(DbError::NotFound(_))));
    }

    #[test]
    fn delete_messages_from_removes_target_and_later() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();

        db.insert_message("m1", "s1", "user", r#"[{"type":"text","text":"first"}]"#).unwrap();
        // Small sleep to ensure different timestamps
        std::thread::sleep(std::time::Duration::from_millis(10));
        db.insert_message("m2", "s1", "assistant", r#"[{"type":"text","text":"second"}]"#).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        db.insert_message("m3", "s1", "user", r#"[{"type":"text","text":"third"}]"#).unwrap();

        // Delete from m2 onward
        let deleted = db.delete_messages_from("s1", "m2").unwrap();
        assert!(deleted >= 2, "should delete m2 and m3, got {deleted}");

        let remaining = db.list_messages("s1").unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "m1");
    }

    #[test]
    fn count_messages_returns_correct_count() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();

        assert_eq!(db.count_messages("s1").unwrap(), 0);
        db.insert_message("m1", "s1", "user", "[]").unwrap();
        assert_eq!(db.count_messages("s1").unwrap(), 1);
        db.insert_message("m2", "s1", "assistant", "[]").unwrap();
        assert_eq!(db.count_messages("s1").unwrap(), 2);
    }
}
