use egui::{
    Color32, Id, Rect, Response, Rgba, Sense, Stroke, TextStyle, Ui, Vec2, Visuals, WidgetInfo,
    WidgetText, WidgetType, vec2,
};

use super::{
    Container, InsertionPoint, ResizeState, SimplificationOptions, Tile, TileId, Tiles, UiResponse,
};

/// The kind of edit that triggered the call to [`Behavior::on_edit`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditAction {
    /// A tile was resized by dragging or double-clicking a boundary.
    TileResized,

    /// A drag with a tile started.
    TileDragged,

    /// A tile was dropped and its position changed accordingly.
    TileDropped,

    /// A tab was selected by a click, or by hovering a dragged tile over it,
    /// or there was no active tab and egui picked an arbitrary one.
    TabSelected,
}

/// Determines what happens to a tab when a user attempts to close it.
///
/// Returned by [`Behavior::on_tab_close_response`]. This is a richer alternative
/// to the legacy boolean [`Behavior::on_tab_close`] hook.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OnCloseResponse {
    /// Closes the tab.
    Close,

    /// Focuses on the tab (does not close).
    ///
    /// The tab becomes the active tab in its parent [`crate::Tabs`] container.
    Focus,

    /// Ignores the close request (does not close).
    Ignore,
}

/// The state of a tab, used to inform the rendering of the tab.
#[derive(Clone, Debug, Default)]
pub struct TabState {
    /// Is the tab currently selected?
    pub active: bool,

    /// Is the tab currently being dragged?
    pub is_being_dragged: bool,

    /// Should the tab have a close button?
    pub closable: bool,
}

/// Everything the layout pass needs from a [`Behavior`], with the pane type erased.
///
/// The layout pass never looks at a pane's contents — it only needs a handful of numbers.
/// Gathering them up front keeps `Pane` out of the layout signatures entirely, which lets the
/// same code lay out any [`Tiles`], whatever it happens to store in its panes.
pub(crate) struct LayoutContext<'a> {
    pub gap_width: f32,

    /// Gutter between rows/columns of a [`crate::Grid`]; defaults to `gap_width`.
    pub grid_gap_width: f32,

    pub tab_bar_height: f32,
    pub grid_auto_column_count: &'a dyn Fn(usize, Rect, f32) -> usize,

    /// Set by the layout pass if it had to pick an active tab for a [`crate::Tabs`] container.
    ///
    /// Reported back to the caller rather than straight to [`Behavior::on_edit`]: laying out
    /// the tree is not the place to be emitting user-visible edit events from.
    pub tab_auto_selected: &'a std::cell::Cell<bool>,
}

/// Lay out `tiles` starting at `root`, using only the pane-agnostic parts of `behavior`.
///
/// Generic over the pane type of `tiles`, which need not be the pane type `behavior` is for.
///
/// Returns `true` if the pass had to auto-select an active tab, in which case the caller
/// should report [`EditAction::TabSelected`].
pub(crate) fn layout_tiles<Pane, TilesPane>(
    tiles: &mut Tiles<TilesPane>,
    root: Option<TileId>,
    behavior: &dyn Behavior<Pane>,
    style: &egui::Style,
    rect: Rect,
) -> bool {
    let Some(root) = root else {
        return false;
    };

    let grid_auto_column_count = |num_visible_children, rect, gap| {
        behavior.grid_auto_column_count(num_visible_children, rect, gap)
    };
    let tab_auto_selected = std::cell::Cell::new(false);

    let layout = LayoutContext {
        gap_width: behavior.gap_width(style),
        grid_gap_width: behavior.grid_gap_width(style),
        tab_bar_height: behavior.tab_bar_height(style),
        grid_auto_column_count: &grid_auto_column_count,
        tab_auto_selected: &tab_auto_selected,
    };

    tiles.layout_tile(&layout, rect, root);

    tab_auto_selected.get()
}

/// Trait defining how the [`super::Tree`] and its panes should be shown.
pub trait Behavior<Pane> {
    /// Show a pane tile in the given [`egui::Ui`].
    ///
    /// You can make the pane draggable by returning [`UiResponse::DragStarted`]
    /// when the user drags some handle.
    fn pane_ui(&mut self, ui: &mut Ui, tile_id: TileId, pane: &mut Pane) -> UiResponse;

    /// The title of a pane tab.
    fn tab_title_for_pane(&mut self, pane: &Pane) -> WidgetText;

    /// The cursor icon when hovering over a tab.
    fn tab_hover_cursor_icon(&self) -> egui::CursorIcon {
        egui::CursorIcon::Grab
    }

    /// Should the tab have a close-button?
    fn is_tab_closable(&self, _tiles: &Tiles<Pane>, _tile_id: TileId) -> bool {
        false
    }

    /// Called when the close-button on a tab is pressed.
    ///
    /// Return `false` to abort the closing of a tab (e.g. after showing a message box).
    ///
    /// This is the legacy, binary close hook. For richer control (close / focus / ignore),
    /// override [`Self::on_tab_close_response`] instead — when overridden it *supersedes*
    /// this method (the default [`Self::on_tab_close_response`] bridges to this one, but a
    /// custom [`Self::on_tab_close_response`] will not call `on_tab_close` at all).
    fn on_tab_close(&mut self, _tiles: &mut Tiles<Pane>, _tile_id: TileId) -> bool {
        true
    }

    /// Called when the close-button on a tab is pressed, returning richer close semantics.
    ///
    /// This supersedes [`Self::on_tab_close`]: if you override this method, [`Self::on_tab_close`]
    /// is no longer consulted for the close decision.
    ///
    /// The default implementation bridges to the legacy [`Self::on_tab_close`] hook for
    /// backward compatibility: `true` maps to [`OnCloseResponse::Close`] and `false` maps to
    /// [`OnCloseResponse::Ignore`].
    ///
    /// - [`OnCloseResponse::Close`]: the tab is removed from the tree.
    /// - [`OnCloseResponse::Focus`]: the tab is *not* removed; instead it becomes the active
    ///   tab of its parent [`crate::Tabs`] container (no-op if the parent is not a `Tabs`).
    /// - [`OnCloseResponse::Ignore`]: nothing happens.
    fn on_tab_close_response(
        &mut self,
        tiles: &mut Tiles<Pane>,
        tile_id: TileId,
    ) -> OnCloseResponse {
        if self.on_tab_close(tiles, tile_id) {
            OnCloseResponse::Close
        } else {
            OnCloseResponse::Ignore
        }
    }

    /// The size of the close button in the tab.
    fn close_button_outer_size(&self) -> f32 {
        12.0
    }

    /// How much smaller the visual part of the close-button will be
    /// compared to [`Self::close_button_outer_size`].
    fn close_button_inner_margin(&self) -> f32 {
        2.0
    }

    /// The title of a general tab.
    ///
    /// The default implementation calls [`Self::tab_title_for_pane`] for panes and
    /// uses the name of the [`crate::ContainerKind`] for [`crate::Container`]s.
    fn tab_title_for_tile(&mut self, tiles: &Tiles<Pane>, tile_id: TileId) -> WidgetText {
        if let Some(tile) = tiles.get(tile_id) {
            match tile {
                Tile::Pane(pane) => self.tab_title_for_pane(pane),
                Tile::Container(container) => format!("{:?}", container.kind()).into(),
            }
        } else {
            "MISSING TILE".into()
        }
    }

    /// Show the ui for the a tab of some tile.
    ///
    /// The default implementation shows a clickable button with the title for that tile,
    /// gotten with [`Self::tab_title_for_tile`].
    /// The default implementation also calls [`Self::on_tab_button`].
    ///
    /// You can override the default implementation to add e.g. a close button.
    /// Make sure it is sensitive to clicks and drags (if you want to enable drag-and-drop of tabs).
    fn tab_ui(
        &mut self,
        tiles: &mut Tiles<Pane>,
        ui: &mut Ui,
        id: Id,
        tile_id: TileId,
        state: &TabState,
    ) -> Response {
        let text = self.tab_title_for_tile(tiles, tile_id);
        let close_btn_size = Vec2::splat(self.close_button_outer_size());
        let close_btn_left_padding = 4.0;
        let font_id = TextStyle::Button.resolve(ui.style());
        let galley = text.into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, font_id);

        let x_margin = self.tab_title_spacing(ui.visuals());

        let button_width = galley.size().x
            + 2.0 * x_margin
            + f32::from(state.closable) * (close_btn_left_padding + close_btn_size.x);
        let (_, tab_rect) = ui.allocate_space(vec2(button_width, ui.available_height()));

        let draggable = self.is_tile_draggable(tiles, tile_id);
        let sense = if draggable {
            Sense::click_and_drag()
        } else {
            Sense::click()
        };
        let tab_response = ui.interact(tab_rect, id, sense);
        let tab_response = if draggable {
            tab_response.on_hover_cursor(self.tab_hover_cursor_icon())
        } else {
            tab_response
        };

        // A bare `Ui::interact` reports nothing about itself, so without this a tab is an unnamed
        // blob to screen readers, and cannot be found by name from `egui_kittest`.
        //
        // Deliberately outside the `is_rect_visible` check below: a tab scrolled out of the tab
        // bar is still a tab.
        tab_response.widget_info(|| {
            WidgetInfo::selected(
                WidgetType::Button,
                ui.is_enabled(),
                state.active,
                galley.text(),
            )
        });

        // Show a gap when dragged
        if ui.is_rect_visible(tab_rect) && !state.is_being_dragged {
            let bg_color = self.tab_bg_color(ui.visuals(), tiles, tile_id, state);
            let stroke = self.tab_outline_stroke(ui.visuals(), tiles, tile_id, state);
            ui.painter().rect(
                tab_rect.shrink(0.5),
                0.0,
                bg_color,
                stroke,
                egui::StrokeKind::Inside,
            );

            if state.active {
                // Make the tab name area connect with the tab ui area:
                ui.painter().hline(
                    tab_rect.x_range(),
                    tab_rect.bottom(),
                    Stroke::new(stroke.width + 1.0, bg_color),
                );
            }

            // Prepare title's text for rendering
            let text_color = self.tab_text_color(ui.visuals(), tiles, tile_id, state);
            let text_position = egui::Align2::LEFT_CENTER
                .align_size_within_rect(galley.size(), tab_rect.shrink(x_margin))
                .min;

            // Render the title
            ui.painter().galley(text_position, galley, text_color);

            // Conditionally render the close button
            if state.closable {
                let close_btn_rect = egui::Align2::RIGHT_CENTER
                    .align_size_within_rect(close_btn_size, tab_rect.shrink(x_margin));

                // Allocate
                let close_btn_id = ui.auto_id_with("tab_close_btn");
                let close_btn_response = ui
                    .interact(close_btn_rect, close_btn_id, Sense::click_and_drag())
                    .on_hover_cursor(egui::CursorIcon::Default);

                close_btn_response.widget_info(|| {
                    WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), "Close")
                });

                let visuals = ui.style().interact(&close_btn_response);

                // Scale based on the interaction visuals
                let rect = close_btn_rect
                    .shrink(self.close_button_inner_margin())
                    .expand(visuals.expansion);
                let stroke = visuals.fg_stroke;

                // paint the crossed lines
                ui.painter() // paints \
                    .line_segment([rect.left_top(), rect.right_bottom()], stroke);
                ui.painter() // paints /
                    .line_segment([rect.right_top(), rect.left_bottom()], stroke);

                // Give the user a chance to react to the close button being clicked
                // Only close if the user returns true (handled)
                if close_btn_response.clicked()
                    || tab_response.clicked_by(egui::PointerButton::Middle)
                {
                    log::debug!("Tab close requested for tile: {tile_id:?}");

                    // Ask the implementation what to do, then apply it.
                    let response = self.on_tab_close_response(tiles, tile_id);
                    apply_close_response(tiles, tile_id, response);
                }
            }
        }

        self.on_tab_button(tiles, tile_id, tab_response)
    }

    /// Show the ui for the tab being dragged.
    fn drag_ui(&mut self, tiles: &Tiles<Pane>, ui: &mut Ui, tile_id: TileId) {
        let mut frame = egui::Frame::popup(ui.style());
        frame.fill = frame.fill.gamma_multiply(0.5); // Make see-through
        frame.show(ui, |ui| {
            // TODO(emilk): preview contents?
            let text = self.tab_title_for_tile(tiles, tile_id);
            ui.label(text);
        });
    }

    /// Called by the default implementation of [`Self::tab_ui`] for each added button
    fn on_tab_button(
        &mut self,
        _tiles: &mut Tiles<Pane>,
        _tile_id: TileId,
        button_response: Response,
    ) -> Response {
        button_response
    }

    /// Return `false` if a given pane should be removed from its parent.
    fn retain_pane(&mut self, _pane: &Pane) -> bool {
        true
    }

    /// Adds some UI to the top right of each tab bar.
    ///
    /// You can use this to, for instance, add a button for adding new tabs.
    ///
    /// The widgets will be added right-to-left.
    ///
    /// `_scroll_offset` is a mutable reference to the tab scroll value.
    /// Adding to this value will scroll the tabs to the right, subtracting to the left.
    fn top_bar_right_ui(
        &mut self,
        _tiles: &Tiles<Pane>,
        _ui: &mut Ui,
        _tile_id: TileId,
        _tabs: &crate::Tabs,
        _scroll_offset: &mut f32,
    ) {
        // if ui.button("➕").clicked() {
        // }
    }

    /// Adds some UI to the tab bar immediately after the last tab.
    ///
    /// This is rendered inside the tab scroll area's left-to-right flow, so it
    /// scrolls together with the tabs and sits right after the last visible tab
    /// (e.g. a browser-style "➕" button for adding a new tab).
    ///
    /// Note: item spacing inside the tab flow is zero, so add your own spacing
    /// (e.g. `ui.add_space(..)`) if you want a gap before your widget.
    ///
    /// Compare with [`Self::top_bar_right_ui`], which pins widgets to the far
    /// right of the tab bar.
    fn tab_bar_trailing_ui(
        &mut self,
        _tiles: &Tiles<Pane>,
        _ui: &mut Ui,
        _tile_id: TileId,
        _tabs: &crate::Tabs,
    ) {
    }

    /// The height of the bar holding tab titles.
    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        24.0
    }

    /// Width of the gap between tiles in a horizontal or vertical layout,
    /// and between rows/columns in a grid layout.
    fn gap_width(&self, _style: &egui::Style) -> f32 {
        1.0
    }

    /// Width of the gap between rows and columns of a [`crate::Grid`] container.
    ///
    /// Defaults to [`Self::gap_width`], so grids share the global gap unless overridden.
    /// Override to give grids their own gutter, independent of linear/tab spacing.
    fn grid_gap_width(&self, style: &egui::Style) -> f32 {
        self.gap_width(style)
    }

    /// No child should shrink below this width nor height.
    fn min_size(&self) -> f32 {
        32.0
    }

    /// Show we preview panes that are being dragged,
    /// i.e. show their ui in the region where they will end up?
    fn preview_dragged_panes(&self) -> bool {
        false
    }

    /// Cover the tile that is being dragged with this color.
    fn dragged_overlay_color(&self, visuals: &Visuals) -> Color32 {
        visuals.panel_fill.gamma_multiply(0.5)
    }

    /// What are the rules for simplifying the tree?
    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions::default()
    }

    /// Add some custom painting on top of a tile (container or pane), e.g. draw an outline on top of it.
    fn paint_on_top_of_tile(
        &self,
        _painter: &egui::Painter,
        _style: &egui::Style,
        _tile_id: TileId,
        _rect: Rect,
    ) {
    }

    /// The stroke used for the lines in horizontal, vertical, and grid layouts.
    fn resize_stroke(&self, style: &egui::Style, resize_state: ResizeState) -> Stroke {
        match resize_state {
            ResizeState::Idle => {
                Stroke::new(self.gap_width(style), self.tab_bar_color(&style.visuals))
            }
            ResizeState::Hovering => style.visuals.widgets.hovered.fg_stroke,
            ResizeState::Dragging => style.visuals.widgets.active.fg_stroke,
        }
    }

    /// Extra spacing to left and right of tab titles.
    fn tab_title_spacing(&self, _visuals: &Visuals) -> f32 {
        8.0
    }

    /// The background color of the tab bar.
    fn tab_bar_color(&self, visuals: &Visuals) -> Color32 {
        if visuals.dark_mode {
            visuals.extreme_bg_color
        } else {
            (Rgba::from(visuals.panel_fill) * Rgba::from_gray(0.8)).into()
        }
    }

    /// The background color of a tab.
    fn tab_bg_color(
        &self,
        visuals: &Visuals,
        _tiles: &Tiles<Pane>,
        _tile_id: TileId,
        state: &TabState,
    ) -> Color32 {
        if state.active {
            visuals.panel_fill // same as the tab contents
        } else {
            Color32::TRANSPARENT // fade into background
        }
    }

    /// Stroke of the outline around a tab title.
    fn tab_outline_stroke(
        &self,
        visuals: &Visuals,
        _tiles: &Tiles<Pane>,
        _tile_id: TileId,
        state: &TabState,
    ) -> Stroke {
        if state.active {
            Stroke::new(1.0_f32, visuals.widgets.active.bg_fill)
        } else {
            Stroke::NONE
        }
    }

    /// Stroke of the line separating the tab title bar and the content of the active tab.
    fn tab_bar_hline_stroke(&self, visuals: &Visuals) -> Stroke {
        Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color)
    }

    /// The color of the title text of the tab.
    ///
    /// This is the fallback color used if [`Self::tab_title_for_tile`]
    /// has no color.
    fn tab_text_color(
        &self,
        visuals: &Visuals,
        _tiles: &Tiles<Pane>,
        _tile_id: TileId,
        state: &TabState,
    ) -> Color32 {
        if state.active {
            visuals.widgets.active.text_color()
        } else {
            visuals.widgets.noninteractive.text_color()
        }
    }

    /// When drag-and-dropping a tile, the candidate area is drawn with this stroke.
    fn drag_preview_stroke(&self, visuals: &Visuals) -> Stroke {
        visuals.selection.stroke
    }

    /// When drag-and-dropping a tile, the candidate area is drawn with this background color.
    fn drag_preview_color(&self, visuals: &Visuals) -> Color32 {
        visuals.selection.stroke.color.gamma_multiply(0.5)
    }

    /// Color for the drop preview when [`Self::is_drop_allowed`] returns `false`.
    fn drag_preview_color_rejected(&self, _visuals: &Visuals) -> Color32 {
        Color32::from_rgba_premultiplied(180, 40, 40, 100)
    }

    /// Stroke for the drop preview when [`Self::is_drop_allowed`] returns `false`.
    fn drag_preview_stroke_rejected(&self, _visuals: &Visuals) -> Stroke {
        Stroke::new(1.0_f32, Color32::from_rgb(200, 60, 60))
    }

    /// When drag-and-dropping a tile, how do we preview what is about to happen?
    fn paint_drag_preview(
        &self,
        visuals: &Visuals,
        painter: &egui::Painter,
        parent_rect: Option<Rect>,
        preview_rect: Rect,
    ) {
        let preview_stroke = self.drag_preview_stroke(visuals);
        let preview_color = self.drag_preview_color(visuals);

        if let Some(parent_rect) = parent_rect {
            // Show which parent we will be dropped into
            painter.rect_stroke(parent_rect, 1.0, preview_stroke, egui::StrokeKind::Inside);
        }

        painter.rect(
            preview_rect,
            1.0,
            preview_color,
            preview_stroke,
            egui::StrokeKind::Inside,
        );
    }

    /// How many columns should we use for a [`crate::Grid`] put into [`crate::GridLayout::Auto`]?
    ///
    /// The default heuristic tried to find a good column count that results in a per-tile aspect-ratio
    /// of [`Self::ideal_tile_aspect_ratio`].
    ///
    /// The `rect` is the available space for the grid,
    /// and `gap` is the distance between each column and row.
    fn grid_auto_column_count(&self, num_visible_children: usize, rect: Rect, gap: f32) -> usize {
        num_columns_heuristic(
            num_visible_children,
            rect.size(),
            gap,
            self.ideal_tile_aspect_ratio(),
        )
    }

    /// When using [`crate::GridLayout::Auto`], what is the ideal aspect ratio of a tile?
    fn ideal_tile_aspect_ratio(&self) -> f32 {
        4.0 / 3.0
    }

    /// Can this tile be dragged?
    ///
    /// If `false`, the tile cannot be dragged by the user.
    /// This affects both tab dragging and pane dragging.
    ///
    /// Default: `true` (all tiles are draggable).
    fn is_tile_draggable(&self, _tiles: &Tiles<Pane>, _tile_id: TileId) -> bool {
        true
    }

    /// Can the dragged tile be dropped at the given insertion point?
    ///
    /// Called once per frame with the best (closest to cursor) candidate insertion point.
    /// If `false`, the drop preview is drawn in the rejected color
    /// (see [`Self::drag_preview_color_rejected`]) and the drop is not performed.
    ///
    /// Default: `true` (all drops are allowed).
    fn is_drop_allowed(
        &self,
        _tiles: &Tiles<Pane>,
        _dragged_tile_id: TileId,
        _insertion: &InsertionPoint,
    ) -> bool {
        true
    }

    /// Can the children of this container be resized by dragging the separator?
    ///
    /// Only applies to [`crate::Linear`] and [`crate::Grid`] containers.
    ///
    /// Default: `true` (all containers are resizable).
    fn is_container_resizable(&self, _tiles: &Tiles<Pane>, _tile_id: TileId) -> bool {
        true
    }

    // Callbacks:

    /// Called if the user edits the tree somehow, e.g. changes the size of some container,
    /// clicks a tab, or drags a tile.
    fn on_edit(&mut self, _edit_action: EditAction) {}
}

/// Apply the result of a tab close-request to the tiles.
///
/// Factored out of [`Behavior::tab_ui`] so the close semantics are unit-testable
/// without a live [`egui::Ui`]:
/// - [`OnCloseResponse::Close`]: remove the tile from the tiles.
/// - [`OnCloseResponse::Focus`]: leave the tile in place and make it the active tab of its
///   parent [`Tabs`] container. If the tile has no parent, or the parent is not a `Tabs`
///   container, this is a no-op (we never remove on `Focus`).
/// - [`OnCloseResponse::Ignore`]: do nothing.
pub(crate) fn apply_close_response<Pane>(
    tiles: &mut Tiles<Pane>,
    tile_id: TileId,
    response: OnCloseResponse,
) {
    match response {
        OnCloseResponse::Close => {
            log::debug!("Implementation confirmed close request for tile: {tile_id:?}");
            tiles.remove(tile_id);
        }
        OnCloseResponse::Focus => {
            log::debug!("Implementation requested focus instead of close for tile: {tile_id:?}");
            // Make this tab the active tab of its parent `Tabs` container.
            // Reuse `Tabs::set_active` rather than duplicating activation logic.
            if let Some(parent_id) = tiles.parent_of(tile_id)
                && let Some(Tile::Container(Container::Tabs(tabs))) = tiles.get_mut(parent_id)
            {
                tabs.set_active(tile_id);
            }
        }
        OnCloseResponse::Ignore => {
            log::debug!("Implementation denied close request for tile: {tile_id:?}");
        }
    }
}

/// How many columns should we use to fit `n` children in a grid?
fn num_columns_heuristic(n: usize, size: Vec2, gap: f32, desired_aspect: f32) -> usize {
    let mut best_loss = f32::INFINITY;
    let mut best_num_columns = 1;

    for ncols in 1..=n {
        if 4 <= n && ncols == n - 1 {
            // Don't suggest 7 columns when n=8 - that produces an ugly orphan on a single row.
            continue;
        }

        let nrows = n.div_ceil(ncols);

        let cell_width = (size.x - gap * (ncols as f32 - 1.0)) / (ncols as f32);
        let cell_height = (size.y - gap * (nrows as f32 - 1.0)) / (nrows as f32);

        let cell_aspect = cell_width / cell_height;
        let aspect_diff = (desired_aspect - cell_aspect).abs();
        let num_empty_cells = ncols * nrows - n;

        let loss = aspect_diff * n as f32 + 2.0 * num_empty_cells as f32;

        if loss < best_loss {
            best_loss = loss;
            best_num_columns = ncols;
        }
    }

    best_num_columns
}

#[test]
fn test_num_columns_heuristic() {
    // Four tiles should always be in a 1x4, 2x2, or 4x1 grid - NEVER 2x3 or 3x2.

    let n = 4;
    let gap = 0.0;
    let ideal_tile_aspect_ratio = 4.0 / 3.0;

    for i in 0..=100 {
        let size = Vec2::new(100.0, egui::remap(i as f32, 0.0..=100.0, 1.0..=1000.0));

        let ncols = num_columns_heuristic(n, size, gap, ideal_tile_aspect_ratio);
        assert!(
            ncols == 1 || ncols == 2 || ncols == 4,
            "Size {size:?} got {ncols} columns"
        );
    }
}

#[cfg(test)]
mod close_response_tests {
    use super::{Behavior, OnCloseResponse, apply_close_response};
    use crate::{Container, SimplificationOptions, Tile, TileId, Tiles, Tree, UiResponse};

    /// Run the part of a real frame that cleans up dangling references and unreachable tiles
    /// (`simplify` then `gc`), so we can validate the tree after a `Close` removed a tile from
    /// the arena but left a stale child reference in the parent (exactly as the live UI does).
    fn run_frame_cleanup(tree: &mut Tree<Pane>, behavior: &mut dyn Behavior<Pane>) {
        tree.simplify(&SimplificationOptions::default());
        tree.gc(behavior);
    }

    #[derive(Debug, PartialEq)]
    struct Pane(u32);

    /// A behavior that returns a fixed [`OnCloseResponse`] from the new hook.
    struct FixedResponseBehavior(OnCloseResponse);

    impl Behavior<Pane> for FixedResponseBehavior {
        fn pane_ui(
            &mut self,
            _ui: &mut egui::Ui,
            _tile_id: TileId,
            _pane: &mut Pane,
        ) -> UiResponse {
            panic!("not used in these tests")
        }

        fn tab_title_for_pane(&mut self, _pane: &Pane) -> egui::WidgetText {
            panic!("not used in these tests")
        }

        fn on_tab_close_response(
            &mut self,
            _tiles: &mut Tiles<Pane>,
            _tile_id: TileId,
        ) -> OnCloseResponse {
            self.0
        }
    }

    /// A behavior that only overrides the *legacy* boolean hook, to exercise the
    /// default `on_tab_close_response` bridge.
    struct LegacyBoolBehavior(bool);

    impl Behavior<Pane> for LegacyBoolBehavior {
        fn pane_ui(
            &mut self,
            _ui: &mut egui::Ui,
            _tile_id: TileId,
            _pane: &mut Pane,
        ) -> UiResponse {
            panic!("not used in these tests")
        }

        fn tab_title_for_pane(&mut self, _pane: &Pane) -> egui::WidgetText {
            panic!("not used in these tests")
        }

        fn on_tab_close(&mut self, _tiles: &mut Tiles<Pane>, _tile_id: TileId) -> bool {
            self.0
        }
    }

    /// Build a tree: a `Tabs` root with two pane children. Returns `(tree, first_pane, second_pane)`.
    fn tabs_tree_with_two_panes() -> (Tree<Pane>, TileId, TileId) {
        let mut tiles = Tiles::default();
        let a = tiles.insert_pane(Pane(1));
        let b = tiles.insert_pane(Pane(2));
        let root = tiles.insert_tab_tile(vec![a, b]);
        let tree = Tree::new("test_tree", root, tiles);
        (tree, a, b)
    }

    /// Helper: the active child of the (single) Tabs container in the tree.
    fn active_of_tabs(tree: &Tree<Pane>) -> Option<TileId> {
        for tile in tree.tiles.tiles() {
            if let Tile::Container(Container::Tabs(tabs)) = tile {
                return tabs.active;
            }
        }
        None
    }

    #[test]
    fn close_removes_tile_and_tree_stays_valid() {
        let (mut tree, a, _b) = tabs_tree_with_two_panes();
        let mut behavior = FixedResponseBehavior(OnCloseResponse::Close);

        let resp = behavior.on_tab_close_response(&mut tree.tiles, a);
        apply_close_response(&mut tree.tiles, a, resp);

        assert!(tree.tiles.get(a).is_none(), "Close should remove the tile");

        // `Close` (like the real UI) only removes the tile from the map; the parent's stale
        // child reference is cleaned up by the per-frame simplify+gc pass. Run it, then validate.
        run_frame_cleanup(&mut tree, &mut behavior);
        tree.validate().expect("tree must stay valid after Close");
    }

    #[test]
    fn ignore_keeps_tile_and_tree_stays_valid() {
        let (mut tree, a, _b) = tabs_tree_with_two_panes();
        let mut behavior = FixedResponseBehavior(OnCloseResponse::Ignore);

        let resp = behavior.on_tab_close_response(&mut tree.tiles, a);
        apply_close_response(&mut tree.tiles, a, resp);

        assert!(
            tree.tiles.get(a).is_some(),
            "Ignore must NOT remove the tile"
        );
        tree.validate().expect("tree must stay valid after Ignore");
    }

    #[test]
    fn focus_keeps_tile_and_makes_it_active() {
        // Start with `b` active (it was inserted last; set explicitly to be sure).
        let (mut tree, a, b) = tabs_tree_with_two_panes();
        {
            // Make `b` active so that focusing `a` is an observable change.
            for tile in tree.tiles.tiles_mut() {
                if let Tile::Container(Container::Tabs(tabs)) = tile {
                    tabs.set_active(b);
                }
            }
        }
        assert_eq!(active_of_tabs(&tree), Some(b));

        let mut behavior = FixedResponseBehavior(OnCloseResponse::Focus);
        let resp = behavior.on_tab_close_response(&mut tree.tiles, a);
        apply_close_response(&mut tree.tiles, a, resp);

        assert!(
            tree.tiles.get(a).is_some(),
            "Focus must NOT remove the tile"
        );
        assert_eq!(
            active_of_tabs(&tree),
            Some(a),
            "Focus must make the tile the active tab of its parent Tabs"
        );
        tree.validate().expect("tree must stay valid after Focus");
    }

    #[test]
    fn focus_on_tile_without_tabs_parent_is_noop() {
        // A pane that is the lone root (no Tabs parent). Focus must be a no-op, never a removal.
        let mut tiles = Tiles::default();
        let a = tiles.insert_pane(Pane(1));
        let mut tree = Tree::new("solo_tree", a, tiles);

        let mut behavior = FixedResponseBehavior(OnCloseResponse::Focus);
        let resp = behavior.on_tab_close_response(&mut tree.tiles, a);
        apply_close_response(&mut tree.tiles, a, resp);

        assert!(
            tree.tiles.get(a).is_some(),
            "Focus on a parentless tile must NOT remove it"
        );
        tree.validate().expect("tree must stay valid");
    }

    #[test]
    fn legacy_bool_false_bridges_to_ignore() {
        let (mut tree, a, _b) = tabs_tree_with_two_panes();
        let mut behavior = LegacyBoolBehavior(false);

        // The default `on_tab_close_response` should bridge `false` -> Ignore.
        let resp = behavior.on_tab_close_response(&mut tree.tiles, a);
        assert_eq!(resp, OnCloseResponse::Ignore);

        apply_close_response(&mut tree.tiles, a, resp);
        assert!(
            tree.tiles.get(a).is_some(),
            "Legacy `false` must keep the tile (Ignore)"
        );
        tree.validate().expect("tree must stay valid");
    }

    #[test]
    fn legacy_bool_true_bridges_to_close() {
        let (mut tree, a, _b) = tabs_tree_with_two_panes();
        let mut behavior = LegacyBoolBehavior(true);

        let resp = behavior.on_tab_close_response(&mut tree.tiles, a);
        assert_eq!(resp, OnCloseResponse::Close);

        apply_close_response(&mut tree.tiles, a, resp);
        assert!(
            tree.tiles.get(a).is_none(),
            "Legacy `true` must remove the tile (Close)"
        );
        run_frame_cleanup(&mut tree, &mut behavior);
        tree.validate().expect("tree must stay valid");
    }
}
