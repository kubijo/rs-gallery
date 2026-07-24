//! The sidebar tree: scenes grouped under their `scene_meta` titles, the filter that prunes it, and
//! the naming a scene takes from the group it lands in.

use std::collections::BTreeMap;

use crate::{Manifest, SceneEntry, SceneGroupMeta};

/// A node in the sidebar tree: child groups plus the scenes placed directly here.
#[derive(Default)]
pub(crate) struct TreeNode {
    pub(crate) children: BTreeMap<String, TreeNode>,
    pub(crate) scenes: Vec<usize>,
}

/// Build the tree: group titles form the skeleton, then each scene lands under its group (longest
/// `module_path` prefix), or at the root if it declared no group.
pub(crate) fn build_tree(manifest: &Manifest) -> TreeNode {
    let mut tree = TreeNode::default();
    for meta in &manifest.groups {
        node_at(&mut tree, meta.title);
    }
    for (i, scene) in manifest.scenes.iter().enumerate() {
        match longest_group(&manifest.groups, scene.module_path) {
            Some(meta) => node_at(&mut tree, meta.title).scenes.push(i),
            None => tree.scenes.push(i),
        }
    }
    sort_scenes(&mut tree, &manifest.scenes);
    tree
}

/// Sort each node's scenes by `(order, name)` so the catalog is deterministic;
/// inventory registration order is otherwise arbitrary link order.
fn sort_scenes(node: &mut TreeNode, scenes: &[SceneEntry]) {
    node.scenes.sort_by(|&a, &b| {
        (scenes[a].order, scenes[a].name).cmp(&(scenes[b].order, scenes[b].name))
    });
    for child in node.children.values_mut() {
        sort_scenes(child, scenes);
    }
}

/// Walk (creating) the tree to the node named by a slash-separated title.
fn node_at<'a>(tree: &'a mut TreeNode, title: &str) -> &'a mut TreeNode {
    let mut node = tree;
    for part in title.split('/').map(str::trim) {
        node = node.children.entry(part.to_owned()).or_default();
    }
    node
}

/// The group whose `module_path` is the longest prefix of `module_path` (the scene's home group).
fn longest_group<'a>(
    groups: &'a [SceneGroupMeta],
    module_path: &str,
) -> Option<&'a SceneGroupMeta> {
    groups
        .iter()
        .filter(|meta| module_path.starts_with(meta.module_path))
        .max_by_key(|meta| meta.module_path.len())
}

/// Sublime-style fuzzy match for the sidebar filter.
pub(crate) fn fuzzy(text: &str, filter: &str) -> bool {
    sublime_fuzzy::best_match(filter, text).is_some()
}

/// Whether a node's name or anything in its subtree matches the filter.
pub(crate) fn node_matches(
    name: &str,
    node: &TreeNode,
    scenes: &[SceneEntry],
    filter: &str,
) -> bool {
    fuzzy(name, filter)
        || node.scenes.iter().any(|&i| fuzzy(scenes[i].name, filter))
        || node
            .children
            .iter()
            .any(|(child, node)| node_matches(child, node, scenes, filter))
}

/// The visible scenes in render order (for keyboard next/prev), honouring the filter.
pub(crate) fn visible_scenes(
    node: &TreeNode,
    scenes: &[SceneEntry],
    filter: &str,
    ancestor_matched: bool,
    out: &mut Vec<usize>,
) {
    let filtering = !filter.is_empty();
    for (name, child) in &node.children {
        let name_matches = filtering && fuzzy(name, filter);
        if filtering
            && !ancestor_matched
            && !name_matches
            && !node_matches(name, child, scenes, filter)
        {
            continue;
        }
        visible_scenes(child, scenes, filter, ancestor_matched || name_matches, out);
    }
    for &i in &node.scenes {
        if filtering && !ancestor_matched && !fuzzy(scenes[i].name, filter) {
            continue;
        }
        out.push(i);
    }
}

/// The preview heading. A file's default scene IS its `scene_meta` title node, so it shows just the
/// title (e.g. "Components / Greeting"); an additional named scene hangs under it ("… / world").
pub(crate) fn breadcrumb(scene: &SceneEntry, groups: &[SceneGroupMeta]) -> String {
    match longest_group(groups, scene.module_path) {
        Some(group) if scene.default => group.title.to_owned(),
        Some(group) => format!("{} / {}", group.title, scene.name),
        None => scene.name.to_owned(),
    }
}

/// A scene's stable identity for keying selection and persisted knobs — survives reloads and reordering.
pub(crate) fn scene_key(scene: &SceneEntry) -> String {
    format!("{}::{}", scene.module_path, scene.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{group, ordered, scene};

    #[test]
    fn longest_group_picks_the_deepest_matching_prefix() {
        let groups = [group("a", "A"), group("a::b", "A / B")];
        assert_eq!(longest_group(&groups, "a::b::s").unwrap().title, "A / B");
        assert_eq!(longest_group(&groups, "a::x").unwrap().title, "A");
        assert!(longest_group(&groups, "z").is_none());
    }

    #[test]
    fn breadcrumb_is_the_bare_title_for_a_default_scene_and_appends_the_name_otherwise() {
        let groups = [group("m", "MyCar / Map")];
        assert_eq!(
            breadcrumb(&scene("view", "m", true), &groups),
            "MyCar / Map"
        );
        assert_eq!(
            breadcrumb(&scene("aerial", "m", false), &groups),
            "MyCar / Map / aerial"
        );
        assert_eq!(breadcrumb(&scene("loose", "x", false), &[]), "loose");
    }

    #[test]
    fn scene_key_joins_module_path_and_name() {
        assert_eq!(scene_key(&scene("map", "app::map", true)), "app::map::map");
    }

    #[test]
    fn build_tree_nests_each_scene_under_its_title_path() {
        let manifest = Manifest {
            scenes: vec![scene("view", "m", true), scene("dash", "d", true)],
            groups: vec![group("m", "MyCar / Map"), group("d", "MyCar / Dashboard")],
        };
        let tree = build_tree(&manifest);
        let mycar = &tree.children["MyCar"];
        assert_eq!(mycar.children["Map"].scenes, vec![0]);
        assert_eq!(mycar.children["Dashboard"].scenes, vec![1]);
    }

    #[test]
    fn an_ungrouped_scene_lands_at_the_root() {
        let manifest = Manifest {
            scenes: vec![scene("loose", "x", true)],
            groups: vec![],
        };
        let tree = build_tree(&manifest);
        assert_eq!(tree.scenes, vec![0]);
        assert!(tree.children.is_empty());
    }

    #[test]
    fn build_tree_sorts_scenes_by_order_then_name() {
        // Registration (link) order is deliberately not the wanted order.
        let manifest = Manifest {
            scenes: vec![
                ordered("beta", 10),
                ordered("alpha", 10),
                ordered("first", 0),
            ],
            groups: vec![group("m", "Group")],
        };
        let node = &build_tree(&manifest).children["Group"];
        // order 0 leads; the order-10 tie breaks by name (alpha before beta).
        assert_eq!(node.scenes, vec![2, 1, 0]);
    }

    #[test]
    fn fuzzy_matches_subsequences_only() {
        assert!(fuzzy("Dashboard", "Dashboard"));
        assert!(fuzzy("Dashboard", "Dash"));
        assert!(!fuzzy("Dashboard", "zzz"));
    }

    #[test]
    fn node_matches_own_name_scenes_and_descendants() {
        let manifest = Manifest {
            scenes: vec![scene("view", "m", true)],
            groups: vec![group("m", "MyCar / Map")],
        };
        let tree = build_tree(&manifest);
        let mycar = &tree.children["MyCar"];
        assert!(node_matches("MyCar", mycar, &manifest.scenes, "Map"));
        assert!(node_matches("MyCar", mycar, &manifest.scenes, "Car"));
        assert!(!node_matches("MyCar", mycar, &manifest.scenes, "zzz"));
    }

    #[test]
    fn visible_scenes_lists_them_all_when_unfiltered() {
        let manifest = Manifest {
            scenes: vec![scene("view", "m", true), scene("dash", "d", true)],
            groups: vec![group("m", "MyCar / Map"), group("d", "MyCar / Dashboard")],
        };
        let tree = build_tree(&manifest);
        let mut out = Vec::new();
        visible_scenes(&tree, &manifest.scenes, "", false, &mut out);
        assert_eq!(out.len(), 2);
    }
}
