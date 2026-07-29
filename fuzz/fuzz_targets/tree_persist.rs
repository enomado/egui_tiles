#![no_main]
//! The saved-layout reader, fuzzed against files it never expected.
//!
//! A stored layout is the one input a tree gets that nobody wrote on purpose: a file from an
//! older version, a file half-written when the machine went down, a file a user edited by
//! hand. Unlike the ops fuzzer, this one does not go through the crate's API at all — it hands
//! the arena straight to `serde`, so it can describe things the API cannot build: a container
//! whose child id does not exist, two containers sharing a child, a cycle, a root that is also
//! someone's child, an active tab that is not in the tab list.
//!
//! What this target asserts, and — just as important — what it deliberately does not:
//!
//! 1. **Nothing may panic.** A layout file is loaded at startup; a panic there is a crash the
//!    user cannot escape without deleting the file by hand.
//! 2. **A loaded tree must survive the normalization every frame begins with, and be
//!    well-formed afterwards.** [`Tree::ui`] starts by garbage-collecting and simplifying, so
//!    `gc` + `simplify` is what any loaded tree meets before it is ever drawn. If damage from a
//!    file is still present after that, the crate is drawing (and mutating) something that is
//!    not a tree.
//! 3. **A save must survive our own reader unchanged**, whatever the tree describes: write,
//!    read back, and the result must equal the original — same tiles, same ids, same shares,
//!    same visibility, same viewport windows — plus the tile-id counter, which `PartialEq` for
//!    `Tiles` deliberately ignores but a restart depends on.
//!
//! What is *not* asserted is that a freshly parsed tree passes [`Tree::validate`]. The reader is
//! `#[derive(Deserialize)]` and repairs nothing by design, so requiring that would be a finding
//! about the very first mutated byte and would hide everything behind it. The question worth
//! fuzzing is the one above: how much of that damage does the crate absorb before it matters?
//!
//! Panes are read as `ron::Value`, so any payload parses and the fuzzer spends its bytes on the
//! layout rather than on inventing a pane type.

use libfuzzer_sys::fuzz_target;

use egui_tiles::{Behavior, SimplificationOptions, Tile, TileId, Tree, UiResponse};

type Pane = ron::Value;

/// Same tree, every pane payload replaced by the same marker.
///
/// The round-trip claim is about the *layout* — which tiles exist, how they nest, their ids,
/// shares, visibility, windows. Whether a pane payload survives serde is a property of the
/// application's own pane type, and here that type is `ron::Value`, which has quirks of its own:
/// it does not preserve a float's width suffix, so a payload written as `inff64` comes back as an
/// f32 infinity and compares unequal. Interesting, but a fact about `ron`, not about this crate —
/// and left in, it would mask every layout finding behind it.
fn layout_only(tree: &Tree<Pane>) -> Tree<Pane> {
    let mut tree = tree.clone();
    for (_id, tile) in tree.tiles.iter_mut() {
        if let Tile::Pane(pane) = tile {
            *pane = ron::Value::Unit;
        }
    }
    tree
}

/// Headless behavior: `gc`/`simplify` only ever consult `retain_pane`, never draw.
struct Headless;

impl Behavior<Pane> for Headless {
    fn tab_title_for_pane(&mut self, _pane: &Pane) -> egui::WidgetText {
        "".into()
    }

    fn pane_ui(&mut self, _ui: &mut egui::Ui, _tile_id: TileId, _pane: &mut Pane) -> UiResponse {
        UiResponse::None
    }
}

/// The counter `Tiles` hands out fresh ids from. Not part of `PartialEq` for `Tiles` (two trees
/// with the same tiles are the same tree), but it *is* part of the file: lose it on a save and
/// the next tile created after a restart collides with a restored one.
fn next_id_after_load(tree: &Tree<Pane>) -> TileId {
    tree.clone().tiles.next_free_id()
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    // A file we refuse to read is not a finding: the contract is about what we *accept*.
    let Ok(tree) = ron::from_str::<Tree<Pane>>(text) else {
        return;
    };

    // --- 3. round-trip, asserted first because it holds for any tree, damaged or not ---------
    //
    // `f32` fields (linear shares, grid row/column shares, the tree's own width/height) can hold
    // a NaN that came from the file, and NaN != NaN makes equality useless as an oracle. A tree
    // that is not even equal to a copy of itself is exactly that case, and nothing else.
    let comparable = layout_only(&tree) == layout_only(&tree);
    if comparable {
        let once = match ron::ser::to_string_pretty(&tree, ron::ser::PrettyConfig::default()) {
            Ok(text) => text,
            Err(error) => panic!("a tree we hold in memory failed to serialize: {error}"),
        };
        let back = match ron::from_str::<Tree<Pane>>(&once) {
            Ok(back) => back,
            Err(error) => panic!("our own output did not parse back: {error}\n{once}"),
        };
        assert!(
            layout_only(&tree) == layout_only(&back),
            "a save/load round-trip changed the tree\nbefore: {tree:#?}\nafter: {back:#?}\ntext:\n{once}"
        );
        assert_eq!(
            next_id_after_load(&tree),
            next_id_after_load(&back),
            "a save/load round-trip moved the next-free-tile-id counter"
        );
    }

    // --- 1 & 2. the normalization a frame begins with ----------------------------------------
    //
    // `Tree::ui` calls `gc` and then `simplify` before laying anything out, so this is the state
    // the crate actually works with. Both are run on a clone: the round-trip above is about the
    // file as stored, this is about what the file becomes.
    let mut normalized = tree.clone();
    normalized.gc(&mut Headless);
    normalized.simplify(&SimplificationOptions::default());

    if let Err(reason) = normalized.validate() {
        panic!(
            "a loaded tree is still ill-formed after gc+simplify: {reason}\nloaded: {tree:#?}\nnormalized: {normalized:#?}"
        );
    }
});
