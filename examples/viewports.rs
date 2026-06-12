#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

//! Demonstrates detaching tiles into their own native windows (viewports).
//!
//! Two ways to detach:
//!  * **Drag** a tab out of the main window — it spawns a new OS window following the cursor.
//!  * Click **"Pop out ⧉"** inside a pane — programmatic detach via
//!    [`egui_tiles::Tree::move_tile_to_new_viewport`].
//!
//! Closing a detached window drops its subtree (collected by `gc` on the next frame).

use egui_tiles::{TileId, Tree};

struct Pane {
    nr: usize,
}

#[derive(Default)]
struct TreeBehavior {
    /// Tiles the user asked to pop out this frame; applied after `tree.ui`.
    pop_out: Vec<TileId>,
}

impl egui_tiles::Behavior<Pane> for TreeBehavior {
    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        format!("Pane {}", pane.nr).into()
    }

    fn pane_ui(
        &mut self,
        ui: &mut egui::Ui,
        tile_id: TileId,
        pane: &mut Pane,
    ) -> egui_tiles::UiResponse {
        let color = egui::epaint::Hsva::new(0.103 * pane.nr as f32, 0.5, 0.5, 1.0);
        ui.painter().rect_filled(ui.max_rect(), 0.0, color);

        ui.label(format!("The contents of pane {}.", pane.nr));

        if ui.button("Pop out ⧉").clicked() {
            self.pop_out.push(tile_id);
        }

        // Dragging a tab outside the main window detaches it automatically; this button
        // additionally lets you start a drag from the pane body.
        if ui
            .add(egui::Button::new("Drag me out!").sense(egui::Sense::drag()))
            .drag_started()
        {
            egui_tiles::UiResponse::DragStarted
        } else {
            egui_tiles::UiResponse::None
        }
    }
}

fn main() -> Result<(), eframe::Error> {
    env_logger::init(); // Log to stderr (if you run with `RUST_LOG=debug`).

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([700.0, 500.0]),
        ..Default::default()
    };

    let mut tree = create_tree();

    eframe::run_ui_native("egui_tiles viewports", options, move |ui, _frame| {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            let mut behavior = TreeBehavior::default();
            tree.ui(&mut behavior, ui);

            // Apply programmatic pop-out requests, fanning the new windows out diagonally.
            for (i, tile_id) in behavior.pop_out.drain(..).enumerate() {
                let pos = egui::pos2(120.0 + 40.0 * i as f32, 120.0 + 40.0 * i as f32);
                tree.move_tile_to_new_viewport(tile_id, pos);
            }
        });
    })
}

fn create_tree() -> Tree<Pane> {
    let mut next_view_nr = 0;
    let mut gen_pane = || {
        let pane = Pane { nr: next_view_nr };
        next_view_nr += 1;
        pane
    };

    let mut tiles = egui_tiles::Tiles::default();

    let mut tabs = vec![];
    tabs.push({
        let children = (0..3).map(|_| tiles.insert_pane(gen_pane())).collect();
        tiles.insert_horizontal_tile(children)
    });
    tabs.push(tiles.insert_pane(gen_pane()));

    let root = tiles.insert_tab_tile(tabs);

    Tree::new("viewports_tree", root, tiles)
}
