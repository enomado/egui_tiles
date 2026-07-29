//! Builds the seed corpus for the `tree_persist` fuzz target.
//!
//! Two sources, and both are needed:
//!
//! * **real saved layouts** — files a downstream application actually wrote. They carry the
//!   shapes that occur in practice (deep nesting, tab bars with a dozen panes, the exact float
//!   formatting of shares) and no generator would think to produce them. A layout file usually
//!   wraps the tree in a struct of its own, so the tool lifts a `tree` field when it finds one
//!   and takes the file whole otherwise.
//! * **synthetic trees built through the crate's own API** — because the real corpus is thin
//!   (the application that vendors this fork keeps a single autosave plus a dump), and because
//!   whole regions of the format never appear in it: grids with holes, detached viewport
//!   windows, invisible tiles, finite tree width/height, an empty tree. Seeding without them
//!   means the fuzzer must invent those field names byte by byte, which it will not do.
//!
//! Every seed is parsed back before it is written, and a source file that cannot be turned into
//! a seed fails the run: a silently empty corpus looks exactly like a full one from the outside,
//! and then "seeded with real layouts" is a claim about nothing.
//!
//! ```sh
//! cargo run --manifest-path fuzz/corpus_tool/Cargo.toml -- fuzz/seeds/tree_persist [files…]
//! ```

use std::path::{Path, PathBuf};

use egui_tiles::{Tile, TileId, Tiles, Tree};

/// Panes are opaque to the fuzzer: it should spend its bytes on the layout, not on inventing a
/// pane type. Real payloads are replaced with a short marker for the same reason — and because
/// this fork is public, while the layouts we harvest from are not.
type Pane = ron::Value;

fn opaque_pane(n: usize) -> Pane {
    ron::Value::String(format!("p{n}"))
}

/// A layout file usually stores the tree inside a struct of its own; unknown sibling fields are
/// ignored by serde, so this lifts the tree out of any such wrapper.
#[derive(serde::Deserialize)]
struct Wrapper {
    tree: Tree<Pane>,
}

fn write_seed(out_dir: &Path, name: &str, tree: &Tree<Pane>) -> Result<(), String> {
    let text = ron::ser::to_string_pretty(tree, ron::ser::PrettyConfig::default())
        .map_err(|e| format!("{name}: serializing failed: {e}"))?;

    // A seed that does not parse back teaches the fuzzer nothing but our own bug, and it would
    // do so from inside the corpus where it is easy to mistake for a finding.
    let reparsed: Tree<Pane> =
        ron::from_str(&text).map_err(|e| format!("{name}: our own output did not parse: {e}"))?;
    let health = match reparsed.validate() {
        Ok(()) => "valid",
        // Not fatal: a *real* file that fails the oracle is precisely the kind of input this
        // corpus exists to carry. It is reported so it cannot pass unnoticed.
        Err(reason) => {
            println!("  ! {name} does not satisfy the oracle: {reason}");
            "ILL-FORMED"
        }
    };

    let path = out_dir.join(name);
    std::fs::write(&path, text.as_bytes()).map_err(|e| format!("{name}: writing failed: {e}"))?;
    println!("  {name}: {} tiles, {health}", reparsed.tiles.len());
    Ok(())
}

/// Replace every pane payload with a short marker, in place.
fn make_panes_opaque(tree: &mut Tree<Pane>) {
    for (index, (_id, tile)) in tree.tiles.iter_mut().enumerate() {
        if let Tile::Pane(pane) = tile {
            *pane = opaque_pane(index);
        }
    }
}

fn harvest(path: &Path, out_dir: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path:?}: {e}"))?;

    let mut tree = match ron::from_str::<Wrapper>(&text) {
        Ok(wrapper) => wrapper.tree,
        // Not a wrapper — maybe the file is a bare tree.
        Err(wrapper_error) => ron::from_str::<Tree<Pane>>(&text).map_err(|bare_error| {
            format!(
                "{path:?}: neither a wrapper with a `tree` field ({wrapper_error}) nor a bare tree ({bare_error})"
            )
        })?,
    };
    make_panes_opaque(&mut tree);

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{path:?}: unusable file name"))?;
    write_seed(out_dir, name, &tree)
}

// ----------------------------------------------------------------------------------------
// Synthetic trees: one per region of the format that the harvested files do not reach.

fn tree_of(name: &'static str, root: Option<TileId>, tiles: Tiles<Pane>) -> Tree<Pane> {
    match root {
        Some(root) => Tree::new(name, root, tiles),
        None => Tree::empty(name),
    }
}

/// Tabs holding panes, with an explicit active tab.
fn generated_tabs() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let panes: Vec<TileId> = (0..4).map(|i| tiles.insert_pane(opaque_pane(i))).collect();
    let root = tiles.insert_tab_tile(panes);
    tree_of("tabs", Some(root), tiles)
}

/// Nested linear containers with shares that are not all equal — the arithmetic a resize writes.
fn generated_linear_nested() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let a = tiles.insert_pane(opaque_pane(0));
    let b = tiles.insert_pane(opaque_pane(1));
    let c = tiles.insert_pane(opaque_pane(2));
    let inner = tiles.insert_vertical_tile(vec![b, c]);
    let root = tiles.insert_horizontal_tile(vec![a, inner]);

    if let Some(egui_tiles::Tile::Container(egui_tiles::Container::Linear(linear))) =
        tiles.get_mut(root)
    {
        linear.shares.set_share(a, 0.3);
        linear.shares.set_share(inner, 2.7);
    }
    tree_of("linear", Some(root), tiles)
}

/// A grid *with a hole*: dragging a tile out of a grid leaves the cell empty on purpose, and the
/// hole is part of the stored format (`children: [Some(#1), None, …]`).
fn generated_grid_with_hole() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let panes: Vec<TileId> = (0..4).map(|i| tiles.insert_pane(opaque_pane(i))).collect();
    let grid = tiles.insert_grid_tile(panes.clone());
    let side = tiles.insert_tab_tile(vec![]);
    let root = tiles.insert_horizontal_tile(vec![grid, side]);
    let mut tree = tree_of("grid", Some(root), tiles);

    // Moving a child away is what punches the hole; building one by hand would be building a
    // shape the crate does not actually produce.
    tree.move_tile_to_container(panes[1], side, 0, false);
    tree
}

/// Tiles detached into their own OS windows — an independent root each, plus the window's
/// monitor-space position.
fn generated_viewports() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let panes: Vec<TileId> = (0..3).map(|i| tiles.insert_pane(opaque_pane(i))).collect();
    let root = tiles.insert_tab_tile(panes.clone());
    let mut tree = tree_of("viewports", Some(root), tiles);

    tree.move_tile_to_new_viewport(panes[0], egui::pos2(100.0, 200.0));
    tree.move_tile_to_new_viewport(panes[1], egui::pos2(-50.0, 640.5));
    tree
}

/// Invisible tiles and a tree that knows its own size — two small pieces of state that live in
/// the file and nowhere in the harvested layouts.
fn generated_hidden_and_sized() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let panes: Vec<TileId> = (0..3).map(|i| tiles.insert_pane(opaque_pane(i))).collect();
    let root = tiles.insert_tab_tile(panes.clone());
    let mut tree = tree_of("hidden", Some(root), tiles);

    tree.set_visible(panes[2], false);
    tree.set_width(1280.0);
    tree.set_height(720.5);
    tree
}

/// The degenerate ends of the format: no root at all, and a single pane as the root.
fn generated_degenerate() -> Vec<(&'static str, Tree<Pane>)> {
    let empty = Tree::<Pane>::empty("empty");

    let mut tiles = Tiles::default();
    let only = tiles.insert_pane(opaque_pane(0));
    let single = tree_of("single", Some(only), tiles);

    vec![("gen_empty.ron", empty), ("gen_single_pane.ron", single)]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(out_dir) = args.next().map(PathBuf::from) else {
        eprintln!("usage: corpus_tool <out-dir> [layout files or directories…]");
        std::process::exit(2);
    };
    std::fs::create_dir_all(&out_dir).expect("the seed directory must be creatable");

    // Files given on the command line; directories are walked one level deep (`*.ron`).
    let mut sources: Vec<PathBuf> = Vec::new();
    for arg in args {
        let path = PathBuf::from(arg);
        if path.is_dir() {
            for entry in std::fs::read_dir(&path).expect("listing the source directory") {
                let entry = entry.expect("reading a directory entry").path();
                if entry.extension().is_some_and(|e| e == "ron") {
                    sources.push(entry);
                }
            }
        } else {
            sources.push(path);
        }
    }

    println!("generated seeds:");
    let mut failures: Vec<String> = Vec::new();
    let mut written = 0usize;
    let generated: Vec<(&str, Tree<Pane>)> = [
        ("gen_tabs.ron", generated_tabs()),
        ("gen_linear_nested.ron", generated_linear_nested()),
        ("gen_grid_hole.ron", generated_grid_with_hole()),
        ("gen_viewports.ron", generated_viewports()),
        ("gen_hidden_sized.ron", generated_hidden_and_sized()),
    ]
    .into_iter()
    .chain(generated_degenerate())
    .collect();

    for (name, tree) in &generated {
        match write_seed(&out_dir, name, tree) {
            Ok(()) => written += 1,
            Err(reason) => failures.push(reason),
        }
    }

    if !sources.is_empty() {
        println!("harvested seeds:");
        for source in &sources {
            match harvest(source, &out_dir) {
                Ok(()) => written += 1,
                Err(reason) => failures.push(reason),
            }
        }
    }

    for failure in &failures {
        eprintln!("FAILED: {failure}");
    }
    // A tool that quietly writes nothing is indistinguishable from one that wrote everything,
    // and the corpus is the input to every claim the fuzzer makes afterwards.
    assert!(
        failures.is_empty(),
        "{} of {} seeds could not be written",
        failures.len(),
        written + failures.len()
    );
    println!("{written} seeds in {}", out_dir.display());
}
