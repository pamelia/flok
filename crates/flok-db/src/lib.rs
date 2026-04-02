//! # flok-db
//!
//! Database layer for flok. Provides `SQLite`-backed storage for sessions,
//! messages, and project metadata. All database operations are synchronous
//! (rusqlite) and should be called from a blocking task context.

mod migrations;
mod models;
mod queries;
mod schema;

pub use models::{Message, MessageRow, PermissionRuleRow, Project, Role, Session, SessionLabel};
pub use schema::{Db, DbError};
