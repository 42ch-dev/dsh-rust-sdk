//! The client-side session tree: the bounded `subagent.started` edge map and
//! the tree-membership filter.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::Value;

use crate::protocol::Notification;

use super::MAX_PARENT_EDGES;

/// The client-side `subagent.started` parent→child session edge map, bounded
/// to [`MAX_PARENT_EDGES`] entries with drop-oldest eviction.
///
/// Each entry maps a child session id to its parent; the insertion order is
/// tracked so the oldest edges are evicted first once the cap is reached.
#[derive(Debug, Default)]
pub(super) struct ParentMap {
    /// child session id -> parent session id.
    edges: HashMap<String, String>,
    /// Child ids in insertion order, for drop-oldest eviction.
    order: VecDeque<String>,
}

impl ParentMap {
    pub(super) fn new() -> Self {
        Self::default()
    }

    fn get(&self, child: &str) -> Option<&String> {
        self.edges.get(child)
    }

    /// Record (or update) a parent→child edge, evicting the oldest edges
    /// once the map exceeds [`MAX_PARENT_EDGES`].
    fn insert(&mut self, child: String, parent: String) {
        if !self.edges.contains_key(&child) {
            self.order.push_back(child.clone());
        }
        self.edges.insert(child, parent);
        while self.order.len() > MAX_PARENT_EDGES {
            let oldest = self.order.pop_front().expect("order mirrors edges");
            self.edges.remove(&oldest);
        }
    }
}

/// Record a parent→child session edge when `notification` is a well-formed
/// `subagent.started` (both ids non-empty strings, parent != child).
///
/// Called by the read loop **before** fan-out so the tree filter sees fresh
/// edges; reference parity with the Python and TypeScript clients.
pub(super) fn record_session_relationship(map: &mut ParentMap, notification: &Notification) {
    if notification.method != "subagent.started" {
        return;
    }
    let Some(parent) = notification
        .payload
        .get("parentSessionId")
        .and_then(Value::as_str)
        .filter(|parent| !parent.is_empty())
    else {
        return;
    };
    let Some(child) = notification
        .payload
        .get("childSessionId")
        .and_then(Value::as_str)
        .filter(|child| !child.is_empty() && *child != parent)
    else {
        return;
    };
    map.insert(child.to_string(), parent.to_string());
}

/// Whether `session` is `root` itself or reachable by walking parent edges.
///
/// The walk is cycle-guarded (the edge map only ever extends chains upward,
/// so a cycle cannot form; the guard is defensive).
fn is_descendant_of(map: &ParentMap, session: &str, root: &str) -> bool {
    let mut visited: HashSet<&str> = HashSet::new();
    let mut current = session;
    loop {
        if current == root {
            return true;
        }
        if !visited.insert(current) {
            return false;
        }
        match map.get(current) {
            Some(parent) => current = parent.as_str(),
            None => return false,
        }
    }
}

/// Whether a notification belongs to the session tree rooted at `root`.
///
/// `subagent.started`/`subagent.finished` pass when the parent session is
/// already in the tree, or when the child session is the root itself; other
/// notifications pass when their `sessionId` is in the tree.
pub(super) fn notification_in_tree(
    map: &ParentMap,
    notification: &Notification,
    root: &str,
) -> bool {
    if matches!(
        notification.method.as_str(),
        "subagent.started" | "subagent.finished"
    ) {
        if let Some(parent) = notification
            .payload
            .get("parentSessionId")
            .and_then(Value::as_str)
        {
            if is_descendant_of(map, parent, root) {
                return true;
            }
        }
        return notification
            .payload
            .get("childSessionId")
            .and_then(Value::as_str)
            == Some(root);
    }
    match notification
        .payload
        .get("sessionId")
        .and_then(Value::as_str)
    {
        Some(session) => is_descendant_of(map, session, root),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map};

    fn notification(method: &str, fields: &[(&str, Value)]) -> Notification {
        let mut payload = Map::new();
        for (key, value) in fields {
            payload.insert((*key).to_string(), value.clone());
        }
        Notification {
            method: method.to_string(),
            payload,
        }
    }

    fn started(parent: &str, child: &str) -> Notification {
        notification(
            "subagent.started",
            &[
                ("parentSessionId", json!(parent)),
                ("childSessionId", json!(child)),
            ],
        )
    }

    fn event(session: &str) -> Notification {
        notification(
            "session.event",
            &[
                ("sessionId", json!(session)),
                ("event", json!({ "type": "assistant/message" })),
            ],
        )
    }

    #[test]
    fn parent_map_records_valid_started_edges_and_ignores_invalid() {
        let mut map = ParentMap::new();
        record_session_relationship(&mut map, &started("root", "child1"));
        record_session_relationship(&mut map, &started("child1", "child2"));
        // Non-`subagent.started` notifications never record edges.
        record_session_relationship(&mut map, &event("root"));
        record_session_relationship(
            &mut map,
            &notification(
                "subagent.finished",
                &[
                    ("parentSessionId", json!("x")),
                    ("childSessionId", json!("y")),
                ],
            ),
        );
        // Degenerate edges are ignored (reference parity).
        record_session_relationship(&mut map, &started("root", ""));
        record_session_relationship(&mut map, &started("", "child"));
        record_session_relationship(&mut map, &started("same", "same"));
        record_session_relationship(
            &mut map,
            &notification(
                "subagent.started",
                &[
                    ("parentSessionId", json!(1)),
                    ("childSessionId", json!("child")),
                ],
            ),
        );

        assert_eq!(map.edges.len(), 2);
        assert_eq!(map.get("child1").map(String::as_str), Some("root"));
        assert_eq!(map.get("child2").map(String::as_str), Some("child1"));
    }

    #[test]
    fn parent_map_evicts_oldest_edges_past_the_cap() {
        // FIX-3: the edge map is bounded; once MAX_PARENT_EDGES is reached
        // the oldest edges are evicted (drop-oldest), so a long-lived client
        // cannot grow the tree without bound.
        let mut map = ParentMap::new();
        for i in 0..MAX_PARENT_EDGES + 50 {
            map.insert(format!("child-{i}"), format!("parent-{i}"));
        }
        assert_eq!(
            map.edges.len(),
            MAX_PARENT_EDGES,
            "the map must never exceed the cap"
        );
        assert!(
            map.get("child-0").is_none(),
            "the oldest edges must be evicted first"
        );
        assert_eq!(
            map.get("child-50").map(String::as_str),
            Some("parent-50"),
            "the first 50 edges (child-0..child-49) are gone; child-50 is the oldest survivor"
        );
        assert_eq!(
            map.get(&format!("child-{}", MAX_PARENT_EDGES + 49))
                .map(String::as_str),
            Some("parent-100049"),
            "the newest edge must be retained"
        );

        // Re-inserting a known child updates its parent without duplicating
        // the eviction order.
        let mut single = ParentMap::new();
        single.insert("child".into(), "parent-1".into());
        single.insert("child".into(), "parent-2".into());
        assert_eq!(single.edges.len(), 1);
        assert_eq!(single.get("child").map(String::as_str), Some("parent-2"));
    }

    #[test]
    fn descendant_check_walks_edges_and_guards_cycles() {
        let mut map = ParentMap::new();
        record_session_relationship(&mut map, &started("root", "child1"));
        record_session_relationship(&mut map, &started("child1", "child2"));
        record_session_relationship(&mut map, &started("other", "child3"));

        assert!(is_descendant_of(&map, "root", "root")); // the root itself
        assert!(is_descendant_of(&map, "child1", "root"));
        assert!(is_descendant_of(&map, "child2", "root"));
        assert!(!is_descendant_of(&map, "child3", "root"));
        assert!(!is_descendant_of(&map, "child3", "child1"));
        assert!(is_descendant_of(&map, "child3", "other"));

        // A cycle (impossible via the record rule, but the walk must not hang).
        map.insert("a".into(), "b".into());
        map.insert("b".into(), "a".into());
        assert!(!is_descendant_of(&map, "a", "root"));
    }

    #[test]
    fn tree_filter_membership_follows_sequence() {
        // The sequence a client would observe: root starts child1, which
        // starts child2; "other" and "sub" belong to a different tree.
        let mut map = ParentMap::new();
        for edge in [
            started("root", "child1"),
            started("child1", "child2"),
            started("other", "sub"),
        ] {
            record_session_relationship(&mut map, &edge);
        }

        assert!(notification_in_tree(&map, &event("root"), "root"));
        assert!(notification_in_tree(&map, &event("child1"), "root"));
        assert!(notification_in_tree(&map, &event("child2"), "root"));
        assert!(!notification_in_tree(&map, &event("other"), "root"));
        assert!(!notification_in_tree(&map, &event("unrelated"), "root"));

        // Lifecycle edges pass when the *parent* is in the tree...
        assert!(notification_in_tree(
            &map,
            &started("child1", "child3"),
            "root"
        ));
        assert!(!notification_in_tree(
            &map,
            &started("other", "child3"),
            "root"
        ));
        assert!(notification_in_tree(
            &map,
            &notification(
                "subagent.finished",
                &[
                    ("parentSessionId", json!("child2")),
                    ("childSessionId", json!("grandchild")),
                ],
            ),
            "root"
        ));
        // ...or when the child session is the root itself.
        assert!(notification_in_tree(
            &map,
            &notification(
                "subagent.finished",
                &[
                    ("parentSessionId", json!("unrelated")),
                    ("childSessionId", json!("root")),
                ],
            ),
            "root"
        ));

        // Notifications without a session identity never match.
        assert!(!notification_in_tree(
            &map,
            &notification("session.status", &[]),
            "root"
        ));
        assert!(!notification_in_tree(
            &map,
            &notification("subagent.started", &[("parentSessionId", json!(7))],),
            "root"
        ));
    }
}
