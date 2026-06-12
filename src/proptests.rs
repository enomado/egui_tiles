//! Property tests for the `Tiles`/`Tree` arena, including detached viewport roots.
//!
//! A random sequence of structural operations is applied to a tree; after every op the
//! [`Tree::validate`] oracle must pass, surviving pane `TileId`s must keep their payload
//! (issue #10 — ids must not silently rebind), and *non-destructive* ops must never drop a
//! pane. Lives inside the crate (not `tests/`) so it can drive moves via the crate-private
//! `move_tile` / `InsertionPoint`.

use std::collections::{HashMap, HashSet};

use proptest::prelude::*;

use crate::{
    Behavior, Container, ContainerInsertion, InsertionPoint, SimplificationOptions, Tile, TileId,
    Tiles, Tree, UiResponse, ViewportTile,
};

/// Minimal behavior so `gc`/`simplify` (which only touch `retain_pane`) can run headless.
struct NoopBehavior;

impl Behavior<u32> for NoopBehavior {
    fn tab_title_for_pane(&mut self, _pane: &u32) -> egui::WidgetText {
        "".into()
    }

    fn pane_ui(&mut self, _ui: &mut egui::Ui, _tile_id: TileId, _pane: &mut u32) -> UiResponse {
        UiResponse::None
    }
}

#[derive(Debug, Clone)]
enum Op {
    /// Insert a fresh pane and append it under the main root (only if the root is a `Tabs`).
    InsertPane,
    /// Detach the i-th non-root tile into its own viewport.
    Detach(usize),
    /// Dock the i-th viewport root back under the main root (only if the root is a `Tabs`).
    DockBack(usize),
    /// Make the i-th tile active (reveal it in any enclosing tab bars).
    MakeActive(usize),
    /// Prune/normalize with default options.
    Simplify,
    /// Garbage-collect unreachable tiles.
    Gc,
    /// Recursively remove the i-th non-root tile.
    Remove(usize),
    /// Move the i-th non-root tile into the j-th container tile. Deliberately
    /// allows the destination to lie *inside* the moved subtree (the cycle case
    /// that `move_tile`'s self-or-descendant guard must reject as a no-op).
    MoveTile(usize, usize),
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        Just(Op::InsertPane),
        any::<u16>().prop_map(|i| Op::Detach(i as usize)),
        any::<u16>().prop_map(|i| Op::DockBack(i as usize)),
        any::<u16>().prop_map(|i| Op::MakeActive(i as usize)),
        Just(Op::Simplify),
        Just(Op::Gc),
        any::<u16>().prop_map(|i| Op::Remove(i as usize)),
        (any::<u16>(), any::<u16>()).prop_map(|(i, j)| Op::MoveTile(i as usize, j as usize)),
    ]
}

/// Pane tile ids, in deterministic (sorted) order.
fn pane_tiles(tree: &Tree<u32>) -> Vec<TileId> {
    let mut v: Vec<TileId> = tree
        .tiles
        .tile_ids()
        .filter(|id| matches!(tree.tiles.get(*id), Some(Tile::Pane(_))))
        .collect();
    v.sort_by_key(|t| t.0);
    v
}

/// Deterministic ordering of tile ids (`HashMap` iteration order is not stable across runs).
fn sorted_ids(tree: &Tree<u32>) -> Vec<TileId> {
    let mut v: Vec<TileId> = tree.tiles.tile_ids().collect();
    v.sort_by_key(|t| t.0);
    v
}

fn pick_non_root(tree: &Tree<u32>, i: usize) -> Option<TileId> {
    let roots: HashSet<TileId> = tree.roots().into_iter().collect();
    let candidates: Vec<TileId> = sorted_ids(tree)
        .into_iter()
        .filter(|id| !roots.contains(id))
        .collect();
    (!candidates.is_empty()).then(|| candidates[i % candidates.len()])
}

fn pick_any(tree: &Tree<u32>, i: usize) -> Option<TileId> {
    let ids = sorted_ids(tree);
    (!ids.is_empty()).then(|| ids[i % ids.len()])
}

/// The i-th container tile (any kind), in deterministic order. Move destinations
/// must be containers — `move_tile_to_container` rejects panes.
fn pick_container(tree: &Tree<u32>, i: usize) -> Option<TileId> {
    let containers: Vec<TileId> = sorted_ids(tree)
        .into_iter()
        .filter(|id| matches!(tree.tiles.get(*id), Some(Tile::Container(_))))
        .collect();
    (!containers.is_empty()).then(|| containers[i % containers.len()])
}

/// `(root, child_count)` iff the main root is a `Tabs` container we can append into.
fn tabs_root(tree: &Tree<u32>) -> Option<(TileId, usize)> {
    let r = tree.root()?;
    match tree.tiles.get(r) {
        Some(Tile::Container(Container::Tabs(tabs))) => Some((r, tabs.children.len())),
        _ => None,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn tiles_invariants_hold(ops in prop::collection::vec(op_strategy(), 0..60)) {
        let mut next_pane: u32 = 0;
        let mut payloads: HashMap<TileId, u32> = HashMap::new();
        let mut behavior = NoopBehavior;

        // Seed: a 2-tab root (so it is not itself prunable straight away).
        let mut tiles = Tiles::default();
        let fresh = |tiles: &mut Tiles<u32>, payloads: &mut HashMap<TileId, u32>, next: &mut u32| {
            let p = *next;
            *next += 1;
            let id = tiles.insert_pane(p);
            payloads.insert(id, p);
            id
        };
        let a = fresh(&mut tiles, &mut payloads, &mut next_pane);
        let b = fresh(&mut tiles, &mut payloads, &mut next_pane);
        let root = tiles.insert_tab_tile(vec![a, b]);
        let mut tree = Tree::new("proptest_tree", root, tiles);

        prop_assert!(tree.validate().is_ok(), "seed invalid: {:?}", tree.validate());

        for op in ops {
            let panes_before = pane_tiles(&tree);

            match op {
                Op::InsertPane => {
                    if let Some((r, n)) = tabs_root(&tree) {
                        let p = next_pane;
                        next_pane += 1;
                        let id = tree.tiles.insert_pane(p);
                        payloads.insert(id, p);
                        tree.move_tile(id, InsertionPoint::new(r, ContainerInsertion::Tabs(n)), false);
                    }
                }
                Op::Detach(i) => {
                    if let Some(id) = pick_non_root(&tree, i) {
                        tree.move_tile_to_new_viewport(id, egui::pos2(10.0, 10.0));
                    }
                }
                Op::DockBack(i) => {
                    let vp_roots: Vec<TileId> =
                        tree.viewport_tiles.iter().map(|v| v.root).collect();
                    if !vp_roots.is_empty()
                        && let Some((r, n)) = tabs_root(&tree)
                    {
                        let id = vp_roots[i % vp_roots.len()];
                        tree.dock_viewport_back(
                            id,
                            InsertionPoint::new(r, ContainerInsertion::Tabs(n)),
                        );
                    }
                }
                Op::MakeActive(i) => {
                    if let Some(id) = pick_any(&tree, i) {
                        tree.make_active(|tid, _| tid == id);
                    }
                }
                Op::Simplify => tree.simplify(&SimplificationOptions::default()),
                Op::Gc => tree.gc(&mut behavior),
                Op::Remove(i) => {
                    if let Some(id) = pick_non_root(&tree, i) {
                        tree.remove_recursively(id);
                    }
                }
                Op::MoveTile(i, j) => {
                    if let (Some(src), Some(dest)) =
                        (pick_non_root(&tree, i), pick_container(&tree, j))
                    {
                        tree.move_tile_to_container(src, dest, 0, false);
                    }
                }
            }

            // (1) Structural oracle.
            if let Err(e) = tree.validate() {
                return Err(TestCaseError::fail(format!("validate failed after {op:?}: {e}")));
            }

            // (2) TileId stability (issue #10): a surviving pane id keeps its payload.
            for tid in sorted_ids(&tree) {
                if let (Some(want), Some(Tile::Pane(got))) =
                    (payloads.get(&tid), tree.tiles.get(tid))
                {
                    prop_assert_eq!(got, want, "TileId {:?} rebound after {:?}", tid, op);
                }
            }

            // (3) Non-destructive ops must not lose any pane tile.
            if matches!(
                op,
                Op::Simplify
                    | Op::Gc
                    | Op::Detach(_)
                    | Op::DockBack(_)
                    | Op::MakeActive(_)
                    | Op::MoveTile(_, _)
            ) {
                let panes_after = pane_tiles(&tree);
                for p in &panes_before {
                    prop_assert!(
                        panes_after.contains(p),
                        "pane {:?} lost after {:?}",
                        p,
                        op
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// move_tile cycle guard: dropping a container onto something it contains must be
// a no-op, not a silent subtree-eating cycle.
// ---------------------------------------------------------------------------

/// Moving a container into one of its own descendants used to splice the new
/// parent inside the moved subtree, forming a cycle that `gc` then "repaired"
/// by deleting the whole subtree — silently destroying every pane in it.
/// The self-or-descendant guard must reject the move (no-op): tree unchanged,
/// every pane intact, oracle still green.
#[test]
fn move_into_own_descendant_is_rejected() {
    let mut tiles = Tiles::default();
    let a = tiles.insert_pane(0);
    let b = tiles.insert_pane(1);
    let inner = tiles.insert_tab_tile(vec![a, b]); // a container we'll try to misplace
    let c = tiles.insert_pane(2);
    let outer = tiles.insert_horizontal_tile(vec![inner, c]);
    let mut tree = Tree::new("t", outer, tiles);

    let panes_before = pane_tiles(&tree);
    let snapshot = format!("{tree:?}");

    // Try to move `outer` (the root) into `inner`, which is *inside* `outer`.
    tree.move_tile_to_container(outer, inner, 0, false);
    // And `inner` into itself.
    tree.move_tile_to_container(inner, inner, 0, false);

    // gc would be where the loss surfaced — run it to prove nothing was orphaned.
    tree.gc(&mut NoopBehavior);

    assert_eq!(tree.validate(), Ok(()), "tree corrupted by rejected move");
    assert_eq!(
        pane_tiles(&tree),
        panes_before,
        "panes lost: move-into-descendant was not a no-op"
    );
    assert_eq!(format!("{tree:?}"), snapshot, "tree mutated by rejected move");
    // The three original panes are all still reachable.
    for p in [a, b, c] {
        assert!(matches!(tree.tiles.get(p), Some(Tile::Pane(_))), "pane {p:?} vanished");
    }
}

/// A *legitimate* move (into a container that is not in the moved subtree) still
/// works — the guard rejects only the cyclic case, not all moves.
#[test]
fn move_into_unrelated_container_still_works() {
    let mut tiles = Tiles::default();
    let a = tiles.insert_pane(0);
    let left = tiles.insert_tab_tile(vec![a]);
    let b = tiles.insert_pane(1);
    let right = tiles.insert_tab_tile(vec![b]);
    let root = tiles.insert_horizontal_tile(vec![left, right]);
    let mut tree = Tree::new("t", root, tiles);

    tree.move_tile_to_container(a, right, 0, false);

    assert_eq!(tree.validate(), Ok(()));
    let right_children = match tree.tiles.get(right) {
        Some(Tile::Container(c)) => c.children_vec(),
        other => panic!("expected container, got {other:?}"),
    };
    assert!(right_children.contains(&a), "legit move did not land");
}

// ---------------------------------------------------------------------------
// Teeth: each `validate()` branch must actually fire on a deliberately broken tree.
// ---------------------------------------------------------------------------

#[test]
fn validate_catches_orphan_tile() {
    let mut tiles = Tiles::default();
    let a = tiles.insert_pane(0);
    let root = tiles.insert_tab_tile(vec![a]);
    let _orphan = tiles.insert_pane(99); // never attached to any root
    let tree = Tree::new("t", root, tiles);
    let err = tree.validate().unwrap_err();
    assert!(err.contains("orphan"), "got: {err}");
}

#[test]
fn validate_catches_missing_child() {
    let mut tiles = Tiles::default();
    let a = tiles.insert_pane(0);
    let root = tiles.insert_tab_tile(vec![a]);
    tiles.remove(a); // root still lists `a`, but it's gone
    let tree = Tree::new("t", root, tiles);
    let err = tree.validate().unwrap_err();
    assert!(err.contains("missing child"), "got: {err}");
}

#[test]
fn validate_catches_shared_child() {
    let mut tiles = Tiles::default();
    let a = tiles.insert_pane(0);
    let t1 = tiles.insert_tab_tile(vec![a]);
    let t2 = tiles.insert_tab_tile(vec![a]); // `a` is a child of two containers
    let root = tiles.insert_horizontal_tile(vec![t1, t2]);
    let tree = Tree::new("t", root, tiles);
    let err = tree.validate().unwrap_err();
    assert!(err.contains("child of both"), "got: {err}");
}

#[test]
fn validate_catches_active_not_a_child() {
    let mut tiles = Tiles::default();
    let a = tiles.insert_pane(0);
    let b = tiles.insert_pane(1);
    let tabs = tiles.insert_tab_tile(vec![a]);
    let root = tiles.insert_horizontal_tile(vec![tabs, b]);
    // Point the tab container's active tab at `b`, which is not one of its children.
    if let Some(Tile::Container(Container::Tabs(t))) = tiles.get_mut(tabs) {
        t.active = Some(b);
    }
    let tree = Tree::new("t", root, tiles);
    let err = tree.validate().unwrap_err();
    assert!(err.contains("active tab"), "got: {err}");
}

#[test]
fn validate_catches_root_that_is_also_a_child() {
    let mut tiles = Tiles::default();
    let a = tiles.insert_pane(0);
    let inner = tiles.insert_tab_tile(vec![a]);
    let root = tiles.insert_horizontal_tile(vec![inner]);
    let mut tree = Tree::new("t", root, tiles);
    // Mark `inner` as a viewport root while it is still a child of `root`.
    tree.viewport_tiles.push(ViewportTile {
        root: inner,
        screen_pos: egui::pos2(0.0, 0.0),
        dragged: false,
    });
    let err = tree.validate().unwrap_err();
    assert!(err.contains("is also a child"), "got: {err}");
}

/// A well-formed tree (incl. a detached viewport) passes the oracle.
#[test]
fn validate_accepts_well_formed_tree_with_viewport() {
    let mut tiles = Tiles::default();
    let a = tiles.insert_pane(0);
    let b = tiles.insert_pane(1);
    let root = tiles.insert_tab_tile(vec![a, b]);
    let detached = tiles.insert_pane(2);
    let mut tree = Tree::new("t", root, tiles);
    tree.move_tile_to_new_viewport(detached, egui::pos2(0.0, 0.0));
    assert_eq!(tree.validate(), Ok(()));
}
