//! Proof-grade serde round-trip tests for *runtime-mutated* (user-defined) layouts.
//!
//! The integration tests in `tests/serialize.rs` and `tests/viewport.rs` round-trip
//! freshly *built* trees. Real applications persist a tree the user has been *editing*
//! — panes added and removed, tiles detached into their own windows and re-docked,
//! tabs re-activated, the tree simplified. These tests evolve a tree through that
//! public-API mutation surface and then assert the saved layout deserializes back to
//! *exactly* the same structure, is internally well-formed, and is safe to keep
//! editing (the id counter persisted, so new inserts cannot collide).
//!
//! They live inside the crate (rather than in `tests/`) because re-docking exercises
//! [`InsertionPoint`], whose constructor is crate-private.

use crate::{
    Behavior, Container, ContainerInsertion, InsertionPoint, SimplificationOptions, Tile, TileId,
    Tiles, Tree, UiResponse,
};

/// A non-trivial pane payload: an enum with several shapes and an owned `String`,
/// exercising the generic `Pane: Serialize` path with realistic application data
/// rather than a single `usize`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum AppPane {
    Empty,
    Counter(u64),
    Named { title: String, dirty: bool },
}

/// Minimal behavior so we can drive [`Tree::gc`], which (unlike `simplify`) needs one.
struct NoopBehavior;

impl Behavior<AppPane> for NoopBehavior {
    fn pane_ui(&mut self, _ui: &mut egui::Ui, _tile_id: TileId, _pane: &mut AppPane) -> UiResponse {
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, _pane: &AppPane) -> egui::WidgetText {
        "pane".into()
    }
}

/// Build a tree and put it through a representative mix of the operations a user
/// triggers while editing a layout, returning the evolved tree.
///
/// The result intentionally has a sparse id space: tiles have been removed, so the
/// internal id counter sits *above* the highest live id. That gap is what makes the
/// `next_tile_id` round-trip observable (see [`keeps_editing_safe_after_round_trip`]).
fn build_mutated_tree() -> Tree<AppPane> {
    let mut tiles = Tiles::default();

    // A horizontal split of two tabs, plus a grid — a typical starting layout.
    let a = tiles.insert_pane(AppPane::Counter(0));
    let b = tiles.insert_pane(AppPane::Named {
        title: "editor".to_owned(),
        dirty: true,
    });
    let left = tiles.insert_tab_tile(vec![a, b]);

    let c = tiles.insert_pane(AppPane::Empty);
    let right = tiles.insert_tab_tile(vec![c]);

    let g0 = tiles.insert_pane(AppPane::Counter(7));
    let g1 = tiles.insert_pane(AppPane::Counter(8));
    let g2 = tiles.insert_pane(AppPane::Counter(9));
    let grid = tiles.insert_grid_tile(vec![g0, g1, g2]);

    let top = tiles.insert_horizontal_tile(vec![left, right]);
    let root = tiles.insert_tab_tile(vec![top, grid]);
    let mut tree = Tree::new("mutated", root, tiles);

    // --- user edits ---

    // Add a brand-new pane into the `right` tab container.
    let d = tree.tiles.insert_pane(AppPane::Named {
        title: "log".to_owned(),
        dirty: false,
    });
    tree.move_tile_to_container(d, right, 1, false);

    // Insert a fresh vertical split as a new tab of the root.
    let e = tree.tiles.insert_pane(AppPane::Counter(42));
    let f = tree.tiles.insert_pane(AppPane::Empty);
    let vsplit = tree.tiles.insert_vertical_tile(vec![e, f]);
    tree.move_tile_to_container(vsplit, root, 2, false);

    // Detach a pane into its own window, then re-dock a *different* one back —
    // exercising both the detach and the re-dock paths and leaving one live viewport.
    tree.move_tile_to_new_viewport(g2, egui::pos2(120.0, 80.0));
    tree.move_tile_to_new_viewport(b, egui::pos2(300.0, 200.0));
    tree.dock_viewport_back(b, InsertionPoint::new(left, ContainerInsertion::Tabs(0)));

    // Remove a pane outright (leaves a hole in the id space; `gc` reclaims the tile).
    tree.remove_recursively(f);

    // Re-activate a specific tab so the persisted `active` is non-default.
    tree.make_active(|_id, tile| matches!(tile, Tile::Pane(AppPane::Counter(42))));

    // Clean the tree up the way the per-frame `ui` pass would.
    let mut behavior = NoopBehavior;
    tree.simplify(&SimplificationOptions::default());
    tree.gc(&mut behavior);

    // Sanity: the tree we are about to round-trip is itself well-formed.
    assert_eq!(tree.validate(), Ok(()), "fixture tree is not well-formed");
    tree
}

/// A runtime-mutated tree round-trips through JSON and RON to a structurally
/// identical, internally-consistent tree.
#[test]
fn mutated_tree_round_trips_json_and_ron() {
    let original = build_mutated_tree();

    let json = serde_json::to_string(&original).expect("json serialize");
    let restored: Tree<AppPane> = serde_json::from_str(&json).expect("json deserialize");
    assert_eq!(
        original, restored,
        "mutated tree did not round-trip via JSON"
    );
    assert_eq!(
        restored.validate(),
        Ok(()),
        "JSON-restored tree fails its structural invariants"
    );

    let ron = ron::to_string(&original).expect("ron serialize");
    let restored: Tree<AppPane> = ron::from_str(&ron).expect("ron deserialize");
    assert_eq!(
        original, restored,
        "mutated tree did not round-trip via RON"
    );
    assert_eq!(
        restored.validate(),
        Ok(()),
        "RON-restored tree fails its structural invariants"
    );
}

/// The id counter survives the round-trip, so a layout can keep being edited after
/// being saved and reloaded: the next inserted tile gets a fresh id that does not
/// collide with any existing tile, and `validate()` still passes.
///
/// This pins the value of `Tiles::next_tile_id` across serde. Because the fixture has
/// a sparse id space (tiles were removed), the *exact* id handed out after a restore
/// differs depending on whether the counter was persisted or reset — which is what
/// makes a regression here observable rather than masked by the collision-skip in
/// `Tiles::next_free_id`.
#[test]
fn keeps_editing_safe_after_round_trip() {
    let original = build_mutated_tree();

    // What id does the *original* tree hand out next? Clone so we don't perturb it.
    let expected_next_id = original.tiles.clone().next_free_id();
    let existing_ids: std::collections::HashSet<TileId> = original.tiles.tile_ids().collect();

    for label in ["json", "ron"] {
        let mut restored: Tree<AppPane> = if label == "json" {
            let s = serde_json::to_string(&original).expect("json serialize");
            serde_json::from_str(&s).expect("json deserialize")
        } else {
            let s = ron::to_string(&original).expect("ron serialize");
            ron::from_str(&s).expect("ron deserialize")
        };

        // Inserting after a restore must produce a *fresh* id, never aliasing a tile
        // already present in the restored tree.
        let new_id = restored.tiles.insert_pane(AppPane::Counter(1000));
        assert!(
            !existing_ids.contains(&new_id),
            "{label}: post-restore insert reused existing id {new_id:?}"
        );

        // …and it must match the id the original tree would have handed out, proving
        // the counter value itself round-tripped (not just that the collision guard
        // papered over a reset).
        assert_eq!(
            new_id, expected_next_id,
            "{label}: restored id counter diverged from the original"
        );

        // The fresh pane has to be attached for the tree to stay well-formed.
        let root = restored.root().expect("restored tree has a root");
        restored.move_tile_to_container(new_id, root, usize::MAX, false);
        assert_eq!(
            restored.validate(),
            Ok(()),
            "{label}: tree is not well-formed after a post-restore insert"
        );
    }
}

/// A tree carrying a still-detached viewport that was *also* re-docked-and-redetached
/// round-trips with its viewport set and positions intact. Complements
/// `tests/viewport.rs`, which covers a single never-re-docked viewport.
#[test]
fn detached_then_redocked_tree_round_trips() {
    let mut tiles = Tiles::default();
    let a = tiles.insert_pane(AppPane::Counter(0));
    let b = tiles.insert_pane(AppPane::Empty);
    let c = tiles.insert_pane(AppPane::Named {
        title: "side".to_owned(),
        dirty: false,
    });
    let root = tiles.insert_tab_tile(vec![a, b, c]);
    let mut tree = Tree::new("redock", root, tiles);

    // Detach two, re-dock one, leave one detached at a known position.
    tree.move_tile_to_new_viewport(b, egui::pos2(10.0, 20.0));
    tree.move_tile_to_new_viewport(c, egui::pos2(30.0, 40.0));
    tree.dock_viewport_back(
        c,
        InsertionPoint::new(root, ContainerInsertion::Tabs(usize::MAX)),
    );

    assert!(tree.is_viewport_root(b));
    assert!(!tree.is_viewport_root(c));
    assert_eq!(tree.validate(), Ok(()));

    let json = serde_json::to_string(&tree).expect("json serialize");
    let restored: Tree<AppPane> = serde_json::from_str(&json).expect("json deserialize");
    assert_eq!(
        tree, restored,
        "detach/redock tree did not round-trip via JSON"
    );
    assert!(restored.is_viewport_root(b));
    assert_eq!(restored.validate(), Ok(()));
    // The redocked pane is back in the main root.
    match restored.tiles.get(root) {
        Some(Tile::Container(Container::Tabs(tabs))) => assert!(tabs.children.contains(&c)),
        other => panic!("expected tabs root, got {other:?}"),
    }

    let ron = ron::to_string(&tree).expect("ron serialize");
    let restored: Tree<AppPane> = ron::from_str(&ron).expect("ron deserialize");
    assert_eq!(
        tree, restored,
        "detach/redock tree did not round-trip via RON"
    );
    assert!(restored.is_viewport_root(b));
    assert_eq!(restored.validate(), Ok(()));
}
