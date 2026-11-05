//! Pure workspace tree logic: the explicit parent/child link on top of the
//! `state.workspaces` order and worktree group membership.
//!
//! The module knows neither the server nor the client: the server checks its
//! invariants here, the sidebar builds entries, keys and the CLI plan moves.

use std::collections::{HashMap, HashSet};

use crate::api::schema::{Method, WorkspaceMoveBlockParams, WorkspaceMoveParams};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkspaceNode<'a> {
    pub id: &'a str,
    pub worktree_key: Option<&'a str>,
    pub is_linked_worktree: bool,
    pub parent_id: Option<&'a str>,
}

/// Keys of worktree groups that have both a root and a linked child.
pub(crate) fn grouped_worktree_keys<'a>(nodes: &[WorkspaceNode<'a>]) -> HashSet<&'a str> {
    let linked = nodes
        .iter()
        .filter(|node| node.is_linked_worktree)
        .filter_map(|node| node.worktree_key)
        .collect::<HashSet<_>>();
    nodes
        .iter()
        .filter(|node| !node.is_linked_worktree)
        .filter_map(|node| node.worktree_key)
        .filter(|key| linked.contains(key))
        .collect()
}

/// Index of the effective explicit parent. A link that breaks the tree
/// invariants has no effect: a missing or self reference, a parent that has its
/// own parent, a linked worktree parent, or a child inside a worktree group.
pub(crate) fn effective_parent_index(
    nodes: &[WorkspaceNode<'_>],
    grouped: &HashSet<&str>,
    index: usize,
) -> Option<usize> {
    let node = nodes.get(index)?;
    let parent_id = node.parent_id?;
    if node.is_linked_worktree || node.worktree_key.is_some_and(|key| grouped.contains(key)) {
        return None;
    }
    let parent_index = nodes
        .iter()
        .position(|candidate| candidate.id == parent_id)?;
    let parent = &nodes[parent_index];
    (parent_index != index && parent.parent_id.is_none() && !parent.is_linked_worktree)
        .then_some(parent_index)
}

/// Explicit children of each parent, in `state.workspaces` order.
pub(crate) fn explicit_children(nodes: &[WorkspaceNode<'_>]) -> HashMap<usize, Vec<usize>> {
    let mut children = HashMap::<usize, Vec<usize>>::new();
    if nodes.iter().all(|node| node.parent_id.is_none()) {
        return children;
    }
    let grouped = grouped_worktree_keys(nodes);
    for index in 0..nodes.len() {
        if let Some(parent) = effective_parent_index(nodes, &grouped, index) {
            children.entry(parent).or_default().push(index);
        }
    }
    children
}

/// Indices whose link has no effect and must be cleared.
pub(crate) fn invalid_parent_links(nodes: &[WorkspaceNode<'_>]) -> Vec<usize> {
    let grouped = grouped_worktree_keys(nodes);
    (0..nodes.len())
        .filter(|index| {
            nodes[*index].parent_id.is_some()
                && effective_parent_index(nodes, &grouped, *index).is_none()
        })
        .collect()
}

/// Why a parent link is refused: the API error code and message.
pub(crate) fn parent_link_error(
    nodes: &[WorkspaceNode<'_>],
    child: Option<usize>,
    parent: usize,
) -> Option<(&'static str, &'static str)> {
    let parent_node = nodes.get(parent)?;
    if child == Some(parent) {
        return Some((
            "workspace_parent_self",
            "a workspace cannot be its own parent",
        ));
    }
    if parent_node.parent_id.is_some() {
        return Some((
            "workspace_parent_nested",
            "the parent workspace already has a parent; only one nesting level is supported",
        ));
    }
    if parent_node.is_linked_worktree {
        return Some((
            "workspace_parent_worktree_child",
            "a linked worktree workspace cannot be a parent",
        ));
    }
    let child_node = nodes.get(child?)?;
    if child_node.is_linked_worktree {
        return Some((
            "workspace_parent_worktree_child",
            "a linked worktree workspace stays in its worktree group",
        ));
    }
    if child_node
        .worktree_key
        .is_some_and(|key| grouped_worktree_keys(nodes).contains(key))
    {
        return Some((
            "workspace_parent_worktree_group",
            "the root of a worktree group cannot have a parent",
        ));
    }
    if nodes
        .iter()
        .any(|candidate| candidate.parent_id == Some(child_node.id))
    {
        return Some((
            "workspace_parent_nested",
            "the workspace already has children; only one nesting level is supported",
        ));
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceMovePlacement {
    Before(String),
    After(String),
    Index(usize),
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceMoveError(pub String);

/// A top-level sidebar entry: a root with all of its children.
struct RootUnit {
    /// First member ignoring explicit children: the sidebar positions the entry by it.
    anchor: usize,
    /// All block members in `state.workspaces` order.
    members: Vec<usize>,
}

fn root_units(
    nodes: &[WorkspaceNode<'_>],
    grouped: &HashSet<&str>,
    children: &HashMap<usize, Vec<usize>>,
) -> Vec<RootUnit> {
    let mut seen_keys = HashSet::new();
    let is_child = children
        .values()
        .flatten()
        .copied()
        .collect::<HashSet<usize>>();
    let mut units = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        if is_child.contains(&index) {
            continue;
        }
        let key = node.worktree_key.filter(|key| grouped.contains(key));
        if key.is_some_and(|key| !seen_keys.insert(key)) {
            continue;
        }
        let mut members = match key {
            Some(key) => nodes
                .iter()
                .enumerate()
                .filter(|(_, member)| member.worktree_key == Some(key))
                .map(|(member, _)| member)
                .collect::<Vec<_>>(),
            None => vec![index],
        };
        let explicit = members
            .iter()
            .filter_map(|member| children.get(member))
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        members.extend(explicit);
        members.sort_unstable();
        units.push(RootUnit {
            anchor: index,
            members,
        });
    }
    units
}

/// Insert position among the remaining entries of the same level.
fn destination(
    placement: &WorkspaceMovePlacement,
    position: usize,
    count: usize,
    position_of: impl Fn(&str) -> Result<usize, WorkspaceMoveError>,
) -> Result<Option<usize>, WorkspaceMoveError> {
    let dest = match placement {
        WorkspaceMovePlacement::Before(target) => position_of(target)?,
        WorkspaceMovePlacement::After(target) => position_of(target)? + 1,
        WorkspaceMovePlacement::Index(index) => {
            if *index >= count {
                return Err(WorkspaceMoveError(format!(
                    "index {index} is out of bounds for {count} positions"
                )));
            }
            *index
        }
        WorkspaceMovePlacement::Up => match position.checked_sub(1) {
            Some(dest) => dest,
            None => return Ok(None),
        },
        WorkspaceMovePlacement::Down => {
            if position + 1 >= count {
                return Ok(None);
            }
            position + 1
        }
    };
    Ok((dest != position).then_some(dest))
}

/// The API method that moves a workspace. `Ok(None)` means it is already there.
pub(crate) fn plan_workspace_move(
    nodes: &[WorkspaceNode<'_>],
    source: &str,
    placement: &WorkspaceMovePlacement,
) -> Result<Option<Method>, WorkspaceMoveError> {
    let source_index = nodes
        .iter()
        .position(|node| node.id == source)
        .ok_or_else(|| WorkspaceMoveError(format!("workspace {source} not found")))?;
    let grouped = grouped_worktree_keys(nodes);
    let children = explicit_children(nodes);
    let source_node = &nodes[source_index];
    let siblings = if let Some(parent) = effective_parent_index(nodes, &grouped, source_index) {
        children.get(&parent).cloned()
    } else {
        source_node
            .worktree_key
            .filter(|key| source_node.is_linked_worktree && grouped.contains(key))
            .map(|key| {
                nodes
                    .iter()
                    .enumerate()
                    .filter(|(_, node)| node.is_linked_worktree && node.worktree_key == Some(key))
                    .map(|(index, _)| index)
                    .collect()
            })
    };
    if let Some(siblings) = siblings {
        return plan_child_move(nodes, source_index, &siblings, placement);
    }
    plan_root_move(nodes, source_index, &grouped, &children, placement)
}

fn plan_child_move(
    nodes: &[WorkspaceNode<'_>],
    source_index: usize,
    siblings: &[usize],
    placement: &WorkspaceMovePlacement,
) -> Result<Option<Method>, WorkspaceMoveError> {
    let source = nodes[source_index].id;
    let Some(position) = siblings.iter().position(|index| *index == source_index) else {
        return Ok(None);
    };
    let remaining = siblings
        .iter()
        .copied()
        .filter(|index| *index != source_index)
        .collect::<Vec<_>>();
    let dest = destination(placement, position, siblings.len(), |target| {
        remaining
            .iter()
            .position(|index| nodes[*index].id == target)
            .ok_or_else(|| {
                WorkspaceMoveError(format!(
                    "workspace {target} is not a sibling of child workspace {source}"
                ))
            })
    })?;
    let Some(dest) = dest else {
        return Ok(None);
    };
    let insert_index = match remaining.get(dest) {
        Some(next) => *next,
        None => remaining.last().map_or(source_index, |last| *last + 1),
    };
    Ok(Some(Method::WorkspaceMove(WorkspaceMoveParams {
        workspace_id: source.to_owned(),
        insert_index,
    })))
}

fn plan_root_move(
    nodes: &[WorkspaceNode<'_>],
    source_index: usize,
    grouped: &HashSet<&str>,
    children: &HashMap<usize, Vec<usize>>,
    placement: &WorkspaceMovePlacement,
) -> Result<Option<Method>, WorkspaceMoveError> {
    let source = nodes[source_index].id;
    let units = root_units(nodes, grouped, children);
    let Some(position) = units
        .iter()
        .position(|unit| unit.members.contains(&source_index))
    else {
        return Ok(None);
    };
    let remaining = units
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != position)
        .map(|(_, unit)| unit)
        .collect::<Vec<_>>();
    let dest = destination(placement, position, units.len(), |target| {
        if units[position]
            .members
            .iter()
            .any(|member| nodes[*member].id == target)
        {
            return Err(WorkspaceMoveError(format!(
                "workspace {target} moves together with workspace {source}"
            )));
        }
        remaining
            .iter()
            .position(|unit| unit.anchor_or_root_matches(nodes, target))
            .ok_or_else(|| {
                WorkspaceMoveError(format!(
                    "workspace {target} is not a top-level workspace; move relative to its parent"
                ))
            })
    })?;
    let Some(dest) = dest else {
        return Ok(None);
    };
    let before = remaining.get(dest).map(|unit| unit.anchor);
    let members = &units[position].members;
    if members.len() == 1 {
        return Ok(Some(Method::WorkspaceMove(WorkspaceMoveParams {
            workspace_id: source.to_owned(),
            insert_index: before.unwrap_or(nodes.len()),
        })));
    }
    Ok(Some(Method::WorkspaceMoveBlock(WorkspaceMoveBlockParams {
        workspace_ids: members
            .iter()
            .map(|member| nodes[*member].id.to_owned())
            .collect(),
        before_workspace_id: before.map(|index| nodes[index].id.to_owned()),
    })))
}

impl RootUnit {
    /// A move target must be a root of the entry: the anchor or a non-linked group member.
    fn anchor_or_root_matches(&self, nodes: &[WorkspaceNode<'_>], target: &str) -> bool {
        nodes[self.anchor].id == target
            || self.members.iter().any(|member| {
                let node = &nodes[*member];
                node.id == target && node.worktree_key.is_some() && !node.is_linked_worktree
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(id: &str) -> WorkspaceNode<'_> {
        WorkspaceNode {
            id,
            worktree_key: None,
            is_linked_worktree: false,
            parent_id: None,
        }
    }

    fn child<'a>(id: &'a str, parent: &'a str) -> WorkspaceNode<'a> {
        WorkspaceNode {
            parent_id: Some(parent),
            ..root(id)
        }
    }

    fn wt<'a>(id: &'a str, key: &'a str, linked: bool) -> WorkspaceNode<'a> {
        WorkspaceNode {
            worktree_key: Some(key),
            is_linked_worktree: linked,
            ..root(id)
        }
    }

    /// Applies a plan to an order the way move_workspace and move_workspace_block do.
    fn apply(order: &[&str], method: Option<Method>) -> Vec<String> {
        let mut order = order.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
        match method {
            None => {}
            Some(Method::WorkspaceMove(params)) => {
                let source = order
                    .iter()
                    .position(|id| *id == params.workspace_id)
                    .unwrap();
                let target = if source < params.insert_index {
                    params.insert_index - 1
                } else {
                    params.insert_index
                };
                let moved = order.remove(source);
                order.insert(target, moved);
            }
            Some(Method::WorkspaceMoveBlock(params)) => {
                order.retain(|id| !params.workspace_ids.contains(id));
                let at = params
                    .before_workspace_id
                    .as_ref()
                    .and_then(|before| order.iter().position(|id| id == before))
                    .unwrap_or(order.len());
                order.splice(at..at, params.workspace_ids);
            }
            Some(other) => panic!("unexpected method {other:?}"),
        }
        order
    }

    fn moved(
        nodes: &[WorkspaceNode<'_>],
        source: &str,
        placement: WorkspaceMovePlacement,
    ) -> Vec<String> {
        let order = nodes.iter().map(|node| node.id).collect::<Vec<_>>();
        apply(
            &order,
            plan_workspace_move(nodes, source, &placement).unwrap(),
        )
    }

    #[test]
    fn explicit_children_keep_workspace_order_and_ignore_invalid_links() {
        let nodes = [
            child("c2", "p"),
            root("p"),
            child("c1", "p"),
            child("missing", "gone"),
            child("self", "self"),
            child("nested", "c1"),
        ];
        let children = explicit_children(&nodes);
        assert_eq!(children.len(), 1);
        assert_eq!(children[&1], vec![0, 2]);
        assert_eq!(invalid_parent_links(&nodes), vec![3, 4, 5]);
    }

    #[test]
    fn worktree_group_members_do_not_become_explicit_children() {
        let nodes = [
            root("p"),
            WorkspaceNode {
                parent_id: Some("p"),
                ..wt("group-root", "repo", false)
            },
            WorkspaceNode {
                parent_id: Some("p"),
                ..wt("linked", "repo", true)
            },
            WorkspaceNode {
                parent_id: Some("p"),
                ..wt("alone", "other", false)
            },
        ];
        assert_eq!(explicit_children(&nodes)[&0], vec![3]);
        assert_eq!(invalid_parent_links(&nodes), vec![1, 2]);
    }

    #[test]
    fn parent_link_errors_name_each_refused_shape() {
        let nodes = [
            root("a"),
            child("b", "a"),
            wt("group-root", "repo", false),
            wt("linked", "repo", true),
            root("free"),
        ];
        let code = |child: Option<usize>, parent: usize| {
            parent_link_error(&nodes, child, parent).map(|(code, _)| code)
        };
        assert_eq!(code(Some(4), 4), Some("workspace_parent_self"));
        assert_eq!(code(Some(4), 1), Some("workspace_parent_nested"));
        assert_eq!(code(Some(0), 4), Some("workspace_parent_nested"));
        assert_eq!(code(Some(4), 3), Some("workspace_parent_worktree_child"));
        assert_eq!(code(Some(3), 4), Some("workspace_parent_worktree_child"));
        assert_eq!(code(Some(2), 4), Some("workspace_parent_worktree_group"));
        assert_eq!(code(Some(4), 0), None);
        assert_eq!(code(Some(4), 2), None);
        assert_eq!(code(None, 0), None);
        assert_eq!(code(None, 1), Some("workspace_parent_nested"));
    }

    #[test]
    fn root_without_children_moves_alone_among_roots() {
        let nodes = [root("a"), root("b"), root("c")];
        assert_eq!(
            moved(&nodes, "c", WorkspaceMovePlacement::Up),
            ["a", "c", "b"]
        );
        assert_eq!(
            moved(&nodes, "a", WorkspaceMovePlacement::Down),
            ["b", "a", "c"]
        );
        assert_eq!(
            moved(&nodes, "a", WorkspaceMovePlacement::After("c".into())),
            ["b", "c", "a"]
        );
        assert_eq!(
            moved(&nodes, "c", WorkspaceMovePlacement::Before("a".into())),
            ["c", "a", "b"]
        );
        assert_eq!(
            moved(&nodes, "a", WorkspaceMovePlacement::Index(2)),
            ["b", "c", "a"]
        );
        assert!(
            plan_workspace_move(&nodes, "a", &WorkspaceMovePlacement::Up)
                .unwrap()
                .is_none()
        );
        assert!(
            plan_workspace_move(&nodes, "c", &WorkspaceMovePlacement::Down)
                .unwrap()
                .is_none()
        );
        assert!(
            plan_workspace_move(&nodes, "a", &WorkspaceMovePlacement::Index(3)).is_err(),
            "index past the last position is refused"
        );
    }

    #[test]
    fn root_sharing_a_key_with_a_child_of_another_parent_moves_alone() {
        let nodes = [
            wt("r", "key", false),
            root("p"),
            WorkspaceNode {
                parent_id: Some("p"),
                ..wt("c", "key", false)
            },
        ];
        let order = moved(&nodes, "r", WorkspaceMovePlacement::Index(1));
        assert_eq!(order, ["p", "c", "r"]);
    }

    #[test]
    fn parent_moves_with_explicit_children_as_one_block() {
        let nodes = [
            root("a"),
            root("p"),
            child("c1", "p"),
            root("z"),
            child("c2", "p"),
        ];
        let method = plan_workspace_move(&nodes, "p", &WorkspaceMovePlacement::Down)
            .unwrap()
            .unwrap();
        let Method::WorkspaceMoveBlock(params) = &method else {
            panic!("expected a block move, got {method:?}");
        };
        assert_eq!(params.workspace_ids, ["p", "c1", "c2"]);
        assert_eq!(params.before_workspace_id, None);
        assert_eq!(
            moved(&nodes, "p", WorkspaceMovePlacement::Up),
            ["p", "c1", "c2", "a", "z"]
        );
    }

    #[test]
    fn worktree_root_moves_with_linked_and_explicit_children() {
        let nodes = [
            root("a"),
            wt("repo", "key", false),
            child("explicit", "repo"),
            wt("linked", "key", true),
        ];
        assert_eq!(
            moved(&nodes, "repo", WorkspaceMovePlacement::Before("a".into())),
            ["repo", "explicit", "linked", "a"]
        );
    }

    #[test]
    fn root_move_targets_must_be_top_level() {
        let nodes = [root("a"), root("p"), child("c", "p")];
        assert!(
            plan_workspace_move(&nodes, "a", &WorkspaceMovePlacement::Before("c".into())).is_err()
        );
        assert!(
            plan_workspace_move(&nodes, "p", &WorkspaceMovePlacement::Before("c".into())).is_err()
        );
        assert!(
            plan_workspace_move(&nodes, "a", &WorkspaceMovePlacement::Before("gone".into()))
                .is_err()
        );
    }

    #[test]
    fn child_moves_only_among_siblings() {
        let nodes = [
            root("p"),
            child("c1", "p"),
            root("q"),
            child("c2", "p"),
            child("c3", "p"),
            child("other", "q"),
        ];
        assert_eq!(
            moved(&nodes, "c2", WorkspaceMovePlacement::Up),
            ["p", "c2", "c1", "q", "c3", "other"]
        );
        assert_eq!(
            moved(&nodes, "c2", WorkspaceMovePlacement::Down),
            ["p", "c1", "q", "c3", "c2", "other"]
        );
        assert_eq!(
            moved(&nodes, "c3", WorkspaceMovePlacement::Before("c1".into())),
            ["p", "c3", "c1", "q", "c2", "other"]
        );
        assert_eq!(
            moved(&nodes, "c1", WorkspaceMovePlacement::Index(2)),
            ["p", "q", "c2", "c3", "c1", "other"]
        );
        assert!(
            plan_workspace_move(&nodes, "c1", &WorkspaceMovePlacement::Up)
                .unwrap()
                .is_none()
        );
        assert!(plan_workspace_move(
            &nodes,
            "c1",
            &WorkspaceMovePlacement::Before("other".into())
        )
        .is_err());
    }

    #[test]
    fn linked_worktree_child_moves_among_linked_siblings() {
        let nodes = [
            wt("repo", "key", false),
            wt("one", "key", true),
            wt("two", "key", true),
        ];
        assert_eq!(
            moved(&nodes, "two", WorkspaceMovePlacement::Up),
            ["repo", "two", "one"]
        );
    }
}
