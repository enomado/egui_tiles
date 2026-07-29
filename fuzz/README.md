# Fuzzing

| target | input | asks |
|---|---|---|
| `tree_persist` | a saved layout (RON) | can a file the reader *accepts* leave the crate working on something that is not a tree? |

The oracle is [`Tree::validate`] — the structural check in `src/tree.rs`: children exist, no tile
has two parents, the forest is acyclic and reachable from its roots, no root is also a child, and
a tab container's open tab is one of its own tabs.

This is not a replacement for `src/proptests.rs`. Those drive the *public API*, which cannot
build a broken tree in the first place; `serde` can. A file is the one input nobody wrote on
purpose — an older version, a half-written save, a hand edit — and the deserializer is
`#[derive(Deserialize)]`, which repairs nothing.

## What the target asserts

1. **Nothing may panic.** A layout is loaded at startup; a panic there is a crash the user cannot
   escape without deleting the file by hand.
2. **A loaded tree must be well-formed after the normalization every frame begins with.**
   `Tree::ui` starts with `gc` and `simplify`, so that is what any loaded tree meets before it is
   drawn. Damage still present afterwards means the crate is drawing — and mutating — something
   that is not a tree.
3. **A save must survive our own reader unchanged**: same tiles, ids, shares, visibility, windows,
   plus the tile-id counter (`PartialEq for Tiles` ignores it; a restart does not).

Deliberately **not** asserted: that a freshly parsed tree passes `validate`. The reader repairs
nothing by design, so that would fire on the first mutated byte and hide everything behind it.
The question worth fuzzing is the one in (2): how much of that damage does the crate absorb
before it matters?

Pane payloads are read as `ron::Value`, so any payload parses. The round-trip in (3) compares
trees with the payloads blanked out: whether a payload survives serde is a property of the
application's pane type, and `ron::Value` has quirks of its own (it drops a float's width suffix,
so `inff64` returns as an f32 infinity and compares unequal) which would mask every layout finding
behind them.

## Running

```sh
cargo +nightly fuzz run tree_persist fuzz/corpus/tree_persist fuzz/seeds/tree_persist -s none
```

The first corpus directory is the writable one (libFuzzer stores what it finds interesting there,
and it is git-ignored); `seeds/` follows as read-only input. `cargo fuzz` needs nightly, and the
crate pins a stable toolchain, hence the explicit `+nightly`.

Useful flags:

* `-s none` — build without AddressSanitizer. This is safe Rust and the oracles are logical
  rather than memory-safety ones, so the sanitizer has little to catch; ~34k exec/s without it.
* `-- -max_total_time=300` — bound a run.
* `-- -runs=0` — just load the corpus and exit, enough to notice a seed that no longer passes.

A crash lands in `fuzz/artifacts/tree_persist/`. Shrink it with `cargo +nightly fuzz tmin
--sanitizer=none tree_persist <artifact>`, then re-run that file to read the panic.

## The seed corpus

`corpus_tool` builds `seeds/tree_persist` from two sources, and both are needed:

* **real saved layouts** from an application that vendors this fork — the shapes that occur in
  practice, with the float formatting a real resize produces;
* **synthetic trees built through the crate's API** — because the real corpus is thin, and whole
  regions of the format never appear in it: grids with holes, detached viewport windows,
  invisible tiles, a finite tree width/height, an empty tree. Without them the fuzzer would have
  to invent those field names byte by byte, which it will not do.

```sh
cargo run --manifest-path fuzz/corpus_tool/Cargo.toml -- fuzz/seeds/tree_persist [files or dirs…]
```

Every seed is parsed back before it is written, and a source that cannot be turned into a seed
fails the run: a silently empty corpus looks exactly like a full one from the outside.

## What has been found

Each is fixed, with a regression test next to the code it broke and a mutation that puts the test
back in the red.

* `Tree::viewport_tiles` had no serde default, so every layout written before that field existed
  failed to parse **whole** — and an application that falls back to a built-in layout when a save
  cannot be read then silently discards the user's arrangement. Found by the corpus tool, on a
  real file, before a single fuzz iteration had run.
* `ViewportTile::dragged` was persisted, though it means "the OS is dragging this window right
  now" — a gesture that ends on the next mouse-release, which after a restart has not happened.
* `gc` deleted a **shared tile** instead of the duplicate *reference* to it: the tile was taken
  out of the arena on the duplicate visit and never put back, so the container that legitimately
  owned it named a child that no longer existed. `simplify` then pruned that container as empty,
  and its parent, until the whole tree was gone and its panes were orphans.
* `gc` never questioned a **root**, so a file could name one tile as the root of two detached
  windows (both then asking egui for the same `ViewportId`), or as a window root *and* a tile of
  the main tree, and no pass ever repaired it.
* `Tabs::simplify_children` carried `active` across a `Replace` but forgot it on a `Remove`, so
  pruning an empty container that happened to be the open tab left the container pointing at a
  tile no longer in the tree.

A theme runs through the last three: the invariant was held by the **render pass**
(`Tabs::layout` calls `ensure_active` every frame), so everything looking at the tree between
loading it and drawing it — deciding which pane to reveal, taking an undo snapshot, writing a save
on startup — saw the broken value, and a save put it straight back on disk.
