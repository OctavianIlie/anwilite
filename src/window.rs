use crate::layout::{
    DEFAULT_SNAP_GAPS, SnapRect, SnapSide, snap_rect_with_gaps, tile_rects_with_ratio,
};
use crate::workspace::{WORKSPACE_COUNT, Workspaces};

use smithay::{
    desktop::{
        WindowSurfaceType,
        utils::{bbox_from_surface_tree, under_from_surface_tree},
    },
    utils::{Logical, Point, Rectangle},
    wayland::{
        compositor::with_states,
        shell::xdg::{SurfaceCachedState, ToplevelSurface},
    },
};
use std::time::{Duration, Instant};
use wayland_protocols::xdg::shell::server::xdg_toplevel;
use wayland_server::protocol::wl_surface::WlSurface;

const MIN_WIDTH: f64 = 120.0;
const MIN_HEIGHT: f64 = 90.0;
const RESIZE_INTERVAL: Duration = Duration::from_micros(8_333);

#[derive(Clone, Copy)]
struct NormalGeometry {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

#[derive(Clone, Copy)]
struct PendingUnsnap {
    target_size: (i32, i32),
    fraction_x: f64,
    grab_y: f64,
    pointer_x: f64,
    pointer_y: f64,
}

pub struct ManagedWindow {
    pub surface: ToplevelSurface,
    pub x: i32,
    pub y: i32,
    maximized: bool,
    snapped: Option<(SnapSide, SnapRect)>,
    saved_normal: Option<NormalGeometry>,
    restoring: Option<(NormalGeometry, Instant)>,
    pub workspace: usize,
    pending_unsnap: Option<PendingUnsnap>,
    tiled: Option<SnapRect>,
    pre_tile: Option<NormalGeometry>,
    minimized: bool,
    minimized_order: u64,
}

struct DragState {
    surface: ToplevelSurface,
    offset_x: f64,
    offset_y: f64,
}

#[derive(Clone)]
struct ResizeAnchor {
    surface: ToplevelSurface,
    edges: xdg_toplevel::ResizeEdge,
    left: i32,
    right: i32,
    top: i32,
    bottom: i32,
}

struct ResizeState {
    anchor: ResizeAnchor,
    press_x: f64,
    press_y: f64,
    start_width: i32,
    start_height: i32,
    wanted_size: Option<(i32, i32)>,
    last_sent_size: Option<(i32, i32)>,
    last_configure_at: Instant,
}

struct SettlingResize {
    anchor: ResizeAnchor,
    target_size: (i32, i32),
    started_at: Instant,
}

fn changes_left(edges: xdg_toplevel::ResizeEdge) -> bool {
    matches!(
        edges,
        xdg_toplevel::ResizeEdge::Left
            | xdg_toplevel::ResizeEdge::TopLeft
            | xdg_toplevel::ResizeEdge::BottomLeft
    )
}

fn changes_right(edges: xdg_toplevel::ResizeEdge) -> bool {
    matches!(
        edges,
        xdg_toplevel::ResizeEdge::Right
            | xdg_toplevel::ResizeEdge::TopRight
            | xdg_toplevel::ResizeEdge::BottomRight
    )
}

fn changes_top(edges: xdg_toplevel::ResizeEdge) -> bool {
    matches!(
        edges,
        xdg_toplevel::ResizeEdge::Top
            | xdg_toplevel::ResizeEdge::TopLeft
            | xdg_toplevel::ResizeEdge::TopRight
    )
}

fn changes_bottom(edges: xdg_toplevel::ResizeEdge) -> bool {
    matches!(
        edges,
        xdg_toplevel::ResizeEdge::Bottom
            | xdg_toplevel::ResizeEdge::BottomLeft
            | xdg_toplevel::ResizeEdge::BottomRight
    )
}

fn window_geometry(window: &ManagedWindow) -> Option<Rectangle<i32, Logical>> {
    let bounds = bbox_from_surface_tree(window.surface.wl_surface(), (0, 0));
    let geometry = with_states(window.surface.wl_surface(), |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .geometry
    })
    .and_then(|rect| rect.intersection(bounds))
    .unwrap_or(bounds);

    (geometry.size.w > 0 && geometry.size.h > 0).then_some(geometry)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutMode {
    #[default]
    Floating,
    Tiled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransferAction {
    KeepPlacement,
    RestoreFloating,
    Retile,
}

fn transfer_action(source: LayoutMode, destination: LayoutMode) -> TransferAction {
    match (source, destination) {
        (LayoutMode::Floating, LayoutMode::Floating) => TransferAction::KeepPlacement,

        (LayoutMode::Tiled, LayoutMode::Floating) => TransferAction::RestoreFloating,

        (_, LayoutMode::Tiled) => TransferAction::Retile,
    }
}

// Navigation follows tile order, not window stacking order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileFocusDirection {
    Previous,
    Next,
}

#[derive(Debug, Clone, Copy)]
enum TileSwapTarget {
    Adjacent(TileFocusDirection),
    Master,
}

fn adjacent_tile_index(
    current_index: usize,
    tile_count: usize,
    direction: TileFocusDirection,
) -> Option<usize> {
    if tile_count < 2 || current_index >= tile_count {
        return None;
    }

    Some(match direction {
        TileFocusDirection::Previous => {
            if current_index == 0 {
                tile_count - 1
            } else {
                current_index - 1
            }
        }
        TileFocusDirection::Next => {
            if current_index + 1 == tile_count {
                0
            } else {
                current_index + 1
            }
        }
    })
}

fn adjacent_swap_pair(
    tile_indices: &[usize],
    focused_index: usize,
    direction: TileFocusDirection,
) -> Option<(usize, usize)> {
    let focused_position = tile_indices
        .iter()
        .position(|&index| index == focused_index)?;

    let neighbor_position = adjacent_tile_index(focused_position, tile_indices.len(), direction)?;

    Some((focused_index, tile_indices[neighbor_position]))
}

fn master_swap_pair(tile_indices: &[usize], focused_index: usize) -> Option<(usize, usize)> {
    let &master_index = tile_indices.first()?;

    if focused_index == master_index || !tile_indices.contains(&focused_index) {
        return None;
    }

    Some((focused_index, master_index))
}

#[derive(Default)]
pub struct WindowManager {
    pub windows: Vec<ManagedWindow>,
    workspaces: Workspaces,

    last_focused: [Option<WlSurface>; WORKSPACE_COUNT],

    dragging: Option<DragState>,
    resizing: Option<ResizeState>,
    settling: Option<SettlingResize>,
    layout_modes: [LayoutMode; WORKSPACE_COUNT],
    minimize_counter: u64,
    master_ratios: [Option<u8>; WORKSPACE_COUNT],
}

impl WindowManager {
    pub fn add(&mut self, surface: ToplevelSurface) {
        let offset = (self.windows.len() % 8) as i32 * 48;

        // Remember which workspace is active.
        let workspace = self.workspaces.current_number();

        self.windows.insert(
            0,
            ManagedWindow {
                surface,
                x: 80 + offset,
                y: 80 + offset,
                workspace,
                maximized: false,
                saved_normal: None,
                restoring: None,
                snapped: None,
                pending_unsnap: None,
                tiled: None,
                pre_tile: None,
                minimized: false,
                minimized_order: 0,
            },
        );
    }

    pub fn current_workspace(&self) -> usize {
        self.workspaces.current_number()
    }

    pub fn current_layout_mode(&self) -> LayoutMode {
        self.layout_modes[self.current_workspace() - 1]
    }

    pub fn master_ratio_for_workspace(&self, workspace: usize) -> u8 {
        self.master_ratios[workspace - 1].unwrap_or(50)
    }

    pub fn set_master_ratio(&mut self, percent: u8, output_size: (i32, i32)) -> bool {
        if self.current_layout_mode() != LayoutMode::Tiled
            || self.is_dragging()
            || self.is_resizing()
            || self.settling.is_some()
        {
            return false;
        }

        if !(20..=80).contains(&percent) {
            return false;
        }

        let workspace = self.current_workspace();
        let old_percent = self.master_ratio_for_workspace(workspace);

        if old_percent == percent {
            return false;
        }

        let participants: Vec<&ManagedWindow> = self
            .windows
            .iter()
            .filter(|window| {
                window.workspace == workspace && window.surface.alive() && !window.minimized
            })
            .collect();

        if participants.iter().any(|window| {
            window.tiled.is_none() || window.pending_unsnap.is_some() || window.restoring.is_some()
        }) {
            return false;
        }

        // Validate before changing the stored ratio.
        if tile_rects_with_ratio(output_size, participants.len(), DEFAULT_SNAP_GAPS, percent)
            .is_none()
        {
            return false;
        }

        self.master_ratios[workspace - 1] = Some(percent);

        self.reflow_tiled_workspace(workspace, output_size);

        true
    }

    pub fn adjust_master_ratio(&mut self, delta: i8, output_size: (i32, i32)) -> bool {
        let current = self.master_ratio_for_workspace(self.current_workspace());

        let next = (i16::from(current) + i16::from(delta)).clamp(20, 80) as u8;

        self.set_master_ratio(next, output_size)
    }

    pub fn reset_master_ratio(&mut self, output_size: (i32, i32)) -> bool {
        self.set_master_ratio(50, output_size)
    }

    fn stop_tiling_workspace(&mut self, workspace: usize) {
        for window in self
            .windows
            .iter_mut()
            .filter(|window| window.workspace == workspace)
        {
            // A maximized tile is only zoomed within the tiled workspace.
            // Exiting tiling must also clear its maximized protocol state.
            let was_maximized_tile = window.tiled.is_some() && window.maximized;
            let saved = window.pre_tile.take();

            window.tiled = None;
            if was_maximized_tile {
                window.maximized = false;
            }

            let Some(normal) = saved else {
                if was_maximized_tile {
                    window.surface.with_pending_state(|state| {
                        state.states.unset(xdg_toplevel::State::Maximized);
                        state.size = None;
                    });
                    window.surface.send_configure();
                }
                continue;
            };

            if let Some(geometry) = window_geometry(window) {
                window.x = normal.left - geometry.loc.x;
                window.y = normal.top - geometry.loc.y;
            }

            // Return directly to the pre-tiling floating geometry rather than
            // passing through the old tiled rectangle.
            window.restoring = Some((normal, Instant::now()));

            window.surface.with_pending_state(|state| {
                if was_maximized_tile {
                    state.states.unset(xdg_toplevel::State::Maximized);
                }
                state.size = Some((normal.width, normal.height).into());
            });

            window.surface.send_configure();
        }

        self.layout_modes[workspace - 1] = LayoutMode::Floating;
    }

    pub fn reflow_for_surface(&mut self, surface: &WlSurface, output_size: (i32, i32)) -> bool {
        let Some(workspace) = self
            .windows
            .iter()
            .find(|window| window.surface.wl_surface() == surface)
            .map(|window| window.workspace)
        else {
            return false;
        };

        self.reflow_tiled_workspace(workspace, output_size)
    }

    pub fn reflow_all_tiled_workspaces(&mut self, output_size: (i32, i32)) -> bool {
        let mut changed = false;

        for workspace in 1..=WORKSPACE_COUNT {
            changed |= self.reflow_tiled_workspace(workspace, output_size);
        }

        changed
    }

    pub fn reflow_tiled_workspace(&mut self, workspace: usize, output_size: (i32, i32)) -> bool {
        if !(1..=WORKSPACE_COUNT).contains(&workspace) {
            return false;
        }

        if self.layout_modes[workspace - 1] != LayoutMode::Tiled {
            return false;
        }

        let indices: Vec<usize> = self
            .windows
            .iter()
            .enumerate()
            .filter(|(_, window)| {
                window.workspace == workspace && window.surface.alive() && !window.minimized
            })
            .map(|(index, _)| index)
            .collect();

        let master_percent = self.master_ratio_for_workspace(workspace);
        let Some(targets) = tile_rects_with_ratio(
            output_size,
            indices.len(),
            DEFAULT_SNAP_GAPS,
            master_percent,
        ) else {
            let maximized_surfaces: Vec<ToplevelSurface> = indices
                .iter()
                .filter_map(|&index| {
                    let window = &self.windows[index];

                    if window.tiled.is_some() && window.maximized {
                        Some(window.surface.clone())
                    } else {
                        None
                    }
                })
                .collect();

            for surface in maximized_surfaces {
                if !self.restore(&surface) {
                    return false;
                }
            }

            self.stop_tiling_workspace(workspace);
            return true;
        };

        let has_new_tile = indices
            .iter()
            .any(|&index| self.windows[index].tiled.is_none());

        if has_new_tile {
            let maximized_surfaces: Vec<ToplevelSurface> = indices
                .iter()
                .filter_map(|&index| {
                    let window = &self.windows[index];

                    if window.tiled.is_some() && window.maximized {
                        Some(window.surface.clone())
                    } else {
                        None
                    }
                })
                .collect();

            for surface in maximized_surfaces {
                if !self.restore(&surface) {
                    return false;
                }
            }
        }

        let mut planned = Vec::new();

        for (index, target) in indices.into_iter().zip(targets) {
            let window = &self.windows[index];

            // A floating maximized window may enter tiling only if we can
            // restore its original geometry when tiling is switched off.
            if (window.maximized && window.tiled.is_none() && window.saved_normal.is_none())
                || window.pending_unsnap.is_some()
                || window.restoring.is_some()
                || (window.snapped.is_some() && window.saved_normal.is_none())
            {
                return false;
            }

            let Some(geometry) = window_geometry(window) else {
                return false;
            };

            // Preserve the original floating geometry.
            let normal = if window.tiled.is_none() {
                if let Some(saved) = window.pre_tile {
                    Some(saved)
                } else if window.maximized || window.snapped.is_some() {
                    window.saved_normal
                } else {
                    Some(NormalGeometry {
                        left: window.x + geometry.loc.x,
                        top: window.y + geometry.loc.y,
                        width: geometry.size.w,
                        height: geometry.size.h,
                    })
                }
            } else {
                None
            };

            if window.tiled == Some(target) {
                continue;
            }

            planned.push((index, target, normal, geometry.loc.x, geometry.loc.y));
        }

        let changed = !planned.is_empty();

        for (index, target, normal, offset_x, offset_y) in planned {
            let window = &mut self.windows[index];
            let entering_from_maximized = window.maximized && window.tiled.is_none();

            if window.snapped.is_some() {
                window.snapped = None;
                window.saved_normal = None;
            }

            if let Some(normal) = normal {
                window.pre_tile = Some(normal);
            }

            if entering_from_maximized {
                // The original floating geometry now lives in pre_tile.
                window.maximized = false;
                window.saved_normal = None;
            }

            window.tiled = Some(target);

            // A tile that was *already* maximized stays zoomed in.
            if window.maximized {
                continue;
            }

            window.x = target.x - offset_x;
            window.y = target.y - offset_y;

            window.surface.with_pending_state(|state| {
                if entering_from_maximized {
                    state.states.unset(xdg_toplevel::State::Maximized);
                }
                state.size = Some((target.width, target.height).into());
            });

            window.surface.send_pending_configure();
        }

        changed
    }

    pub fn toggle_tiling(&mut self, output_size: (i32, i32)) -> bool {
        if self.dragging.is_some() || self.resizing.is_some() || self.settling.is_some() {
            return false;
        }

        let workspace = self.current_workspace();
        let slot = workspace - 1;

        // TILED -> FLOATING (also works when a tile is maximized).
        if self.layout_modes[slot] == LayoutMode::Tiled {
            self.stop_tiling_workspace(workspace);
            return true;
        }

        // FLOATING -> TILED
        let windows: Vec<&ManagedWindow> = self
            .windows
            .iter()
            .filter(|window| {
                window.workspace == workspace && window.surface.alive() && !window.minimized
            })
            .collect();

        if windows.iter().any(|window| {
            window.restoring.is_some()
                || window.pending_unsnap.is_some()
                || window.tiled.is_some()
                || ((window.maximized || window.snapped.is_some()) && window.saved_normal.is_none())
                || window_geometry(window).is_none()
        }) {
            return false;
        }

        if tile_rects_with_ratio(
            output_size,
            windows.len(),
            DEFAULT_SNAP_GAPS,
            self.master_ratio_for_workspace(workspace),
        )
        .is_none()
        {
            return false;
        }

        let has_windows = !windows.is_empty();
        self.layout_modes[slot] = LayoutMode::Tiled;

        if !self.reflow_tiled_workspace(workspace, output_size) && has_windows {
            self.layout_modes[slot] = LayoutMode::Floating;
            return false;
        }

        true
    }

    pub fn switch_to_workspace(&mut self, number: usize) -> bool {
        if self.is_dragging() || self.is_resizing() {
            return false;
        }
        self.workspaces.switch_to(number)
    }

    pub fn move_focused_to_workspace(
        &mut self,
        focused: &WlSurface,
        number: usize,
        output_size: (i32, i32),
    ) -> bool {
        if self.is_dragging() || self.is_resizing() || self.settling.is_some() {
            return false;
        }

        let current = self.current_workspace();

        if !(1..=WORKSPACE_COUNT).contains(&number) || number == current {
            return false;
        }

        let Some(index) = self.windows.iter().position(|window| {
            window.workspace == current
                && window.surface.alive()
                && window.surface.wl_surface() == focused
        }) else {
            return false;
        };

        let source_mode = self.layout_modes[current - 1];
        let destination_mode = self.layout_modes[number - 1];

        let action = transfer_action(source_mode, destination_mode);

        // Validate the window before modifying workspace state.
        let window = &self.windows[index];

        if window.pending_unsnap.is_some() || window.restoring.is_some() || window.maximized {
            return false;
        }

        if action == TransferAction::RestoreFloating
            && window.tiled.is_some()
            && window.pre_tile.is_none()
        {
            return false;
        }

        // Switch to the target workspace first.
        if !self.workspaces.switch_to(number) {
            return false;
        }

        let mut window = self.windows.remove(index);

        window.workspace = number;

        // TILED -> FLOATING:
        // Return the window to its original floating geometry.
        if action == TransferAction::RestoreFloating {
            if window.tiled.take().is_some() {
                if let Some(normal) = window.pre_tile.take() {
                    if let Some(geometry) = window_geometry(&window) {
                        window.x = normal.left - geometry.loc.x;
                        window.y = normal.top - geometry.loc.y;
                    }

                    // The client still needs time to resize.
                    window.restoring = Some((normal, Instant::now()));

                    window.surface.with_pending_state(|state| {
                        state.size = Some((normal.width, normal.height).into());
                    });

                    window.surface.send_configure();
                }
            }
        }

        self.windows.insert(0, window);

        if self.last_focused[current - 1].as_ref() == Some(focused) {
            self.last_focused[current - 1] = None;
        }

        self.last_focused[number - 1] = Some(focused.clone());

        if source_mode == LayoutMode::Tiled {
            self.reflow_tiled_workspace(current, output_size);
        }

        if destination_mode == LayoutMode::Tiled {
            self.reflow_tiled_workspace(number, output_size);
        }

        true
    }

    pub fn adjacent_tiled_surface(
        &self,
        focused: Option<&WlSurface>,
        direction: TileFocusDirection,
    ) -> Option<WlSurface> {
        if self.current_layout_mode() != LayoutMode::Tiled {
            return None;
        }

        // reflow_tiled_workspace() uses this same Vec order for master + stack.
        // is_window_visible() excludes minimized and maximization-hidden tiles.
        let tiles: Vec<WlSurface> = self
            .windows
            .iter()
            .filter(|window| window.tiled.is_some() && self.is_window_visible(window))
            .map(|window| window.surface.wl_surface().clone())
            .collect();

        let focused = focused?;
        let current_index = tiles.iter().position(|surface| surface == focused)?;
        let next_index = adjacent_tile_index(current_index, tiles.len(), direction)?;

        Some(tiles[next_index].clone())
    }

    pub fn swap_focused_tile(
        &mut self,
        focused: &WlSurface,
        direction: TileFocusDirection,
        output_size: (i32, i32),
    ) -> bool {
        self.swap_tile(focused, TileSwapTarget::Adjacent(direction), output_size)
    }

    pub fn promote_focused_tile(&mut self, focused: &WlSurface, output_size: (i32, i32)) -> bool {
        self.swap_tile(focused, TileSwapTarget::Master, output_size)
    }

    fn swap_tile(
        &mut self,
        focused: &WlSurface,
        target: TileSwapTarget,
        output_size: (i32, i32),
    ) -> bool {
        if self.current_layout_mode() != LayoutMode::Tiled
            || self.is_dragging()
            || self.is_resizing()
            || self.settling.is_some()
        {
            return false;
        }
        let workspace = self.current_workspace();

        let indices: Vec<usize> = self
            .windows
            .iter()
            .enumerate()
            .filter(|(_, window)| {
                window.workspace == workspace && window.surface.alive() && !window.minimized
            })
            .map(|(index, _)| index)
            .collect();
        if indices.len() < 2 {
            return false;
        }
        if indices.iter().any(|&index| {
            let window = &self.windows[index];

            window.tiled.is_none()
                || window.maximized
                || window.pending_unsnap.is_some()
                || window.restoring.is_some()
                || window_geometry(window).is_none()
        }) {
            return false;
        }

        // Make sure the output can fit the current layout.
        let ratio = self.master_ratio_for_workspace(workspace);

        if tile_rects_with_ratio(output_size, indices.len(), DEFAULT_SNAP_GAPS, ratio).is_none() {
            return false;
        }

        // Find the focused window in the visible tile sequence.
        let Some(focused_index) = indices
            .iter()
            .copied()
            .find(|&index| self.windows[index].surface.wl_surface() == focused)
        else {
            return false;
        };

        let pair = match target {
            TileSwapTarget::Adjacent(direction) => {
                adjacent_swap_pair(&indices, focused_index, direction)
            }
            TileSwapTarget::Master => master_swap_pair(&indices, focused_index),
        };

        let Some((first, second)) = pair else {
            return false;
        };

        // Swap entire ManagedWindow entries, not their saved geometry.
        self.windows.swap(first, second);

        // Assign new rectangles according to the new tile order.
        if self.reflow_tiled_workspace(workspace, output_size) {
            true
        } else {
            // Restore the original order if reflow fails.
            self.windows.swap(first, second);
            false
        }
    }

    pub fn focused_surface(&self) -> Option<WlSurface> {
        let current = self.current_workspace();

        if let Some(saved) = &self.last_focused[current - 1] {
            let still_valid = self.windows.iter().any(|window| {
                self.is_window_visible(window) && window.surface.wl_surface() == saved
            });

            if still_valid {
                return Some(saved.clone());
            }
        }

        self.windows
            .iter()
            .find(|window| self.is_window_visible(window))
            .map(|window| window.surface.wl_surface().clone())
    }

    pub fn is_window_visible(&self, window: &ManagedWindow) -> bool {
        let current = self.current_workspace();

        if window.workspace != current || !window.surface.alive() || window.minimized {
            return false;
        }

        let maximized_tile = self.windows.iter().find(|other| {
            other.workspace == current
                && other.surface.alive()
                && other.tiled.is_some()
                && other.maximized
                && !other.minimized
        });
        match maximized_tile {
            Some(zoomed) => zoomed.surface == window.surface,
            None => true,
        }
    }

    pub fn focus_is_valid(&self, surface: &WlSurface) -> bool {
        self.windows
            .iter()
            .any(|window| self.is_window_visible(window) && window.surface.wl_surface() == surface)
    }

    pub fn remember_focus(&mut self, focused: Option<&WlSurface>) {
        let Some(surface) = focused else {
            return;
        };

        let Some(window) = self
            .windows
            .iter()
            .find(|window| window.surface.alive() && window.surface.wl_surface() == surface)
        else {
            return;
        };
        let workspace_index = window.workspace - 1;

        self.last_focused[workspace_index] = Some(surface.clone());
    }

    pub fn retain_live(&mut self) {
        self.windows.retain(|window| window.surface.alive());

        if self
            .dragging
            .as_ref()
            .is_some_and(|drag| !drag.surface.alive())
        {
            self.dragging = None;
        }
        if self
            .resizing
            .as_ref()
            .is_some_and(|resize| !resize.anchor.surface.alive())
        {
            self.resizing = None;
        }
        if self
            .settling
            .as_ref()
            .is_some_and(|settling| !settling.anchor.surface.alive())
        {
            self.settling = None;
        }
    }

    fn hit_window(window: &ManagedWindow, px: f64, py: f64) -> bool {
        under_from_surface_tree(
            window.surface.wl_surface(),
            (px, py).into(),
            (window.x, window.y),
            WindowSurfaceType::TOPLEVEL | WindowSurfaceType::SUBSURFACE,
        )
        .is_some()
    }

    pub fn toplevel_at(&self, px: f64, py: f64) -> Option<ToplevelSurface> {
        self.windows
            .iter()
            .find(|window| self.is_window_visible(window) && Self::hit_window(window, px, py))
            .map(|window| window.surface.clone())
    }

    pub fn surface_at(&self, px: f64, py: f64) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.windows.iter().find_map(|window| {
            if !self.is_window_visible(window) {
                return None;
            }

            let (surface, location) = under_from_surface_tree(
                window.surface.wl_surface(),
                (px, py).into(),
                (window.x, window.y),
                WindowSurfaceType::TOPLEVEL | WindowSurfaceType::SUBSURFACE,
            )?;

            Some((surface, (location.x as f64, location.y as f64).into()))
        })
    }

    pub fn toplevel_for_surface(&self, focused: &WlSurface) -> Option<ToplevelSurface> {
        self.windows
            .iter()
            .find(|window| self.is_window_visible(window) && window.surface.wl_surface() == focused)
            .map(|window| window.surface.clone())
    }

    pub fn raise_at(&mut self, px: f64, py: f64) -> Option<WlSurface> {
        let current = self.current_workspace();

        let index = self.windows.iter().position(|window| {
            self.is_window_visible(window) && Self::hit_window(window, px, py)
        })?;

        let surface = self.windows[index].surface.wl_surface().clone();

        // Floating windows are raised when clicked.
        // Tiled windows keep their layout order.
        if self.layout_modes[current - 1] == LayoutMode::Floating {
            let window = self.windows.remove(index);
            self.windows.insert(0, window);
        }

        Some(surface)
    }

    pub fn snap(&mut self, focused: &WlSurface, side: SnapSide, output_size: (i32, i32)) -> bool {
        if self.is_dragging() || self.is_resizing() {
            return false;
        }

        let Some(target) = snap_rect_with_gaps(output_size, side, DEFAULT_SNAP_GAPS) else {
            return false;
        };

        let current = self.current_workspace();

        let Some(window) = self.windows.iter_mut().find(|window| {
            window.workspace == current
                && window.surface.alive()
                && window.surface.wl_surface() == focused
        }) else {
            return false;
        };

        let Some(geometry) = window_geometry(window) else {
            return false;
        };

        // Save the original floating geometry once.
        if let Some((normal, _)) = window.restoring {
            window.saved_normal = Some(normal);
        } else if !window.maximized && window.snapped.is_none() {
            window.saved_normal = Some(NormalGeometry {
                left: window.x + geometry.loc.x,
                top: window.y + geometry.loc.y,
                width: geometry.size.w,
                height: geometry.size.h,
            });
        }

        if window.tiled.is_some() || window.pending_unsnap.is_some() {
            return false;
        }

        window.restoring = None;
        window.maximized = false;
        window.snapped = Some((side, target));

        window.x = target.x - geometry.loc.x;
        window.y = target.y - geometry.loc.y;

        window.surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Maximized);
            state.size = Some((target.width, target.height).into());
        });

        window.surface.send_configure();

        true
    }

    pub fn maximize(&mut self, surface: &ToplevelSurface, output_size: (i32, i32)) -> bool {
        // Don't maximize during an interactive drag or resize.
        if self.dragging.is_some() || self.resizing.is_some() {
            return false;
        }

        let active_workspace = self.current_workspace();

        // Count tiled windows before borrowing a window mutably.
        let tiled_count = self
            .windows
            .iter()
            .filter(|window| {
                window.workspace == active_workspace
                    && window.surface.alive()
                    && !window.minimized
                    && window.tiled.is_some()
            })
            .count();

        // Locate the requested window.
        let Some(index) = self
            .windows
            .iter()
            .position(|window| &window.surface == surface)
        else {
            return false;
        };

        let workspace_window_count = self
            .windows
            .iter()
            .filter(|window| {
                window.workspace == active_workspace && window.surface.alive() && !window.minimized
            })
            .count();

        // Validate the transition before changing anything.
        if self.windows[index].pending_unsnap.is_some() {
            return false;
        }

        // Only one tiled window may be maximized at a time.
        let another_tile_is_maximized =
            self.windows
                .iter()
                .enumerate()
                .any(|(other_index, window)| {
                    other_index != index
                        && window.workspace == active_workspace
                        && window.surface.alive()
                        && window.tiled.is_some()
                        && window.maximized
                });

        if self.windows[index].minimized {
            return false;
        }

        if self.windows[index].tiled.is_some()
            && (self.windows[index].workspace != active_workspace
                || tiled_count == 0
                || tiled_count != workspace_window_count
                || another_tile_is_maximized)
        {
            return false;
        }

        if self
            .settling
            .as_ref()
            .is_some_and(|s| &s.anchor.surface == surface)
        {
            self.settling = None;
        }
        let window = &mut self.windows[index];
        let Some(geometry) = window_geometry(window) else {
            return false;
        };

        // TILED -> MAXIMIZED

        if window.tiled.is_some() {
            // Preserve window.tiled and window.pre_tile.
            window.maximized = true;

            // Maximized windows cover the entire output.
            window.x = -geometry.loc.x;
            window.y = -geometry.loc.y;

            window.surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Maximized);
                state.size = Some(output_size.into());
            });

            window.surface.send_configure();

            return true;
        }

        // FLOATING OR SNAPPED -> MAXIMIZED

        // Preserve original floating geometry.
        if let Some((normal, _)) = window.restoring {
            window.saved_normal = Some(normal);
        } else if !window.maximized && window.snapped.is_none() {
            window.saved_normal = Some(NormalGeometry {
                left: window.x + geometry.loc.x,
                top: window.y + geometry.loc.y,
                width: geometry.size.w,
                height: geometry.size.h,
            });
        }

        window.restoring = None;
        window.maximized = true;
        window.snapped = None;

        window.x = -geometry.loc.x;
        window.y = -geometry.loc.y;

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Maximized);
            state.size = Some(output_size.into());
        });

        surface.send_configure();

        true
    }

    pub fn restore(&mut self, surface: &ToplevelSurface) -> bool {
        let Some(window) = self.windows.iter_mut().find(|w| &w.surface == surface) else {
            return false;
        };
        if window.maximized {
            if let Some(target) = window.tiled {
                window.maximized = false;

                if let Some(geometry) = window_geometry(window) {
                    window.x = target.x - geometry.loc.x;
                    window.y = target.y - geometry.loc.y;
                }

                window.surface.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Maximized);
                    state.size = Some((target.width, target.height).into());
                });
                window.surface.send_configure();
                return true;
            }
        }
        if !window.maximized && window.snapped.is_none() {
            return false;
        }

        let saved = window.saved_normal.take();
        if let Some(normal) = saved {
            if let Some(geometry) = window_geometry(window) {
                window.x = normal.left - geometry.loc.x;
                window.y = normal.top - geometry.loc.y;
            }

            window.restoring = Some((normal, Instant::now()));
        }
        window.maximized = false;
        window.snapped = None;
        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Maximized);
            state.size = saved.map(|normal| (normal.width, normal.height).into());
        });
        surface.send_configure();
        true
    }

    pub fn minimize_window(&mut self, surface: &ToplevelSurface, output_size: (i32, i32)) -> bool {
        // Don't interrupt an active interactive operation.
        if self.is_dragging() || self.is_resizing() {
            return false;
        }

        let Some(index) = self
            .windows
            .iter()
            .position(|window| &window.surface == surface && window.surface.alive())
        else {
            return false;
        };

        if self.windows[index].minimized || self.windows[index].pending_unsnap.is_some() {
            return false;
        }

        let workspace = self.windows[index].workspace;
        let tiled_mode = self.layout_modes[workspace - 1] == LayoutMode::Tiled;

        // A maximized tile must first leave maximized mode.
        // Otherwise it could keep hiding its neighboring tile.
        if self.windows[index].tiled.is_some()
            && self.windows[index].maximized
            && !self.restore(surface)
        {
            return false;
        }

        self.minimize_counter = self.minimize_counter.saturating_add(1);

        {
            let window = &mut self.windows[index];

            window.minimized = true;
            window.minimized_order = self.minimize_counter;

            if tiled_mode {
                window.tiled = None;
            }
        }

        if tiled_mode {
            self.reflow_tiled_workspace(workspace, output_size);
        }

        true
    }

    pub fn restore_last_minimized(&mut self, output_size: (i32, i32)) -> Option<WlSurface> {
        let workspace = self.current_workspace();

        let surface = self
            .windows
            .iter()
            .filter(|window| {
                window.workspace == workspace && window.surface.alive() && window.minimized
            })
            .max_by_key(|window| window.minimized_order)
            .map(|window| window.surface.clone())?;

        let index = self
            .windows
            .iter()
            .position(|window| window.surface == surface)?;

        if self.layout_modes[workspace - 1] == LayoutMode::Tiled {
            let visible_count = self
                .windows
                .iter()
                .filter(|window| {
                    window.workspace == workspace && window.surface.alive() && !window.minimized
                })
                .count();

            let next_count = visible_count.checked_add(1)?;

            tile_rects_with_ratio(
                output_size,
                next_count,
                DEFAULT_SNAP_GAPS,
                self.master_ratio_for_workspace(workspace),
            )?;
        }

        self.windows[index].minimized = false;

        if self.layout_modes[workspace - 1] == LayoutMode::Tiled {
            self.reflow_tiled_workspace(workspace, output_size);
        }

        Some(surface.wl_surface().clone())
    }

    pub fn resize_placed_windows(&mut self, output_size: (i32, i32)) {
        for window in &mut self.windows {
            if window.maximized {
                window.surface.with_pending_state(|state| {
                    state.size = Some(output_size.into());
                });

                window.surface.send_pending_configure();
            } else if let Some((side, _)) = window.snapped {
                let Some(target) = snap_rect_with_gaps(output_size, side, DEFAULT_SNAP_GAPS) else {
                    continue;
                };

                window.snapped = Some((side, target));

                if let Some(geometry) = window_geometry(window) {
                    window.x = target.x - geometry.loc.x;
                    window.y = target.y - geometry.loc.y;
                }

                window.surface.with_pending_state(|state| {
                    state.size = Some((target.width, target.height).into());
                });

                window.surface.send_pending_configure();
            }
        }

        // Recalculate tiled geometry for every tiled workspace.
        for workspace in 1..=WORKSPACE_COUNT {
            if self.layout_modes[workspace - 1] != LayoutMode::Tiled {
                continue;
            }

            let indices: Vec<usize> = self
                .windows
                .iter()
                .enumerate()
                .filter(|(_, window)| {
                    window.workspace == workspace
                        && window.tiled.is_some()
                        && window.surface.alive()
                        && !window.minimized
                })
                .map(|(index, _)| index)
                .collect();

            let master_percent = self.master_ratio_for_workspace(workspace);
            let Some(rectangles) = tile_rects_with_ratio(
                output_size,
                indices.len(),
                DEFAULT_SNAP_GAPS,
                master_percent,
            ) else {
                continue;
            };

            for (index, target) in indices.into_iter().zip(rectangles) {
                let window = &mut self.windows[index];

                if window.tiled == Some(target) {
                    continue;
                }

                window.tiled = Some(target);
                if window.maximized {
                    continue;
                }

                if let Some(geometry) = window_geometry(window) {
                    window.x = target.x - geometry.loc.x;
                    window.y = target.y - geometry.loc.y;
                }

                window.surface.with_pending_state(|state| {
                    state.size = Some((target.width, target.height).into());
                });

                window.surface.send_pending_configure();
            }
        }
    }

    pub fn sync_mode_position(&mut self, surface: &WlSurface) {
        let Some(window) = self
            .windows
            .iter_mut()
            .find(|w| w.surface.wl_surface() == surface)
        else {
            return;
        };
        let Some(geometry) = window_geometry(window) else {
            return;
        };

        if let Some(pending) = window.pending_unsnap {
            if (geometry.size.w, geometry.size.h) != pending.target_size {
                return;
            }
            let left = pending.pointer_x - pending.fraction_x * geometry.size.w as f64;
            let top = pending.pointer_y - pending.grab_y;
            window.x = left.round() as i32 - geometry.loc.x;
            window.y = top.round() as i32 - geometry.loc.y;
            window.pending_unsnap = None;

            if let Some(drag) = self.dragging.as_mut() {
                if drag.surface == window.surface {
                    drag.offset_x = pending.pointer_x - window.x as f64;
                    drag.offset_y = pending.pointer_y - window.y as f64;
                }
            }
            return;
        }
        if !window.maximized {
            if let Some(target) = window.tiled {
                window.x = target.x - geometry.loc.x;
                window.y = target.y - geometry.loc.y;
                return;
            }
        }

        if window.maximized {
            window.x = -geometry.loc.x;
            window.y = -geometry.loc.y;
        } else if let Some((_, target)) = window.snapped {
            window.x = target.x - geometry.loc.x;
            window.y = target.y - geometry.loc.y;
        } else if let Some((normal, started)) = window.restoring {
            window.x = normal.left - geometry.loc.x;
            window.y = normal.top - geometry.loc.y;
            if (geometry.size.w == normal.width && geometry.size.h == normal.height)
                || started.elapsed() > Duration::from_secs(2)
            {
                window.restoring = None;
            }
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging.is_some()
    }

    pub fn start_drag(&mut self, px: f64, py: f64) {
        if let Some(surface) = self.toplevel_at(px, py) {
            self.start_drag_for(&surface, px, py);
        }
    }

    pub fn start_drag_for(&mut self, surface: &ToplevelSurface, px: f64, py: f64) -> bool {
        if self.dragging.is_some() || self.resizing.is_some() {
            return false;
        }

        let current = self.current_workspace();

        let Some(window) = self.windows.iter_mut().find(|window| {
            window.workspace == current && &window.surface == surface && window.surface.alive()
        }) else {
            return false;
        };

        if window.maximized || window.pending_unsnap.is_some() || window.tiled.is_some() {
            return false;
        }

        if window.snapped.is_some() {
            let Some(geometry) = window_geometry(window) else {
                return false;
            };

            let Some(normal) = window.saved_normal.take() else {
                return false;
            };

            let left = window.x as f64 + geometry.loc.x as f64;
            let top = window.y as f64 + geometry.loc.y as f64;

            let fraction_x = ((px - left) / geometry.size.w as f64).clamp(0.0, 1.0);

            let grab_y = (py - top).clamp(0.0, normal.height.saturating_sub(1).max(0) as f64);

            window.pending_unsnap = Some(PendingUnsnap {
                target_size: (normal.width, normal.height),
                fraction_x,
                grab_y,
                pointer_x: px,
                pointer_y: py,
            });

            window.snapped = None;
            window.restoring = None;

            window.surface.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
                state.size = Some((normal.width, normal.height).into());
            });

            window.surface.send_configure();
        }

        self.dragging = Some(DragState {
            surface: window.surface.clone(),
            offset_x: px - window.x as f64,
            offset_y: py - window.y as f64,
        });

        true
    }

    pub fn update_drag(&mut self, px: f64, py: f64) {
        let Some(drag) = self.dragging.as_ref() else {
            return;
        };

        let Some(window) = self
            .windows
            .iter_mut()
            .find(|window| window.surface == drag.surface)
        else {
            return;
        };

        if let Some(pending) = window.pending_unsnap.as_mut() {
            pending.pointer_x = px;
            pending.pointer_y = py;
            return;
        }

        window.x = (px - drag.offset_x).round() as i32;
        window.y = (py - drag.offset_y).round() as i32;
    }

    pub fn end_drag(&mut self) {
        self.dragging = None;
    }

    pub fn is_resizing(&self) -> bool {
        self.resizing.is_some()
    }

    pub fn start_resize_for(
        &mut self,
        surface: &ToplevelSurface,
        px: f64,
        py: f64,
        edges: xdg_toplevel::ResizeEdge,
    ) -> bool {
        if self.dragging.is_some()
            || self.resizing.is_some()
            || edges == xdg_toplevel::ResizeEdge::None
        {
            return false;
        }
        let Some(window) = self
            .windows
            .iter()
            .find(|window| &window.surface == surface)
        else {
            return false;
        };
        if window.maximized
            || window.snapped.is_some()
            || window.pending_unsnap.is_some()
            || window.tiled.is_some()
        {
            return false;
        }
        let Some(geometry) = window_geometry(window) else {
            return false;
        };

        let left = window.x + geometry.loc.x;
        let top = window.y + geometry.loc.y;
        let anchor = ResizeAnchor {
            surface: surface.clone(),
            edges,
            left,
            right: left + geometry.size.w,
            top,
            bottom: top + geometry.size.h,
        };

        self.settling = None;
        self.resizing = Some(ResizeState {
            anchor,
            press_x: px,
            press_y: py,
            start_width: geometry.size.w,
            start_height: geometry.size.h,
            wanted_size: None,
            last_sent_size: None,
            last_configure_at: Instant::now(),
        });

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
        });
        surface.send_pending_configure();
        true
    }

    pub fn update_resize(&mut self, px: f64, py: f64) {
        let Some(resize) = self.resizing.as_mut() else {
            return;
        };
        if !resize.anchor.surface.alive() {
            self.resizing = None;
            return;
        }

        let dx = px - resize.press_x;
        let dy = py - resize.press_y;
        let edges = resize.anchor.edges;
        let width = if changes_left(edges) {
            resize.start_width as f64 - dx
        } else if changes_right(edges) {
            resize.start_width as f64 + dx
        } else {
            resize.start_width as f64
        };
        let height = if changes_top(edges) {
            resize.start_height as f64 - dy
        } else if changes_bottom(edges) {
            resize.start_height as f64 + dy
        } else {
            resize.start_height as f64
        };
        resize.wanted_size = Some((
            width.round().max(MIN_WIDTH) as i32,
            height.round().max(MIN_HEIGHT) as i32,
        ));
    }

    pub fn pending_resize_wait(&self) -> Option<Duration> {
        let resize = self.resizing.as_ref()?;
        if !resize.anchor.surface.alive() || resize.wanted_size == resize.last_sent_size {
            return None;
        }
        resize.wanted_size?;
        Some(RESIZE_INTERVAL.saturating_sub(resize.last_configure_at.elapsed()))
    }

    pub fn flush_resize(&mut self) {
        let Some(resize) = self.resizing.as_mut() else {
            return;
        };
        if !resize.anchor.surface.alive() {
            self.resizing = None;
            return;
        }
        if resize.last_configure_at.elapsed() < RESIZE_INTERVAL {
            return;
        }
        let Some(size) = resize.wanted_size else {
            return;
        };
        if resize.last_sent_size == Some(size) {
            return;
        }

        resize.anchor.surface.with_pending_state(|state| {
            state.size = Some(size.into());
            state.states.set(xdg_toplevel::State::Resizing);
        });
        resize.anchor.surface.send_pending_configure();
        resize.last_sent_size = Some(size);
        resize.last_configure_at = Instant::now();
    }

    pub fn sync_resize_position(&mut self, surface: &WlSurface) {
        // The settling anchor is only needed briefly after release.
        if self
            .settling
            .as_ref()
            .is_some_and(|s| s.started_at.elapsed() > Duration::from_secs(2))
        {
            self.settling = None;
        }

        let anchor = self
            .resizing
            .as_ref()
            .map(|r| &r.anchor)
            .or_else(|| self.settling.as_ref().map(|s| &s.anchor))
            .filter(|anchor| anchor.surface.wl_surface() == surface)
            .cloned();
        let Some(anchor) = anchor else {
            return;
        };
        let Some(window) = self
            .windows
            .iter_mut()
            .find(|window| window.surface == anchor.surface)
        else {
            return;
        };
        let Some(geometry) = window_geometry(window) else {
            return;
        };

        if changes_left(anchor.edges) {
            window.x = anchor.right - geometry.size.w - geometry.loc.x;
        } else {
            window.x = anchor.left - geometry.loc.x;
        }
        if changes_top(anchor.edges) {
            window.y = anchor.bottom - geometry.size.h - geometry.loc.y;
        } else {
            window.y = anchor.top - geometry.loc.y;
        }

        if self.settling.as_ref().is_some_and(|s| {
            s.anchor.surface == anchor.surface
                && s.target_size == (geometry.size.w, geometry.size.h)
        }) {
            self.settling = None;
        }
    }

    pub fn finish_resize(&mut self) {
        let Some(resize) = self.resizing.take() else {
            return;
        };
        if !resize.anchor.surface.alive() {
            return;
        }
        let target_size = resize
            .wanted_size
            .unwrap_or((resize.start_width, resize.start_height));

        if changes_left(resize.anchor.edges) || changes_top(resize.anchor.edges) {
            self.settling = Some(SettlingResize {
                anchor: resize.anchor.clone(),
                target_size,
                started_at: Instant::now(),
            });
        }

        resize.anchor.surface.with_pending_state(|state| {
            if resize.last_sent_size != Some(target_size) {
                state.size = Some(target_size.into());
            }
            state.states.unset(xdg_toplevel::State::Resizing);
        });
        resize.anchor.surface.send_pending_configure();
    }
}

#[cfg(test)]
mod tile_focus_tests {
    use super::{TileFocusDirection, adjacent_tile_index};

    #[test]
    fn next_tile_walks_forward_and_wraps() {
        assert_eq!(adjacent_tile_index(0, 4, TileFocusDirection::Next), Some(1));
        assert_eq!(adjacent_tile_index(2, 4, TileFocusDirection::Next), Some(3));
        assert_eq!(adjacent_tile_index(3, 4, TileFocusDirection::Next), Some(0));
    }

    #[test]
    fn previous_tile_walks_backward_and_wraps() {
        assert_eq!(
            adjacent_tile_index(3, 4, TileFocusDirection::Previous),
            Some(2)
        );
        assert_eq!(
            adjacent_tile_index(1, 4, TileFocusDirection::Previous),
            Some(0)
        );
        assert_eq!(
            adjacent_tile_index(0, 4, TileFocusDirection::Previous),
            Some(3)
        );
    }

    #[test]
    fn navigation_rejects_missing_or_single_tile() {
        assert_eq!(adjacent_tile_index(0, 0, TileFocusDirection::Next), None);
        assert_eq!(
            adjacent_tile_index(0, 1, TileFocusDirection::Previous),
            None
        );
        assert_eq!(adjacent_tile_index(2, 2, TileFocusDirection::Next), None);
    }
}

#[cfg(test)]
mod transfer_tests {
    use super::*;

    #[test]
    fn floating_to_floating_keeps_placement() {
        assert_eq!(
            transfer_action(LayoutMode::Floating, LayoutMode::Floating,),
            TransferAction::KeepPlacement
        );
    }

    #[test]
    fn floating_to_tiled_retiles() {
        assert_eq!(
            transfer_action(LayoutMode::Floating, LayoutMode::Tiled,),
            TransferAction::Retile
        );
    }

    #[test]
    fn tiled_to_floating_restores_geometry() {
        assert_eq!(
            transfer_action(LayoutMode::Tiled, LayoutMode::Floating,),
            TransferAction::RestoreFloating
        );
    }

    #[test]
    fn tiled_to_tiled_retiles() {
        assert_eq!(
            transfer_action(LayoutMode::Tiled, LayoutMode::Tiled,),
            TransferAction::Retile
        );
    }
}

#[cfg(test)]
mod tile_swap_tests {
    use super::{TileFocusDirection, adjacent_swap_pair};

    #[test]
    fn swap_finds_next_tile() {
        let indices = [0, 3, 7];

        assert_eq!(
            adjacent_swap_pair(&indices, 3, TileFocusDirection::Next),
            Some((3, 7))
        );
    }

    #[test]
    fn swap_wraps_around() {
        let indices = [0, 3, 7];

        assert_eq!(
            adjacent_swap_pair(&indices, 7, TileFocusDirection::Next),
            Some((7, 0))
        );

        assert_eq!(
            adjacent_swap_pair(&indices, 0, TileFocusDirection::Previous),
            Some((0, 7))
        );
    }

    #[test]
    fn swap_rejects_invalid_selection() {
        assert_eq!(adjacent_swap_pair(&[], 0, TileFocusDirection::Next), None);

        assert_eq!(adjacent_swap_pair(&[2], 2, TileFocusDirection::Next), None);

        assert_eq!(
            adjacent_swap_pair(&[0, 3, 7], 5, TileFocusDirection::Previous),
            None
        );
    }
}

#[cfg(test)]
mod master_promotion_tests {
    use super::master_swap_pair;

    #[test]
    fn stack_tile_can_be_promoted() {
        let indices = [0, 3, 7, 10];

        assert_eq!(master_swap_pair(&indices, 7), Some((7, 0)));
    }

    #[test]
    fn current_master_cannot_be_promoted_again() {
        let indices = [0, 3, 7];

        assert_eq!(master_swap_pair(&indices, 0), None);
    }

    #[test]
    fn invalid_or_missing_tiles_are_rejected() {
        assert_eq!(master_swap_pair(&[], 0), None);

        assert_eq!(master_swap_pair(&[2, 5, 8], 3), None);

        assert_eq!(master_swap_pair(&[2, 5, 8], 8), Some((8, 2)));
    }
}
