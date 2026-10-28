//! Floating split layouts.
//!
//! Each pane of a tab can have a float: a split layout of its own that is
//! displayed over the pane, inset by `floating_pane_padding`, with an index
//! bar along its bottom row. The float of the active pane has keyboard
//! focus while it is visible. Leader key assignments keep acting on the
//! tab's own layout underneath, except after `FloatingPaneLeader`, which
//! makes the next one act on the float instead.

use crate::quad::TripleLayerQuadAllocator;
use crate::termwindow::render::RenderScreenLineParams;
use ::window::{RectF, WindowOps};
use config::{DimensionContext, TabBarColors};
use mux::renderable::RenderableDimensions;
use mux::tab::{Float, PositionedPane, PositionedSplit, Tab};
use mux::Mux;
use std::sync::Arc;
use termwiz::cell::CellAttributes;
use termwiz::color::ColorSpec;
use termwiz::surface::SEQ_ZERO;
use wezterm_term::color::ColorAttribute;
use wezterm_term::{Line, TerminalSize};
use window::color::LinearRgba;

/// The smallest float, in cells, including its index bar
const MIN_FLOAT_WIDTH: usize = 10;
const MIN_FLOAT_HEIGHT: usize = 3;

/// A visible float laid out over its owner pane, in window cell coordinates
pub struct PositionedFloat {
    pub float: Float,
    /// True if the float has keyboard focus
    pub is_focused: bool,
    /// The owner pane as it is currently laid out
    pub owner: PositionedPane,
    /// The area covered by the float, including its index bar
    pub left: usize,
    pub top: usize,
    pub width: usize,
    pub height: usize,
    /// The panes and splits of the float, offset into window coordinates
    pub panes: Vec<PositionedPane>,
    pub splits: Vec<PositionedSplit>,
}

impl PositionedFloat {
    fn contains(left: usize, top: usize, width: usize, height: usize, column: usize, row: i64) -> bool {
        row >= top as i64
            && row < (top + height) as i64
            && column >= left
            && column < left + width
    }

    /// Returns true if the cell is within the area covered by the float
    pub fn area_contains(&self, column: usize, row: i64) -> bool {
        Self::contains(self.left, self.top, self.width, self.height, column, row)
    }

    /// Returns true if the cell is within the owner pane, which the float
    /// captures mouse input for
    pub fn owner_contains(&self, column: usize, row: i64) -> bool {
        let o = &self.owner;
        Self::contains(o.left, o.top, o.width, o.height, column, row)
    }
}

impl crate::TermWindow {
    /// Returns the float that has keyboard focus in the active tab
    pub fn get_active_float(&self) -> Option<Float> {
        let tab = Mux::get().get_active_tab_for_window(self.mux_window_id)?;
        if self.tab_state(tab.tab_id()).overlay.is_some() {
            return None;
        }
        tab.get_active_float()
    }

    /// Returns the tab whose split layout pane key assignments act on:
    /// the focused float, unless the assignment was made via the leader
    pub fn get_target_tab(&self) -> Option<Arc<Tab>> {
        let tab = Mux::get().get_active_tab_for_window(self.mux_window_id)?;
        if !self.action_targets_tab {
            if let Some(float) = self.get_active_float() {
                return Some(float.tab);
            }
        }
        Some(tab)
    }

    /// Computes the area, in cells, that a float covers over `owner`
    fn float_area(&self, owner: &PositionedPane) -> (usize, usize, usize, usize) {
        let cell_width = self.render_metrics.cell_size.width as f32;
        let cell_height = self.render_metrics.cell_size.height as f32;
        let h_context = DimensionContext {
            dpi: self.dimensions.dpi as f32,
            pixel_max: owner.width as f32 * cell_width,
            pixel_cell: cell_width,
        };
        let v_context = DimensionContext {
            dpi: self.dimensions.dpi as f32,
            pixel_max: owner.height as f32 * cell_height,
            pixel_cell: cell_height,
        };
        let padding = &self.config.floating_pane_padding;
        let cells = |pixels: f32, cell: f32| (pixels / cell).round() as usize;
        let pad_left = cells(padding.left.evaluate_as_pixels(h_context), cell_width);
        let pad_right = cells(padding.right.evaluate_as_pixels(h_context), cell_width);
        let pad_top = cells(padding.top.evaluate_as_pixels(v_context), cell_height);
        let pad_bottom = cells(padding.bottom.evaluate_as_pixels(v_context), cell_height);

        let width = owner
            .width
            .saturating_sub(pad_left + pad_right)
            .max(MIN_FLOAT_WIDTH.min(owner.width));
        let height = owner
            .height
            .saturating_sub(pad_top + pad_bottom)
            .max(MIN_FLOAT_HEIGHT.min(owner.height));
        let left = owner.left + pad_left.min(owner.width - width);
        let top = owner.top + pad_top.min(owner.height - height);
        (left, top, width, height)
    }

    /// Returns the size of the split layout of a float covering the
    /// specified area, which loses its last row to the index bar
    fn float_terminal_size(&self, width: usize, height: usize) -> TerminalSize {
        let rows = height.saturating_sub(1).max(1);
        TerminalSize {
            rows,
            cols: width,
            pixel_width: width * self.render_metrics.cell_size.width as usize,
            pixel_height: rows * self.render_metrics.cell_size.height as usize,
            dpi: self.dimensions.dpi as u32,
        }
    }

    /// Returns the visible floats of the active tab whose owner panes are
    /// currently laid out (a zoomed pane hides the other panes and their
    /// floats). Floats are resized here to track their owner, so that
    /// window resizes, split changes and zooming all carry over to them.
    pub fn get_floats_to_render(&self) -> Vec<PositionedFloat> {
        let tab = match Mux::get().get_active_tab_for_window(self.mux_window_id) {
            Some(tab) => tab,
            None => return vec![],
        };
        if self.tab_state(tab.tab_id()).overlay.is_some() {
            return vec![];
        }
        let active = tab.get_active_pane().map(|p| p.pane_id());
        let owners = tab.iter_panes();

        let mut floats = vec![];
        for float in tab.iter_floats() {
            if !float.visible {
                continue;
            }
            let owner = match owners.iter().find(|p| p.pane.pane_id() == float.owner) {
                Some(owner) => owner.clone(),
                None => continue,
            };

            let (left, top, width, height) = self.float_area(&owner);
            let size = self.float_terminal_size(width, height);
            if float.tab.get_size() != size {
                float.tab.resize(size);
            }

            let is_focused = Some(float.owner) == active;
            let mut panes = float.tab.iter_panes();
            for p in &mut panes {
                p.left += left;
                p.top += top;
                p.is_active = p.is_active && is_focused;
                if let Some(overlay) = self.pane_state(p.pane.pane_id()).overlay.as_ref() {
                    p.pane = Arc::clone(&overlay.pane);
                }
            }
            let mut splits = float.tab.iter_splits();
            for s in &mut splits {
                s.left += left;
                s.top += top;
            }

            floats.push(PositionedFloat {
                float,
                is_focused,
                owner,
                left,
                top,
                width,
                height,
                panes,
                splits,
            });
        }
        floats
    }

    /// Hides the focused float, or shows the float of the active pane,
    /// spawning one in the pane's working directory if it has none
    pub fn toggle_floating_pane(&mut self) {
        let tab = match Mux::get().get_active_tab_for_window(self.mux_window_id) {
            Some(tab) => tab,
            None => return,
        };
        if self.tab_state(tab.tab_id()).overlay.is_some() {
            return;
        }
        let owner = match tab.get_active_pane() {
            Some(pane) => pane,
            None => return,
        };

        if let Some(float) = tab.get_float(owner.pane_id()) {
            tab.set_float_visible(float.owner, !float.visible);
        } else {
            let pos = match tab
                .iter_panes()
                .into_iter()
                .find(|p| p.pane.pane_id() == owner.pane_id())
            {
                Some(pos) => pos,
                None => return,
            };
            let (_, _, width, height) = self.float_area(&pos);
            let size = self.float_terminal_size(width, height);
            promise::spawn::spawn(async move {
                if let Err(err) = Mux::get().spawn_float(&tab, &owner, size).await {
                    log::error!("Failed to spawn floating pane: {:#}", err);
                }
            })
            .detach();
        }

        if let Some(window) = self.window.as_ref() {
            window.invalidate();
        }
    }

    /// Converts an area in window cell coordinates to pixels
    fn cell_area_to_pixels(
        &self,
        left: usize,
        top: usize,
        width: usize,
        height: usize,
    ) -> anyhow::Result<RectF> {
        let cell_width = self.render_metrics.cell_size.width as f32;
        let cell_height = self.render_metrics.cell_size.height as f32;
        let border = self.get_os_border();
        let first_row_offset = if self.show_tab_bar && !self.config.tab_bar_at_bottom {
            self.tab_bar_pixel_height()?
        } else {
            0.
        } + border.top.get() as f32;
        let (padding_left, padding_top) = self.padding_left_top();

        Ok(euclid::rect(
            padding_left + border.left.get() as f32 + left as f32 * cell_width,
            first_row_offset + padding_top + top as f32 * cell_height,
            width as f32 * cell_width,
            height as f32 * cell_height,
        ))
    }

    /// Paints the visible floats over the panes of the active tab
    pub fn paint_floats(
        &mut self,
        layers: &mut TripleLayerQuadAllocator,
        focused: bool,
    ) -> anyhow::Result<()> {
        for float in self.get_floats_to_render() {
            let palette = float.owner.pane.palette();
            let area = self.cell_area_to_pixels(float.left, float.top, float.width, float.height)?;
            // Cover the owner pane entirely, including any cells that the
            // float's own panes don't reach
            self.filled_rectangle(
                layers,
                0,
                area,
                palette.resolve_bg(ColorAttribute::Default).to_linear(),
            )?;

            // The scroll bar is drawn along the window edge, which isn't
            // where a float's active pane is
            let show_scroll_bar = std::mem::replace(&mut self.show_scroll_bar, false);
            let mut result = Ok(());
            for pos in &float.panes {
                if pos.is_active {
                    self.update_text_cursor(pos);
                    if focused {
                        pos.pane.advise_focus();
                        Mux::get().record_focus_for_current_identity(pos.pane.pane_id());
                    }
                }
                result = self.paint_pane(pos, layers);
                if result.is_err() {
                    break;
                }
            }
            self.show_scroll_bar = show_scroll_bar;
            result?;

            // Dragging a split resizes the window's tab, so the float's own
            // splits are drawn without registering them for dragging
            let num_ui_items = self.ui_items.len();
            for split in &float.splits {
                self.paint_split(layers, split, &float.owner.pane)?;
            }
            self.ui_items.truncate(num_ui_items);

            self.paint_float_index_bar(&float, layers)?;
            self.paint_float_border(area, layers)?;
        }
        Ok(())
    }

    /// Paints the index bar along the bottom row of a float, listing its
    /// panes by the index that ActivatePaneByIndex uses
    fn paint_float_index_bar(
        &mut self,
        float: &PositionedFloat,
        layers: &mut TripleLayerQuadAllocator,
    ) -> anyhow::Result<()> {
        let colors = self
            .config
            .resolved_palette
            .tab_bar
            .clone()
            .unwrap_or_else(TabBarColors::default);
        let background = CellAttributes::default()
            .set_background(ColorSpec::TrueColor(*colors.background()))
            .clone();

        let mut line = Line::from_text("", &background, SEQ_ZERO, None);
        // List every pane by its real index; a zoomed layout only lays out
        // the zoomed pane, and reports it as index 0
        let tab = &float.float.tab;
        let zoomed_id = tab.get_zoomed_pane().map(|p| p.pane_id());
        for pos in tab.iter_panes_ignoring_zoom() {
            let is_active = pos.index == tab.get_active_idx();
            let attrs = if is_active {
                colors.active_tab().as_cell_attributes()
            } else {
                colors.inactive_tab().as_cell_attributes()
            };
            let zoomed = if Some(pos.pane.pane_id()) == zoomed_id {
                " [Z]"
            } else {
                ""
            };
            let label = format!(" {}: {}{} ", pos.index, pos.pane.get_title(), zoomed);
            line.append_line(Line::from_text(&label, &attrs, SEQ_ZERO, None), SEQ_ZERO);
        }
        // Fill the rest of the row with the bar background, then clip
        // labels that don't fit
        let used = line.len();
        if used < float.width {
            let fill = " ".repeat(float.width - used);
            line.append_line(Line::from_text(&fill, &background, SEQ_ZERO, None), SEQ_ZERO);
        }
        line.resize(float.width, SEQ_ZERO);

        let area = self.cell_area_to_pixels(float.left, float.top + float.height - 1, float.width, 1)?;
        let palette = float.owner.pane.palette();
        let gl_state = self.render_state.as_ref().unwrap();
        let white_space = gl_state.util_sprites.white_space.texture_coords();
        let filled_box = gl_state.util_sprites.filled_box.texture_coords();
        let window_is_transparent =
            !self.window_background.is_empty() || self.config.window_background_opacity != 1.0;
        let default_bg = palette
            .resolve_bg(ColorAttribute::Default)
            .to_linear()
            .mul_alpha(if window_is_transparent {
                0.
            } else {
                self.config.text_background_opacity
            });

        self.render_screen_line(
            RenderScreenLineParams {
                top_pixel_y: area.min_y(),
                left_pixel_x: area.min_x(),
                pixel_width: area.width(),
                stable_line_idx: None,
                line: &line,
                selection: 0..0,
                cursor: &Default::default(),
                palette: &palette,
                dims: &RenderableDimensions {
                    cols: float.width,
                    physical_top: 0,
                    scrollback_rows: 0,
                    scrollback_top: 0,
                    viewport_rows: 1,
                    dpi: self.terminal_size.dpi,
                    pixel_height: self.render_metrics.cell_size.height as usize,
                    pixel_width: area.width() as usize,
                    reverse_video: false,
                },
                config: &self.config,
                cursor_border_color: LinearRgba::default(),
                foreground: palette.foreground.to_linear(),
                pane: None,
                is_active: true,
                selection_fg: LinearRgba::default(),
                selection_bg: LinearRgba::default(),
                cursor_fg: LinearRgba::default(),
                cursor_bg: LinearRgba::default(),
                cursor_is_default_color: true,
                white_space,
                filled_box,
                window_is_transparent,
                default_bg,
                style: None,
                font: None,
                use_pixel_positioning: self.config.experimental_pixel_positioning,
                render_metrics: self.render_metrics,
                shape_key: None,
                password_input: false,
            },
            layers,
        )?;
        Ok(())
    }

    /// Paints `floating_pane_border` around the outside of a float
    fn paint_float_border(
        &mut self,
        area: RectF,
        layers: &mut TripleLayerQuadAllocator,
    ) -> anyhow::Result<()> {
        let config = self.config.floating_pane_border.clone();
        let default_color = self.get_os_border().color;
        let h_context = DimensionContext {
            dpi: self.dimensions.dpi as f32,
            pixel_max: area.width(),
            pixel_cell: self.render_metrics.cell_size.width as f32,
        };
        let v_context = DimensionContext {
            dpi: self.dimensions.dpi as f32,
            pixel_max: area.height(),
            pixel_cell: self.render_metrics.cell_size.height as f32,
        };
        let left = config.left_width.evaluate_as_pixels(h_context);
        let right = config.right_width.evaluate_as_pixels(h_context);
        let top = config.top_height.evaluate_as_pixels(v_context);
        let bottom = config.bottom_height.evaluate_as_pixels(v_context);
        let color = |c: Option<config::RgbaColor>| c.map(|c| c.to_linear()).unwrap_or(default_color);

        let outer_x = area.min_x() - left;
        let outer_y = area.min_y() - top;
        let outer_width = area.width() + left + right;
        let outer_height = area.height() + top + bottom;

        self.filled_rectangle(
            layers,
            2,
            euclid::rect(outer_x, outer_y, outer_width, top),
            color(config.top_color),
        )?;
        self.filled_rectangle(
            layers,
            2,
            euclid::rect(outer_x, area.max_y(), outer_width, bottom),
            color(config.bottom_color),
        )?;
        self.filled_rectangle(
            layers,
            2,
            euclid::rect(outer_x, outer_y, left, outer_height),
            color(config.left_color),
        )?;
        self.filled_rectangle(
            layers,
            2,
            euclid::rect(area.max_x(), outer_y, right, outer_height),
            color(config.right_color),
        )?;
        Ok(())
    }
}
