//! Session tree construction — builds a navigable tree from flat session rows.
//!
//! Sessions are linked via `parent_id` (parent→child). The tree builder
//! loads all sessions for a project, groups them by parent, and assembles
//! a recursive tree structure for the TUI to render.

use std::collections::HashMap;

use flok_db::{Db, Session};

/// A node in the session tree.
#[derive(Debug, Clone)]
pub struct SessionTreeNode {
    pub session: Session,
    pub children: Vec<SessionTreeNode>,
    pub label: Option<String>,
    pub message_count: usize,
    pub is_current: bool,
    /// The message ID in the parent session where this branch was taken.
    pub branch_from_message_id: Option<String>,
}

/// Build the full session tree for a project.
///
/// Returns a list of root nodes (sessions with no parent). Each root
/// has its children recursively attached. The `current_session_id` is
/// marked with `is_current = true` for TUI highlighting.
///
/// # Errors
///
/// Returns an error if database queries fail.
/// Recursively attach children to a node, consuming it from the nodes map.
fn attach_children(
    node_id: &str,
    nodes: &mut HashMap<String, SessionTreeNode>,
    children_map: &HashMap<String, Vec<String>>,
) -> Option<SessionTreeNode> {
    let child_ids = children_map.get(node_id).cloned().unwrap_or_default();
    let mut children: Vec<SessionTreeNode> = Vec::new();
    for cid in &child_ids {
        if let Some(child) = attach_children(cid, nodes, children_map) {
            children.push(child);
        }
    }
    // Sort children by created_at (oldest first)
    children.sort_by(|a, b| a.session.created_at.cmp(&b.session.created_at));

    let mut node = nodes.remove(node_id)?;
    node.children = children;
    Some(node)
}

pub fn build_session_tree(
    db: &Db,
    project_id: &str,
    current_session_id: &str,
) -> anyhow::Result<Vec<SessionTreeNode>> {
    // 1. Load all sessions for the project
    let sessions = db.list_sessions(project_id)?;

    // 2. Load labels (batch)
    let labels = db.list_session_labels(project_id)?;
    let label_map: HashMap<String, String> =
        labels.into_iter().map(|l| (l.session_id, l.label)).collect();

    // 3. Load message counts (batch)
    let counts = db.count_messages_batch(project_id)?;

    // 4. Build nodes and group by parent
    let mut nodes: HashMap<String, SessionTreeNode> = HashMap::new();
    let mut children_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut roots: Vec<String> = Vec::new();

    for session in &sessions {
        // Skip sub-agent sessions: they have parent_id but no branch_from_message_id.
        // User branches always have branch_from_message_id set.
        // Root sessions have parent_id = None.
        if session.parent_id.is_some() && session.branch_from_message_id.is_none() {
            continue;
        }

        let node = SessionTreeNode {
            branch_from_message_id: session.branch_from_message_id.clone(),
            label: label_map.get(&session.id).cloned(),
            message_count: counts.get(&session.id).copied().unwrap_or(0),
            is_current: session.id == current_session_id,
            session: session.clone(),
            children: Vec::new(),
        };
        nodes.insert(session.id.clone(), node);

        match &session.parent_id {
            Some(pid) => {
                children_map.entry(pid.clone()).or_default().push(session.id.clone());
            }
            None => roots.push(session.id.clone()),
        }
    }

    // 5. Attach children and build the tree
    let tree: Vec<SessionTreeNode> =
        roots.iter().filter_map(|rid| attach_children(rid, &mut nodes, &children_map)).collect();

    Ok(tree)
}

/// Depth-first walk of a single node and its children.
fn walk_tree<'a>(
    node: &'a SessionTreeNode,
    depth: usize,
    out: &mut Vec<(usize, &'a SessionTreeNode)>,
) {
    out.push((depth, node));
    for child in &node.children {
        walk_tree(child, depth + 1, out);
    }
}

/// Flatten a session tree into a depth-first list with indentation levels.
///
/// Useful for TUI rendering where a flat list with depth info is easier
/// to work with than a recursive tree.
pub fn flatten_tree(roots: &[SessionTreeNode]) -> Vec<(usize, &SessionTreeNode)> {
    let mut result = Vec::new();
    for root in roots {
        walk_tree(root, 0, &mut result);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use flok_db::Db;

    fn test_db() -> Db {
        Db::open_in_memory().unwrap()
    }

    #[test]
    fn build_tree_single_root() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();
        db.insert_message("m1", "s1", "user", "[]").unwrap();

        let tree = build_session_tree(&db, "p1", "s1").unwrap();
        assert_eq!(tree.len(), 1);
        assert!(tree[0].is_current);
        assert_eq!(tree[0].message_count, 1);
        assert!(tree[0].children.is_empty());
    }

    #[test]
    fn build_tree_with_branches() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();
        db.insert_message("m1", "s1", "user", "[]").unwrap();

        db.create_branch_session("s2", "p1", "s1", "model-a", "branch-a", "m1", None).unwrap();
        db.create_branch_session("s3", "p1", "s1", "model-a", "branch-b", "m1", None).unwrap();

        let tree = build_session_tree(&db, "p1", "s2").unwrap();
        assert_eq!(tree.len(), 1); // One root
        assert_eq!(tree[0].children.len(), 2); // Two branches
        assert!(!tree[0].is_current);
        assert!(tree[0].children[0].is_current); // s2 is current
    }

    #[test]
    fn build_tree_with_labels() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();
        db.upsert_session_label("s1", "checkpoint").unwrap();

        let tree = build_session_tree(&db, "p1", "s1").unwrap();
        assert_eq!(tree[0].label.as_deref(), Some("checkpoint"));
    }

    #[test]
    fn flatten_tree_produces_depth_first_order() {
        let db = test_db();
        db.get_or_create_project("p1", "/tmp/test").unwrap();
        db.create_session("s1", "p1", "model-a").unwrap();
        db.insert_message("m1", "s1", "user", "[]").unwrap();

        db.create_branch_session("s2", "p1", "s1", "model-a", "b1", "m1", None).unwrap();
        db.insert_message("m2", "s2", "user", "[]").unwrap();

        db.create_branch_session("s3", "p1", "s2", "model-a", "b2", "m2", None).unwrap();

        let tree = build_session_tree(&db, "p1", "s1").unwrap();
        let flat = flatten_tree(&tree);

        assert_eq!(flat.len(), 3);
        assert_eq!(flat[0].0, 0); // s1 at depth 0
        assert_eq!(flat[0].1.session.id, "s1");
        assert_eq!(flat[1].0, 1); // s2 at depth 1
        assert_eq!(flat[1].1.session.id, "s2");
        assert_eq!(flat[2].0, 2); // s3 at depth 2
        assert_eq!(flat[2].1.session.id, "s3");
    }
}
