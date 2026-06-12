//! Tests for detached-into-viewport tiles (floating native windows).
//!
//! These exercise the *data-model* invariants of `viewport_tiles` — detach,
//! multi-root `roots()`, and multi-root `simplify` — without a real `egui::Context`
//! (rendering is covered by the `viewport` example and manual/MCP soak later).

#![cfg(feature = "serde")]

use egui_tiles::{SimplificationOptions, Tile, TileId, Tiles, Tree};

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Pane {
    nr: usize,
}

fn child_ids(tree: &Tree<Pane>, container: TileId) -> Vec<TileId> {
    match tree.tiles.get(container) {
        Some(Tile::Container(c)) => c.children_vec(),
        other => panic!("expected a container at {container:?}, got {other:?}"),
    }
}

/// Detaching a tile removes it from its parent and turns it into a viewport root,
/// while leaving the main root intact.
#[test]
fn detach_removes_from_parent_and_adds_viewport_root() {
    let mut tiles = Tiles::default();
    let a = tiles.insert_pane(Pane { nr: 0 });
    let b = tiles.insert_pane(Pane { nr: 1 });
    let c = tiles.insert_pane(Pane { nr: 2 });
    let main_root = tiles.insert_tab_tile(vec![a, b, c]);
    let mut tree = Tree::new("t", main_root, tiles);

    tree.move_tile_to_new_viewport(b, egui::pos2(100.0, 100.0));

    // `b` left its parent…
    assert_eq!(child_ids(&tree, main_root), vec![a, c]);
    // …and became an independent root.
    assert!(tree.is_viewport_root(b));
    assert_eq!(tree.root(), Some(main_root));
    let roots = tree.roots();
    assert_eq!(roots.len(), 2);
    assert!(roots.contains(&b));
    assert!(roots.contains(&main_root));
    assert!(!tree.is_empty());

    // The `Tile` itself is retained (just re-parented to "no parent / window root").
    assert!(tree.tiles.get(b).is_some());

    // Detaching an already-detached tile is a no-op.
    tree.move_tile_to_new_viewport(b, egui::pos2(200.0, 200.0));
    assert_eq!(tree.roots().len(), 2);
}

/// TEETH: a `Replace` produced while simplifying a *viewport* root must rewrite only
/// that viewport's root — never `self.root`. The naive POC pushed every `Replace` into
/// `self.root`, which would clobber the main tree with a window's new root id.
#[test]
fn simplify_viewport_root_does_not_clobber_main_root() {
    let mut tiles = Tiles::default();
    // Main root has TWO tabs, so it is itself not prunable.
    let m1 = tiles.insert_pane(Pane { nr: 0 });
    let m2 = tiles.insert_pane(Pane { nr: 1 });
    let main_root = tiles.insert_tab_tile(vec![m1, m2]);
    // A single-child vertical container — `prune_single_child_containers` collapses it.
    let p = tiles.insert_pane(Pane { nr: 2 });
    let single = tiles.insert_vertical_tile(vec![p]);
    let mut tree = Tree::new("t", main_root, tiles);

    tree.move_tile_to_new_viewport(single, egui::pos2(0.0, 0.0));
    assert!(tree.is_viewport_root(single));

    // Default options prune single-child containers but do NOT force panes into tabs.
    let opts = SimplificationOptions {
        all_panes_must_have_tabs: false,
        ..SimplificationOptions::default()
    };
    tree.simplify(&opts);

    // Main root is untouched (the bug would set it to `p`).
    assert_eq!(tree.root(), Some(main_root));
    assert_eq!(child_ids(&tree, main_root), vec![m1, m2]);
    // The viewport root was replaced by the collapsed child.
    assert!(tree.is_viewport_root(p));
    assert!(!tree.is_viewport_root(single));
    assert_eq!(tree.roots().len(), 2);
}

/// A tree carrying detached viewports round-trips through serde unchanged.
#[test]
fn serde_round_trip_with_viewports() {
    let mut tiles = Tiles::default();
    let a = tiles.insert_pane(Pane { nr: 0 });
    let b = tiles.insert_pane(Pane { nr: 1 });
    let main_root = tiles.insert_tab_tile(vec![a, b]);
    let detached = tiles.insert_pane(Pane { nr: 2 });
    let mut tree = Tree::new("t", main_root, tiles);
    tree.move_tile_to_new_viewport(detached, egui::pos2(42.0, 17.0));

    let json = serde_json::to_string(&tree).expect("json serialize");
    let restored: Tree<Pane> = serde_json::from_str(&json).expect("json deserialize");
    assert_eq!(tree, restored, "JSON did not round-trip");
    assert!(restored.is_viewport_root(detached));

    let ron = ron::to_string(&tree).expect("ron serialize");
    let restored: Tree<Pane> = ron::from_str(&ron).expect("ron deserialize");
    assert_eq!(tree, restored, "RON did not round-trip");
    assert!(restored.is_viewport_root(detached));
}
