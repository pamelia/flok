//! Database query operations for projects, sessions, and messages.

use rusqlite::params;

use crate::models::{MessageRow, Project, Session, SessionLabel};
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

/// Map a row from the sessions table to a `Session` struct.
fn map_session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get(0)?,
        project_id: row.get(1)?,
        parent_id: row.get(2)?,
        title: row.get(3)?,
        model_id: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        branch_from_message_id: row.get(7)?,
        branch_snapshot_hash: row.get(8)?,
    })
}

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
                "SELECT id, project_id, parent_id, title, model_id, created_at, updated_at, \
                        branch_from_message_id, branch_snapshot_hash \
                 FROM sessions WHERE id = ?1",
                params![id],
                map_session_row,
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
            "SELECT id, project_id, parent_id, title, model_id, created_at, updated_at, \
                    branch_from_message_id, branch_snapshot_hash \
             FROM sessions WHERE project_id = ?1 ORDER BY updated_at DESC",
        )?;

        let sessions =
            stmt.query_map(params![project_id], map_session_row)?.collect::<Result<Vec<_>, _>>()?;

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

    /// Create a branch session from an existing session at a specific message.
    ///
    /// The new session is linked to the parent via `parent_id` and records
    /// the branch point message and optional snapshot hash.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    #[allow(clippy::too_many_arguments)]
    pub fn create_branch_session(
        &self,
        id: &str,
        project_id: &str,
        parent_id: &str,
        model_id: &str,
        title: &str,
        branch_from_message_id: &str,
        branch_snapshot_hash: Option<&str>,
    ) -> Result<Session, DbError> {
        self.conn().execute(
            "INSERT INTO sessions \
             (id, project_id, parent_id, model_id, title, branch_from_message_id, branch_snapshot_hash) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, project_id, parent_id, model_id, title, branch_from_message_id, branch_snapshot_hash],
        )?;
        self.get_session(id)
    }

    /// Copy messages from one session to another, up to and including the
    /// given message ID. Messages are inserted with new IDs but preserve
    /// role, parts, and relative ordering.
    ///
    /// Returns the number of messages copied.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures or if `up_to_message_id`
    /// is not found in the source session.
    pub fn copy_messages_to_session(
        &self,
        source_session_id: &str,
        target_session_id: &str,
        up_to_message_id: &str,
        new_id_fn: &dyn Fn() -> String,
    ) -> Result<usize, DbError> {
        // Find the rowid of the cutoff message
        let cutoff_rowid: i64 = self
            .conn()
            .query_row(
                "SELECT rowid FROM messages WHERE id = ?1 AND session_id = ?2",
                params![up_to_message_id, source_session_id],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    DbError::NotFound(format!("message {up_to_message_id}"))
                }
                other => other.into(),
            })?;

        // Select messages up to the cutoff (inclusive)
        let mut stmt = self.conn().prepare(
            "SELECT role, parts FROM messages \
             WHERE session_id = ?1 AND rowid <= ?2 \
             ORDER BY rowid ASC",
        )?;

        let rows: Vec<(String, String)> = stmt
            .query_map(params![source_session_id, cutoff_rowid], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let count = rows.len();

        // Insert copies with new IDs into the target session
        let mut insert_stmt = self.conn().prepare(
            "INSERT INTO messages (id, session_id, role, parts) VALUES (?1, ?2, ?3, ?4)",
        )?;

        for (role, parts) in &rows {
            let new_id = new_id_fn();
            insert_stmt.execute(params![new_id, target_session_id, role, parts])?;
        }

        Ok(count)
    }

    /// List child sessions (branches) of a given parent session.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn list_child_sessions(&self, parent_id: &str) -> Result<Vec<Session>, DbError> {
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, parent_id, title, model_id, created_at, updated_at, \
                    branch_from_message_id, branch_snapshot_hash \
             FROM sessions WHERE parent_id = ?1 ORDER BY created_at ASC",
        )?;

        let sessions =
            stmt.query_map(params![parent_id], map_session_row)?.collect::<Result<Vec<_>, _>>()?;

        Ok(sessions)
    }

    /// Count messages for multiple sessions in one query.
    ///
    /// Returns a map of `session_id -> message_count`.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn count_messages_batch(
        &self,
        project_id: &str,
    ) -> Result<std::collections::HashMap<String, usize>, DbError> {
        let mut stmt = self.conn().prepare(
            "SELECT m.session_id, COUNT(*) \
             FROM messages m \
             JOIN sessions s ON m.session_id = s.id \
             WHERE s.project_id = ?1 \
             GROUP BY m.session_id",
        )?;

        let mut counts = std::collections::HashMap::new();
        let rows = stmt.query_map(params![project_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;

        for row in rows {
            let (session_id, count) = row?;
            counts.insert(session_id, count as usize);
        }

        Ok(counts)
    }

    /// List messages in a session after a given message (exclusive).
    ///
    /// Returns messages with rowid strictly greater than the given message's
    /// rowid. Used for branch summary generation.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn list_messages_after(
        &self,
        session_id: &str,
        after_message_id: &str,
    ) -> Result<Vec<MessageRow>, DbError> {
        let after_rowid: i64 = self
            .conn()
            .query_row(
                "SELECT rowid FROM messages WHERE id = ?1 AND session_id = ?2",
                params![after_message_id, session_id],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    DbError::NotFound(format!("message {after_message_id}"))
                }
                other => other.into(),
            })?;

        let mut stmt = self.conn().prepare(
            "SELECT id, session_id, role, parts, created_at \
             FROM messages WHERE session_id = ?1 AND rowid > ?2 ORDER BY rowid ASC",
        )?;

        let messages = stmt
            .query_map(params![session_id, after_rowid], |row| {
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
}

// ---------------------------------------------------------------------------
// Session Labels
// ---------------------------------------------------------------------------

impl Db {
    /// Set or update a label on a session (one label per session).
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn upsert_session_label(&self, session_id: &str, label: &str) -> Result<(), DbError> {
        self.conn().execute(
            "INSERT OR REPLACE INTO session_labels (session_id, label) VALUES (?1, ?2)",
            params![session_id, label],
        )?;
        Ok(())
    }

    /// Get the label for a session, if any.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn get_session_label(&self, session_id: &str) -> Result<Option<SessionLabel>, DbError> {
        let result = self.conn().query_row(
            "SELECT id, session_id, label, created_at FROM session_labels WHERE session_id = ?1",
            params![session_id],
            |row| {
                Ok(SessionLabel {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    label: row.get(2)?,
                    created_at: row.get(3)?,
                })
            },
        );

        match result {
            Ok(label) => Ok(Some(label)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// List all session labels for a project.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn list_session_labels(&self, project_id: &str) -> Result<Vec<SessionLabel>, DbError> {
        let mut stmt = self.conn().prepare(
            "SELECT sl.id, sl.session_id, sl.label, sl.created_at \
             FROM session_labels sl \
             JOIN sessions s ON sl.session_id = s.id \
             WHERE s.project_id = ?1 \
             ORDER BY sl.created_at DESC",
        )?;

        let labels = stmt
            .query_map(params![project_id], |row| {
                Ok(SessionLabel {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    label: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(labels)
    }

    /// Delete a session's label.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn delete_session_label(&self, session_id: &str) -> Result<(), DbError> {
        self.conn()
            .execute("DELETE FROM session_labels WHERE session_id = ?1", params![session_id])?;
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

    // ---------------------------------------------------------------------------
    // Permission rules
    // ---------------------------------------------------------------------------

    /// Insert or replace a permission rule for a project.
    ///
    /// Uses `INSERT OR REPLACE` so that re-approving the same pattern
    /// updates the existing rule rather than creating a duplicate.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn upsert_permission_rule(
        &self,
        project_id: &str,
        permission: &str,
        pattern: &str,
        action: &str,
    ) -> Result<(), DbError> {
        self.conn().execute(
            "INSERT OR REPLACE INTO permission_rules (project_id, permission, pattern, action)
             VALUES (?1, ?2, ?3, ?4)",
            params![project_id, permission, pattern, action],
        )?;
        Ok(())
    }

    /// List all permission rules for a project.
    ///
    /// Rules are returned in insertion order (by `id`), which preserves
    /// the chronological order of user approvals.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn list_permission_rules(
        &self,
        project_id: &str,
    ) -> Result<Vec<crate::models::PermissionRuleRow>, DbError> {
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, permission, pattern, action, created_at
             FROM permission_rules
             WHERE project_id = ?1
             ORDER BY id ASC",
        )?;

        let rows = stmt.query_map(params![project_id], |row| {
            Ok(crate::models::PermissionRuleRow {
                id: row.get(0)?,
                project_id: row.get(1)?,
                permission: row.get(2)?,
                pattern: row.get(3)?,
                action: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;

        let mut rules = Vec::new();
        for row in rows {
            rules.push(row?);
        }
        Ok(rules)
    }

    /// Delete a specific permission rule by ID.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn delete_permission_rule(&self, id: i64) -> Result<(), DbError> {
        self.conn().execute("DELETE FROM permission_rules WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Delete all permission rules for a project.
    ///
    /// # Errors
    ///
    /// Returns `DbError` on database failures.
    pub fn clear_permission_rules(&self, project_id: &str) -> Result<(), DbError> {
        self.conn()
            .execute("DELETE FROM permission_rules WHERE project_id = ?1", params![project_id])?;
        Ok(())
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
        assert!(session.branch_from_message_id.is_none());
        assert!(session.branch_snapshot_hash.is_none());
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

    // -- Permission Rules --

    #[test]
    fn permission_rule_upsert_and_list() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();

        db.upsert_permission_rule("p1", "bash", "git commit *", "allow").unwrap();
        db.upsert_permission_rule("p1", "bash", "npm install *", "allow").unwrap();

        let rules = db.list_permission_rules("p1").unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].permission, "bash");
        assert_eq!(rules[0].pattern, "git commit *");
        assert_eq!(rules[0].action, "allow");
        assert_eq!(rules[1].pattern, "npm install *");
    }

    #[test]
    fn permission_rule_upsert_replaces_existing() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();

        db.upsert_permission_rule("p1", "bash", "git commit *", "ask").unwrap();
        db.upsert_permission_rule("p1", "bash", "git commit *", "allow").unwrap();

        let rules = db.list_permission_rules("p1").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].action, "allow");
    }

    #[test]
    fn permission_rule_delete_specific() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();

        db.upsert_permission_rule("p1", "bash", "git commit *", "allow").unwrap();
        db.upsert_permission_rule("p1", "bash", "npm install *", "allow").unwrap();

        let rules = db.list_permission_rules("p1").unwrap();
        assert_eq!(rules.len(), 2);

        db.delete_permission_rule(rules[0].id).unwrap();

        let remaining = db.list_permission_rules("p1").unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].pattern, "npm install *");
    }

    #[test]
    fn permission_rule_clear_all() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();

        db.upsert_permission_rule("p1", "bash", "git *", "allow").unwrap();
        db.upsert_permission_rule("p1", "edit", "*", "allow").unwrap();

        db.clear_permission_rules("p1").unwrap();

        let rules = db.list_permission_rules("p1").unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn permission_rules_scoped_to_project() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/project1").unwrap();
        db.get_or_create_project("p2", "/tmp/project2").unwrap();

        db.upsert_permission_rule("p1", "bash", "git *", "allow").unwrap();
        db.upsert_permission_rule("p2", "bash", "npm *", "allow").unwrap();

        let p1_rules = db.list_permission_rules("p1").unwrap();
        assert_eq!(p1_rules.len(), 1);
        assert_eq!(p1_rules[0].pattern, "git *");

        let p2_rules = db.list_permission_rules("p2").unwrap();
        assert_eq!(p2_rules.len(), 1);
        assert_eq!(p2_rules[0].pattern, "npm *");
    }

    // -- Branch Sessions --

    #[test]
    fn create_branch_session_with_metadata() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();
        db.insert_message("m1", "s1", "user", r#"[{"type":"text","text":"hello"}]"#).unwrap();

        let branch = db
            .create_branch_session(
                "s2",
                "p1",
                "s1",
                "model-a",
                "branch title",
                "m1",
                Some("abc123"),
            )
            .unwrap();

        assert_eq!(branch.id, "s2");
        assert_eq!(branch.parent_id.as_deref(), Some("s1"));
        assert_eq!(branch.branch_from_message_id.as_deref(), Some("m1"));
        assert_eq!(branch.branch_snapshot_hash.as_deref(), Some("abc123"));
        assert_eq!(branch.title, "branch title");
    }

    #[test]
    fn copy_messages_to_session_up_to_cutoff() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();
        db.create_session("s2", "p1", "model-a").unwrap();

        db.insert_message("m1", "s1", "user", r#"[{"type":"text","text":"first"}]"#).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        db.insert_message("m2", "s1", "assistant", r#"[{"type":"text","text":"second"}]"#).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        db.insert_message("m3", "s1", "user", r#"[{"type":"text","text":"third"}]"#).unwrap();

        let counter = std::cell::Cell::new(0u32);
        let copied = db
            .copy_messages_to_session("s1", "s2", "m2", &|| {
                let n = counter.get() + 1;
                counter.set(n);
                format!("copy_{n}")
            })
            .unwrap();

        assert_eq!(copied, 2); // m1 and m2
        let target_msgs = db.list_messages("s2").unwrap();
        assert_eq!(target_msgs.len(), 2);
        assert_eq!(target_msgs[0].role, "user");
        assert_eq!(target_msgs[1].role, "assistant");

        // Source session unmodified
        let source_msgs = db.list_messages("s1").unwrap();
        assert_eq!(source_msgs.len(), 3);
    }

    #[test]
    fn list_messages_after_returns_tail() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();

        db.insert_message("m1", "s1", "user", r#"[{"type":"text","text":"first"}]"#).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        db.insert_message("m2", "s1", "assistant", r#"[{"type":"text","text":"second"}]"#).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        db.insert_message("m3", "s1", "user", r#"[{"type":"text","text":"third"}]"#).unwrap();

        let after = db.list_messages_after("s1", "m1").unwrap();
        assert_eq!(after.len(), 2);
        assert_eq!(after[0].id, "m2");
        assert_eq!(after[1].id, "m3");

        // After the last message returns empty
        let after_last = db.list_messages_after("s1", "m3").unwrap();
        assert!(after_last.is_empty());
    }

    #[test]
    fn list_child_sessions_returns_branches() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();
        db.insert_message("m1", "s1", "user", "[]").unwrap();

        db.create_branch_session("s2", "p1", "s1", "model-a", "branch-a", "m1", None).unwrap();
        db.create_branch_session("s3", "p1", "s1", "model-a", "branch-b", "m1", None).unwrap();

        let children = db.list_child_sessions("s1").unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].id, "s2");
        assert_eq!(children[1].id, "s3");
    }

    #[test]
    fn count_messages_batch_returns_all_sessions() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();
        db.create_session("s2", "p1", "model-a").unwrap();

        db.insert_message("m1", "s1", "user", "[]").unwrap();
        db.insert_message("m2", "s1", "assistant", "[]").unwrap();
        db.insert_message("m3", "s2", "user", "[]").unwrap();

        let counts = db.count_messages_batch("p1").unwrap();
        assert_eq!(counts.get("s1"), Some(&2));
        assert_eq!(counts.get("s2"), Some(&1));
    }

    // -- Session Labels --

    #[test]
    fn session_label_upsert_and_get() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();

        db.upsert_session_label("s1", "checkpoint: auth working").unwrap();
        let label = db.get_session_label("s1").unwrap().unwrap();
        assert_eq!(label.label, "checkpoint: auth working");

        // Upsert replaces
        db.upsert_session_label("s1", "updated label").unwrap();
        let label = db.get_session_label("s1").unwrap().unwrap();
        assert_eq!(label.label, "updated label");
    }

    #[test]
    fn session_label_get_none() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();

        assert!(db.get_session_label("s1").unwrap().is_none());
    }

    #[test]
    fn session_label_list_for_project() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();
        db.create_session("s2", "p1", "model-a").unwrap();

        db.upsert_session_label("s1", "label-a").unwrap();
        db.upsert_session_label("s2", "label-b").unwrap();

        let labels = db.list_session_labels("p1").unwrap();
        assert_eq!(labels.len(), 2);
    }

    #[test]
    fn session_label_delete() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();

        db.upsert_session_label("s1", "temp label").unwrap();
        assert!(db.get_session_label("s1").unwrap().is_some());

        db.delete_session_label("s1").unwrap();
        assert!(db.get_session_label("s1").unwrap().is_none());
    }
}
