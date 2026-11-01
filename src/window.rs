use crate::layout::{SnapRect, SnapSide, TilingSettings, snap_rect_in_area, tile_rects_in_area};
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

impl ManagedWindow {
    // A placement clip is used for both tiled and snapped windows.
    // Floating windows have no placement clip.
    pub fn placed_rect_for_render(&self) -> Option<SnapRect> {
        if self.maximized {
            return None;
        }

        let tile = self.tiled.or_else(|| self.snapped.map(|(_, rect)| rect))?;
        // A client may still be between its initial configure and first
        // committed buffer. Keep tile cropping active in that case.
        let Some(geometry) = window_geometry(self) else {
            return Some(tile);
        };

        // Translate the committed XDG geometry into output coordinates.
        let absolute = absolute_window_rect((self.x, self.y), geometry)?;
        let visible = Rectangle::<i32, Logical>::new(
            (absolute.x, absolute.y).into(),
            (absolute.width, absolute.height).into(),
        );

        let slot = Rectangle::<i32, Logical>::new(
            (tile.x, tile.y).into(),
            (tile.width, tile.height).into(),
        );

        // One authoritative rectangle drives BOTH GPU cropping and borders.
        // Never draw into another window's tile while a resize is pending.
        let Some(clipped) = visible.intersection(slot) else {
            return Some(tile);
        };
        if clipped.size.w <= 0 || clipped.size.h <= 0 {
            return None;
        }

        Some(SnapRect {
            x: clipped.loc.x,
            y: clipped.loc.y,
            width: clipped.size.w,
            height: clipped.size.h,
        })
    }

    pub fn focus_rect(&self) -> Option<SnapRect> {
        // Keep the existing no-border policy for maximized windows.
        if self.maximized || self.minimized {
            return None;
        }

        // Tiled/snapped focus borders must match the clip used to render
        // their client surfaces, even during configure/commit transitions.
        if self.tiled.is_some() || self.snapped.is_some() {
            return self.placed_rect_for_render();
        }

        // Floating windows are not clipped to a tiling slot. Their outline
        // follows the actual XDG window geometry as they move or resize.
        let geometry = window_geometry(self)?;
        absolute_window_rect((self.x, self.y), geometry)
    }
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

// The compositor must not request a tile size outside the client's
// advertised xdg_toplevel bounds. A zero constraint means unspecified.
fn tile_fits_size_constraints(tile: SnapRect, minimum: (i32, i32), maximum: (i32, i32)) -> bool {
    let (min_w, min_h) = minimum;
    let (max_w, max_h) = maximum;
    tile.width > 0
        && tile.height > 0
        && tile.width >= min_w
        && tile.height >= min_h
        && (max_w == 0 || tile.width <= max_w)
        && (max_h == 0 || tile.height <= max_h)
}

fn window_can_fit_tile(window: &ManagedWindow, tile: SnapRect) -> bool {
    let (minimum, maximum) = with_states(window.surface.wl_surface(), |states| {
        let mut cached = states.cached_state.get::<SurfaceCachedState>();
        let state = cached.current();
        (
            (state.min_size.w, state.min_size.h),
            (state.max_size.w, state.max_size.h),
        )
    });
    tile_fits_size_constraints(tile, minimum, maximum)
}

// Convert surface-local XDG geometry into compositor output coordinates.
// Pure geometry helper: no Wayland state is needed to unit-test it.
fn absolute_window_rect(origin: (i32, i32), geometry: Rectangle<i32, Logical>) -> Option<SnapRect> {
    if geometry.size.w <= 0 || geometry.size.h <= 0 {
        return None;
    }

    Some(SnapRect {
        x: origin.0.checked_add(geometry.loc.x)?,
        y: origin.1.checked_add(geometry.loc.y)?,
        width: geometry.size.w,
        height: geometry.size.h,
    })
}

// XDG geometry may begin inside the wl_surface (for example, to exclude shadows).
// Keep the *XDG window geometry* aligned with the work-area origin, not the raw
// wl_surface origin. This is independent of whether the client chooses to draw
// a titlebar when it receives xdg_toplevel::State::Maximized.
fn origin_for_maximized_geometry(
    work_origin: (i32, i32),
    geometry: Rectangle<i32, Logical>,
) -> (i32, i32) {
    (
        work_origin.0 - geometry.loc.x,
        work_origin.1 - geometry.loc.y,
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

// Update the xdg_toplevel tiled-edge hints. The caller must send a configure
// after changing these pending states.
fn set_tiled_edges(surface: &ToplevelSurface, tiled: bool) {
    surface.with_pending_state(|state| {
        for edge in [
            xdg_toplevel::State::TiledLeft,
            xdg_toplevel::State::TiledRight,
            xdg_toplevel::State::TiledTop,
            xdg_toplevel::State::TiledBottom,
        ] {
            if tiled {
                state.states.set(edge);
            } else {
                state.states.unset(edge);
            }
        }
    });
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpatialDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowCycleDirection {
    Previous,
    Next,
}

struct WindowCycle {
    order: Vec<WlSurface>,
    index: usize,
}

// Use doubled centres to avoid rounding errors and negative-coordinate bugs.
fn centre(rect: SnapRect) -> (i64, i64) {
    (
        i64::from(rect.x) * 2 + i64::from(rect.width),
        i64::from(rect.y) * 2 + i64::from(rect.height),
    )
}

fn projected_overlap(a_start: i32, a_length: i32, b_start: i32, b_length: i32) -> bool {
    let a_end = i64::from(a_start) + i64::from(a_length);
    let b_end = i64::from(b_start) + i64::from(b_length);
    i64::from(a_start) < b_end && i64::from(b_start) < a_end
}

fn spatial_neighbor(
    source: SnapRect,
    candidates: &[(usize, SnapRect)],
    direction: SpatialDirection,
) -> Option<usize> {
    let (sx, sy) = centre(source);
    candidates
        .iter()
        .filter_map(|&(index, rect)| {
            if rect.width <= 0 || rect.height <= 0 {
                return None;
            }
            let (cx, cy) = centre(rect);
            let (primary, lateral, aligned) = match direction {
                SpatialDirection::Left => (
                    sx - cx,
                    (sy - cy).abs(),
                    projected_overlap(source.y, source.height, rect.y, rect.height),
                ),
                SpatialDirection::Right => (
                    cx - sx,
                    (sy - cy).abs(),
                    projected_overlap(source.y, source.height, rect.y, rect.height),
                ),
                SpatialDirection::Up => (
                    sy - cy,
                    (sx - cx).abs(),
                    projected_overlap(source.x, source.width, rect.x, rect.width),
                ),
                SpatialDirection::Down => (
                    cy - sy,
                    (sx - cx).abs(),
                    projected_overlap(source.x, source.width, rect.x, rect.width),
                ),
            };
            (primary > 0).then_some(((!aligned, primary, lateral, index), index))
        })
        .min_by_key(|(score, _)| *score)
        .map(|(_, index)| index)
}

fn next_cycle_index(index: usize, count: usize, direction: WindowCycleDirection) -> Option<usize> {
    if count < 2 || index >= count {
        return None;
    }
    Some(match direction {
        WindowCycleDirection::Next => (index + 1) % count,
        WindowCycleDirection::Previous => (index + count - 1) % count,
    })
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
    recent_focus: Vec<WlSurface>,
    window_cycle: Option<WindowCycle>,

    dragging: Option<DragState>,
    resizing: Option<ResizeState>,
    settling: Option<SettlingResize>,
    layout_modes: [LayoutMode; WORKSPACE_COUNT],
    minimize_counter: u64,
    master_ratios: [Option<u8>; WORKSPACE_COUNT],
    tiling_settings: TilingSettings,
    show_desktop: [bool; WORKSPACE_COUNT],

    work_area: Option<SnapRect>,
}

impl WindowManager {
    fn current_work_area(&self, output_size: (i32, i32)) -> SnapRect {
        self.work_area.unwrap_or(SnapRect {
            x: 0,
            y: 0,
            width: output_size.0,
            height: output_size.1,
        })
    }

    pub fn set_work_area(&mut self, area: SnapRect, output_size: (i32, i32)) -> bool {
        let valid = area.width > 0
            && area.height > 0
            && area.x >= 0
            && area.y >= 0
            && area
                .x
                .checked_add(area.width)
                .is_some_and(|x| x <= output_size.0)
            && area
                .y
                .checked_add(area.height)
                .is_some_and(|y| y <= output_size.1);

        if !valid {
            eprintln!("Anwilite: ignoring invalid work area: {area:?}");
            return false;
        }

        if self.current_work_area(output_size) == area {
            return false;
        }

        self.work_area = Some(area);

        // Resize maximized and snapped windows and update tiled layouts.
        self.resize_placed_windows(output_size);

        println!(
            "Anwilite: work area updated: ({}, {}) {}x{}",
            area.x, area.y, area.width, area.height
        );

        true
    }

    pub fn configure_startup(&mut self, settings: TilingSettings) {
        assert!(settings.is_valid());

        self.tiling_settings = settings;

        // Original behavior: every workspace starts in the global mode.
        let default_mode = if settings.start_tiled {
            LayoutMode::Tiled
        } else {
            LayoutMode::Floating
        };

        self.layout_modes = [default_mode; WORKSPACE_COUNT];

        // An explicit workspace list overrides the global startup mode.
        if let Some(tiled_workspaces) = settings.tiled_workspaces {
            for (mode, tiled) in self.layout_modes.iter_mut().zip(tiled_workspaces) {
                *mode = if tiled {
                    LayoutMode::Tiled
                } else {
                    LayoutMode::Floating
                };
            }
        }
    }

    pub fn apply_tiling_settings(
        &mut self,
        next: TilingSettings,
        output_size: (i32, i32),
    ) -> Result<bool, String> {
        if !next.is_valid() {
            return Err("invalid tiling settings".into());
        }

        if self.tiling_settings == next {
            return Ok(false);
        }

        let gaps_changed = self.tiling_settings.gaps != next.gaps;
        let geometry_changed =
            gaps_changed || self.tiling_settings.default_master_ratio != next.default_master_ratio;

        // Don't change window geometry during an interactive operation.
        if geometry_changed && (self.is_dragging() || self.is_resizing() || self.settling.is_some())
        {
            return Err("window operation in progress; reload again afterward".into());
        }

        if geometry_changed {
            // Validate every tiled workspace before changing any settings.
            for workspace in 1..=WORKSPACE_COUNT {
                if self.layout_modes[workspace - 1] != LayoutMode::Tiled {
                    continue;
                }

                let participants: Vec<&ManagedWindow> = self
                    .windows
                    .iter()
                    .filter(|window| {
                        window.workspace == workspace && window.surface.alive() && !window.minimized
                    })
                    .collect();

                // Avoid applying settings during incomplete window transitions.
                if participants.iter().any(|window| {
                    window.tiled.is_none()
                        || window.pending_unsnap.is_some()
                        || window.restoring.is_some()
                        || window_geometry(window).is_none()
                }) {
                    return Err(format!(
                        "workspace {workspace} has windows still configuring"
                    ));
                }

                // Preserve per-workspace master-ratio overrides.
                let ratio = self.master_ratios[workspace - 1].unwrap_or(next.default_master_ratio);

                let targets = tile_rects_in_area(
                    self.current_work_area(output_size),
                    participants.len(),
                    next.gaps,
                    ratio,
                )
                .ok_or_else(|| format!("workspace {workspace} cannot fit the new layout"))?;

                if participants
                    .iter()
                    .zip(targets.iter())
                    .any(|(window, &target)| !window_can_fit_tile(window, target))
                {
                    return Err(format!(
                        "workspace {workspace}: a window cannot fit its proposed tile"
                    ));
                }
            }

            // Snapped windows must also fit the new gaps.
            if gaps_changed {
                for window in self.windows.iter().filter(|window| window.surface.alive()) {
                    let Some((side, _)) = window.snapped else {
                        continue;
                    };

                    let target =
                        snap_rect_in_area(self.current_work_area(output_size), side, next.gaps)
                            .ok_or("new snapping geometry is invalid")?;

                    if !window_can_fit_tile(window, target) || window_geometry(window).is_none() {
                        return Err("a snapped window cannot fit the new gaps".into());
                    }
                }
            }
        }

        // Validation succeeded. Now we can commit the new settings.
        self.tiling_settings = next;

        if gaps_changed {
            let area = self.current_work_area(output_size);
            for window in &mut self.windows {
                if !window.surface.alive() {
                    continue;
                }

                let Some((side, old_rect)) = window.snapped else {
                    continue;
                };

                let Some(target) = snap_rect_in_area(area, side, next.gaps) else {
                    continue;
                };

                if target == old_rect {
                    continue;
                }

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

        if geometry_changed {
            self.reflow_all_tiled_workspaces(output_size);
        }

        Ok(true)
    }

    pub fn add(&mut self, surface: ToplevelSurface) {
        let offset = (self.windows.len() % 8) as i32 * 48;

        // Remember which workspace is active.
        let workspace = self.workspaces.current_number();
        self.show_desktop[workspace - 1] = false;

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

    pub fn toggle_show_desktop(&mut self) -> bool {
        let index = self.current_workspace() - 1;

        self.show_desktop[index] = !self.show_desktop[index];

        println!(
            "Anwilite: workspace {} show desktop: {}",
            index + 1,
            self.show_desktop[index]
        );

        true
    }

    pub fn current_layout_mode(&self) -> LayoutMode {
        self.layout_modes[self.current_workspace() - 1]
    }

    pub fn master_ratio_for_workspace(&self, workspace: usize) -> u8 {
        self.master_ratios[workspace - 1].unwrap_or(self.tiling_settings.default_master_ratio)
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
        // Calculate the proposed layout before changing the ratio.
        let Some(targets) = tile_rects_in_area(
            self.current_work_area(output_size),
            participants.len(),
            self.tiling_settings.gaps,
            percent,
        ) else {
            return false;
        };

        // Every client must fit its proposed rectangle.
        if participants
            .iter()
            .zip(targets.iter())
            .any(|(window, &target)| !window_can_fit_tile(window, target))
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
        if self.current_layout_mode() != LayoutMode::Tiled
            || self.is_dragging()
            || self.is_resizing()
            || self.settling.is_some()
        {
            return false;
        }

        let workspace = self.current_workspace();
        let index = workspace - 1;

        let Some(previous_override) = self.master_ratios[index] else {
            return false;
        };

        let default_ratio = self.tiling_settings.default_master_ratio;

        if previous_override != default_ratio && !self.set_master_ratio(default_ratio, output_size)
        {
            return false;
        }

        self.master_ratios[index] = None;

        true
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

            // The client is leaving tiled mode; restore ordinary decorations.
            set_tiled_edges(&window.surface, false);

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
                } else {
                    // Also commit cleared tile flags for hidden/minimized windows.
                    window.surface.send_pending_configure();
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
        let Some(targets) = tile_rects_in_area(
            self.current_work_area(output_size),
            indices.len(),
            self.tiling_settings.gaps,
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

        // A newly opened client or output resize may make the layout too
        // small for an existing window. Preserve all windows by returning
        // this workspace to floating mode rather than overlapping tiles.
        if indices
            .iter()
            .zip(targets.iter())
            .any(|(&index, &target)| !window_can_fit_tile(&self.windows[index], target))
        {
            eprintln!(
                "Anwilite: workspace {} cannot fit all {} tiled windows; restoring floating mode",
                workspace,
                indices.len(),
            );
            self.stop_tiling_workspace(workspace);
            return true;
        }

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

            // Ask clients to suppress CSD shadows beyond their tiled edges.
            set_tiled_edges(&window.surface, true);
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

        let Some(targets) = tile_rects_in_area(
            self.current_work_area(output_size),
            windows.len(),
            self.tiling_settings.gaps,
            self.master_ratio_for_workspace(workspace),
        ) else {
            return false;
        };

        if let Some(target) = windows
            .iter()
            .zip(targets.iter())
            .find_map(|(window, &target)| (!window_can_fit_tile(window, target)).then_some(target))
        {
            eprintln!(
                "Anwilite: cannot tile {} windows: one client rejects a {}x{} slot; keeping floating layout",
                windows.len(),
                target.width,
                target.height,
            );
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
        self.end_window_cycle();
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

        self.end_window_cycle();
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
            set_tiled_edges(&window.surface, false);
            if window.tiled.take().is_some()
                && let Some(normal) = window.pre_tile.take()
            {
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

    pub fn spatial_surface(
        &self,
        focused: Option<&WlSurface>,
        direction: SpatialDirection,
    ) -> Option<WlSurface> {
        let focused = focused?;
        let source = self.windows.iter().find(|window| {
            self.is_window_visible(window) && window.surface.wl_surface() == focused
        })?;
        let source_rect = source.focus_rect()?;
        let candidates: Vec<(usize, SnapRect)> = self
            .windows
            .iter()
            .enumerate()
            .filter(|(_, window)| {
                self.is_window_visible(window) && window.surface.wl_surface() != focused
            })
            .filter_map(|(index, window)| Some((index, window.focus_rect()?)))
            .collect();
        let index = spatial_neighbor(source_rect, &candidates, direction)?;
        Some(self.windows[index].surface.wl_surface().clone())
    }

    /// Cycle through the most recently focused windows on every workspace.
    /// The order remains fixed until the initiating modifier is released.
    pub fn cycle_global_window(
        &mut self,
        focused: Option<&WlSurface>,
        direction: WindowCycleDirection,
    ) -> Option<WlSurface> {
        if self.is_dragging() || self.is_resizing() || self.settling.is_some() {
            return None;
        }
        if self.window_cycle.is_none() {
            let mut order = Vec::new();
            for surface in &self.recent_focus {
                if self.globally_switchable(surface) && !order.contains(surface) {
                    order.push(surface.clone());
                }
            }
            for window in &self.windows {
                let surface = window.surface.wl_surface();
                if self.globally_switchable(surface) && !order.contains(surface) {
                    order.push(surface.clone());
                }
            }
            if order.len() < 2 {
                return None;
            }
            let index = focused.and_then(|surface| order.iter().position(|s| s == surface));
            // If there is no valid current focus, start at the first candidate.
            let index = match index {
                Some(index) => next_cycle_index(index, order.len(), direction)?,
                None => 0,
            };
            self.window_cycle = Some(WindowCycle { order, index });
        } else {
            let cycle = self.window_cycle.as_mut()?;
            cycle.index = next_cycle_index(cycle.index, cycle.order.len(), direction)?;
        }

        let cycle = self.window_cycle.as_ref()?;
        let surface = cycle.order[cycle.index].clone();
        let workspace = self
            .windows
            .iter()
            .find(|window| {
                window.surface.wl_surface() == &surface
                    && window.surface.alive()
                    && !window.minimized
            })?
            .workspace;
        if workspace != self.current_workspace() 
            && !self.workspaces.switch_to(workspace) {
                self.end_window_cycle();
                return None;
            }
        
        self.show_desktop[workspace - 1] = false;
        self.last_focused[workspace - 1] = Some(surface.clone());
        self.raise_focused_floating(&surface);
        Some(surface)
    }

    /// Make keyboard-focused floating windows visible without changing tile order.
    pub fn raise_focused_floating(&mut self, surface: &WlSurface) {
        if self.current_layout_mode() != LayoutMode::Floating {
            return;
        }
        if let Some(index) = self.windows.iter().position(|window| {
            self.is_window_visible(window) && window.surface.wl_surface() == surface
        }) 
            && index != 0 {
                let window = self.windows.remove(index);
                self.windows.insert(0, window);
            }
        
    }

    fn globally_switchable(&self, surface: &WlSurface) -> bool {
        let Some(window) = self.windows.iter().find(|window| {
            window.surface.wl_surface() == surface && window.surface.alive() && !window.minimized
        }) else {
            return false;
        };
        // Don't focus a tile covered by a maximized tile in its workspace.
        !self.windows.iter().any(|other| {
            other.workspace == window.workspace
                && other.surface.alive()
                && !other.minimized
                && other.tiled.is_some()
                && other.maximized
                && other.surface.wl_surface() != surface
        })
    }

    pub fn end_window_cycle(&mut self) {
        self.window_cycle = None;
    }

    pub fn focused_window_rect(&self, focused: Option<&WlSurface>) -> Option<SnapRect> {
        let focused = focused?;

        self.windows
            .iter()
            .find(|window| window.surface.wl_surface() == focused && self.is_window_visible(window))
            .and_then(|window| window.focus_rect())
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

        // Calculate the target rectangles using the configured gaps and ratio.
        let Some(targets) = tile_rects_in_area(
            self.current_work_area(output_size),
            indices.len(),
            self.tiling_settings.gaps,
            ratio,
        ) else {
            return false;
        };

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

        // Check whether every window can fit its destination AFTER swapping.
        if indices.iter().copied().zip(targets).any(|(index, rect)| {
            let destination_window = if index == first {
                second
            } else if index == second {
                first
            } else {
                index
            };

            !window_can_fit_tile(&self.windows[destination_window], rect)
        }) {
            return false;
        }

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

        if self.show_desktop[current - 1] {
            return false;
        }

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
        self.recent_focus.retain(|other| other != surface);
        self.recent_focus.insert(0, surface.clone());
    }

    pub fn retain_live(&mut self) {
        let original_count = self.windows.len();
        self.windows.retain(|window| window.surface.alive());
        if self.windows.len() != original_count {
            let live: Vec<WlSurface> = self
                .windows
                .iter()
                .map(|window| window.surface.wl_surface().clone())
                .collect();
            self.recent_focus.retain(|surface| live.contains(surface));
            self.end_window_cycle();
        }

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

        let Some(target) = snap_rect_in_area(
            self.current_work_area(output_size),
            side,
            self.tiling_settings.gaps,
        ) else {
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
        let area = self.current_work_area(output_size);

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
            // Maximized windows are not presented as ordinary tiled windows.
            set_tiled_edges(&window.surface, false);
            window.maximized = true;

            // Maximized windows cover the entire output.
            (window.x, window.y) = origin_for_maximized_geometry((area.x, area.y), geometry);

            window.surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Maximized);
                state.size = Some((area.width, area.height).into());
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

        (window.x, window.y) = origin_for_maximized_geometry((area.x, area.y), geometry);

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Maximized);
            state.size = Some((area.width, area.height).into());
        });

        surface.send_configure();

        true
    }

    pub fn restore(&mut self, surface: &ToplevelSurface) -> bool {
        let Some(window) = self.windows.iter_mut().find(|w| &w.surface == surface) else {
            return false;
        };
        if window.maximized
            && let Some(target) = window.tiled
        {
            set_tiled_edges(&window.surface, true);
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

            tile_rects_in_area(
                self.current_work_area(output_size),
                next_count,
                self.tiling_settings.gaps,
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
        let area = self.current_work_area(output_size);
        for window in &mut self.windows {
            if window.maximized {
                if let Some(geometry) = window_geometry(window) {
                    (window.x, window.y) =
                        origin_for_maximized_geometry((area.x, area.y), geometry);
                }

                window.surface.with_pending_state(|state| {
                    state.size = Some((area.width, area.height).into());
                });

                window.surface.send_pending_configure();
            } else if let Some((side, _)) = window.snapped {
                let Some(target) = snap_rect_in_area(area, side, self.tiling_settings.gaps) else {
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
            let Some(rectangles) = tile_rects_in_area(
                self.current_work_area(output_size),
                indices.len(),
                self.tiling_settings.gaps,
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
        // A committed XDG geometry update must not put maximized windows
        // back at the origin, behind an exclusive top or side panel.
        let maximized_origin = self.work_area.map_or((0, 0), |area| (area.x, area.y));
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

            if let Some(drag) = self.dragging.as_mut()
                && drag.surface == window.surface
            {
                drag.offset_x = pending.pointer_x - window.x as f64;
                drag.offset_y = pending.pointer_y - window.y as f64;
            }
            return;
        }
        if !window.maximized
            && let Some(target) = window.tiled
        {
            window.x = target.x - geometry.loc.x;
            window.y = target.y - geometry.loc.y;
            return;
        }

        if window.maximized {
            (window.x, window.y) = origin_for_maximized_geometry(maximized_origin, geometry);
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

    // Opt-in diagnostics for maximized windows whose client-side titlebar
    // appears to disappear behind an exclusive-zone panel.
    // Enable with ANWILITE_TRACE_GEOMETRY=1; inactive by default.
    pub fn trace_geometry_on_commit(&self, surface: &WlSurface, output_size: (i32, i32)) {
        if std::env::var_os("ANWILITE_TRACE_GEOMETRY").is_none() {
            return;
        }

        let Some(window) = self
            .windows
            .iter()
            .find(|w| w.surface.wl_surface() == surface)
        else {
            return;
        };

        let bbox = bbox_from_surface_tree(window.surface.wl_surface(), (0, 0));
        let xdg_geometry = with_states(window.surface.wl_surface(), |states| {
            states
                .cached_state
                .get::<SurfaceCachedState>()
                .current()
                .geometry
        });
        let resolved = window_geometry(window);
        let area = self.current_work_area(output_size);

        eprintln!(
            "Anwilite geometry trace: maximized={} snapped={} tiled={} \
             work_area={area:?} surface_origin=({}, {}) \
             surface_bbox={bbox:?} xdg_geometry={xdg_geometry:?} \
             effective_geometry={resolved:?}",
            window.maximized,
            window.snapped.is_some(),
            window.tiled.is_some(),
            window.x,
            window.y,
        );
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
mod tile_constraint_tests {
    use super::{SnapRect, tile_fits_size_constraints};

    #[test]
    fn unconstrained_client_can_be_tiled() {
        let tile = SnapRect {
            x: 0,
            y: 0,
            width: 200,
            height: 120,
        };
        assert!(tile_fits_size_constraints(tile, (0, 0), (0, 0)));
    }

    #[test]
    fn too_small_or_too_large_tiles_are_rejected() {
        let tile = SnapRect {
            x: 0,
            y: 0,
            width: 200,
            height: 120,
        };
        assert!(!tile_fits_size_constraints(tile, (201, 100), (0, 0)));
        assert!(!tile_fits_size_constraints(tile, (100, 121), (0, 0)));
        assert!(!tile_fits_size_constraints(tile, (0, 0), (199, 0)));
        assert!(tile_fits_size_constraints(tile, (100, 90), (400, 180)));
    }
}

#[cfg(test)]
mod spatial_focus_tests {
    use super::{
        SnapRect, SpatialDirection, WindowCycleDirection, next_cycle_index, spatial_neighbor,
    };

    fn rect(x: i32, y: i32, width: i32, height: i32) -> SnapRect {
        SnapRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn spatial_navigation_finds_nearest_aligned_neighbor() {
        let center = rect(100, 100, 100, 100);
        let candidates = [
            (0, rect(0, 100, 80, 100)),
            (1, rect(230, 100, 100, 100)),
            (2, rect(100, 0, 100, 80)),
            (3, rect(100, 230, 100, 80)),
            (4, rect(201, 500, 100, 100)),
        ];
        assert_eq!(
            spatial_neighbor(center, &candidates, SpatialDirection::Left),
            Some(0)
        );
        assert_eq!(
            spatial_neighbor(center, &candidates, SpatialDirection::Right),
            Some(1)
        );
        assert_eq!(
            spatial_neighbor(center, &candidates, SpatialDirection::Up),
            Some(2)
        );
        assert_eq!(
            spatial_neighbor(center, &candidates, SpatialDirection::Down),
            Some(3)
        );
    }

    #[test]
    fn spatial_navigation_rejects_wrong_side_and_handles_negative_coordinates() {
        let center = rect(-100, -100, 60, 60);
        assert_eq!(
            spatial_neighbor(
                center,
                &[(0, rect(-200, -100, 80, 60))],
                SpatialDirection::Right
            ),
            None
        );
        assert_eq!(
            spatial_neighbor(
                center,
                &[(0, rect(-200, -100, 80, 60))],
                SpatialDirection::Left
            ),
            Some(0)
        );
    }

    #[test]
    fn cycle_direction_wraps_and_rejects_single_window() {
        assert_eq!(next_cycle_index(0, 3, WindowCycleDirection::Next), Some(1));
        assert_eq!(next_cycle_index(2, 3, WindowCycleDirection::Next), Some(0));
        assert_eq!(
            next_cycle_index(0, 3, WindowCycleDirection::Previous),
            Some(2)
        );
        assert_eq!(next_cycle_index(0, 1, WindowCycleDirection::Next), None);
    }
}

#[cfg(test)]
mod focus_rect_tests {
    use super::{SnapRect, absolute_window_rect};
    use smithay::utils::{Logical, Rectangle};

    #[test]
    fn floating_rect_includes_window_and_xdg_offsets() {
        let geometry = Rectangle::<i32, Logical>::new((5, -3).into(), (180, 110).into());

        assert_eq!(
            absolute_window_rect((90, 60), geometry),
            Some(SnapRect {
                x: 95,
                y: 57,
                width: 180,
                height: 110,
            })
        );
    }

    #[test]
    fn floating_rect_rejects_invalid_dimensions_and_overflow() {
        let empty = Rectangle::<i32, Logical>::new((0, 0).into(), (0, 100).into());
        assert_eq!(absolute_window_rect((0, 0), empty), None);

        let overflowing = Rectangle::<i32, Logical>::new((5, 0).into(), (100, 100).into());
        assert_eq!(absolute_window_rect((i32::MAX, 0), overflowing), None);
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

#[cfg(test)]
mod tiling_reload_tests {
    use super::*;

    #[test]
    fn empty_window_manager_accepts_valid_tiling_settings() {
        let mut manager = WindowManager::default();

        let settings = TilingSettings {
            gaps: crate::layout::SnapGaps {
                outer: 12,
                inner: 8,
            },
            default_master_ratio: 60,
            start_tiled: false,
            ..TilingSettings::default()
        };

        assert!(
            manager
                .apply_tiling_settings(settings, (1200, 800))
                .unwrap()
        );

        assert_eq!(manager.tiling_settings, settings);

        assert!(
            !manager
                .apply_tiling_settings(settings, (1200, 800))
                .unwrap()
        );
    }
}

#[cfg(test)]
mod workspace_preference_tests {
    use super::*;

    #[test]
    fn reset_override_follows_future_config_changes() {
        let mut manager = WindowManager::default();

        let settings = TilingSettings {
            default_master_ratio: 60,
            start_tiled: true,
            ..TilingSettings::default()
        };

        manager.configure_startup(settings);

        assert!(manager.set_master_ratio(70, (1200, 800)));
        assert_eq!(manager.master_ratios[0], Some(70));

        assert!(manager.reset_master_ratio((1200, 800)));
        assert_eq!(manager.master_ratios[0], None);

        let updated = TilingSettings {
            default_master_ratio: 65,
            ..settings
        };

        assert!(manager.apply_tiling_settings(updated, (1200, 800)).unwrap());

        assert_eq!(manager.master_ratio_for_workspace(1), 65);
    }

    #[test]
    fn workspaces_keep_independent_ratio_overrides() {
        let mut manager = WindowManager::default();

        manager.configure_startup(TilingSettings {
            start_tiled: true,
            ..TilingSettings::default()
        });

        assert!(manager.set_master_ratio(70, (1200, 800)));

        assert!(manager.switch_to_workspace(2));
        assert!(manager.set_master_ratio(75, (1200, 800)));

        assert_eq!(manager.master_ratio_for_workspace(1), 70);
        assert_eq!(manager.master_ratio_for_workspace(2), 75);

        assert!(manager.reset_master_ratio((1200, 800)));

        assert_eq!(manager.master_ratio_for_workspace(1), 70);
        assert_eq!(manager.master_ratio_for_workspace(2), 50);
        assert_eq!(manager.master_ratios[1], None);
    }

    #[test]
    fn reset_clears_override_even_if_ratio_matches_default() {
        let mut manager = WindowManager::default();

        manager.configure_startup(TilingSettings {
            default_master_ratio: 60,
            start_tiled: true,
            ..TilingSettings::default()
        });

        manager.master_ratios[0] = Some(60);

        assert!(manager.reset_master_ratio((1200, 800)));
        assert_eq!(manager.master_ratios[0], None);
    }

    #[test]
    fn startup_modes_follow_explicit_workspace_list() {
        let mut manager = WindowManager::default();

        let mut tiled = [false; WORKSPACE_COUNT];
        tiled[0] = true;
        tiled[2] = true;

        manager.configure_startup(TilingSettings {
            start_tiled: true,
            tiled_workspaces: Some(tiled),
            ..TilingSettings::default()
        });

        assert_eq!(manager.current_layout_mode(), LayoutMode::Tiled);

        assert!(manager.switch_to_workspace(2));
        assert_eq!(manager.current_layout_mode(), LayoutMode::Floating);

        assert!(manager.switch_to_workspace(3));
        assert_eq!(manager.current_layout_mode(), LayoutMode::Tiled);
    }
}

#[cfg(test)]
mod show_desktop_tests {
    use super::*;

    #[test]
    fn show_desktop_toggles_independently_per_workspace() {
        let mut manager = WindowManager::default();

        assert!(!manager.show_desktop[0]);

        assert!(manager.toggle_show_desktop());
        assert!(manager.show_desktop[0]);

        assert!(manager.switch_to_workspace(2));
        assert!(!manager.show_desktop[1]);

        assert!(manager.toggle_show_desktop());
        assert!(manager.show_desktop[1]);

        assert!(manager.switch_to_workspace(1));
        assert!(manager.show_desktop[0]);

        assert!(manager.toggle_show_desktop());
        assert!(!manager.show_desktop[0]);

        // Workspace 2 keeps its own Show Desktop state.
        assert!(manager.show_desktop[1]);
    }
}

#[cfg(test)]
mod reserved_work_area_tests {
    use super::*;

    fn area(x: i32, y: i32, width: i32, height: i32) -> SnapRect {
        SnapRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn work_area_changes_only_once() {
        let mut manager = WindowManager::default();
        let reserved = area(0, 36, 1280, 764);
        assert!(manager.set_work_area(reserved, (1280, 800)));
        assert!(!manager.set_work_area(reserved, (1280, 800)));
        assert_eq!(manager.current_work_area((1280, 800)), reserved);
    }

    #[test]
    fn invalid_work_area_does_not_replace_previous_one() {
        let mut manager = WindowManager::default();
        let reserved = area(0, 36, 1280, 764);
        assert!(manager.set_work_area(reserved, (1280, 800)));
        assert!(!manager.set_work_area(area(0, 36, 1280, 800), (1280, 800)));
        assert_eq!(manager.current_work_area((1280, 800)), reserved);
    }

    #[test]
    fn top_bar_offsets_a_single_tile() {
        let reserved = area(0, 36, 1280, 764);
        let tiles = tile_rects_in_area(
            reserved,
            1,
            crate::layout::SnapGaps { outer: 0, inner: 0 },
            50,
        )
        .unwrap();
        assert_eq!(tiles, vec![reserved]);
    }

    #[test]
    fn top_bar_offsets_snapped_windows() {
        let reserved = area(0, 36, 1280, 764);
        let left = snap_rect_in_area(
            reserved,
            SnapSide::Left,
            crate::layout::SnapGaps { outer: 0, inner: 0 },
        )
        .unwrap();
        assert_eq!(left, area(0, 36, 640, 764));
    }

    #[test]
    fn left_panel_moves_tile_origin() {
        let reserved = area(64, 0, 1216, 800);
        let tiles = tile_rects_in_area(
            reserved,
            1,
            crate::layout::SnapGaps { outer: 0, inner: 0 },
            50,
        )
        .unwrap();
        assert_eq!(tiles, vec![reserved]);
    }
}

#[cfg(test)]
mod maximized_placement_tests {
    use super::*;

    #[test]
    fn window_geometry_starts_below_top_bar() {
        let geometry = Rectangle::<i32, Logical>::new((0, 0).into(), (1280, 764).into());
        assert_eq!(origin_for_maximized_geometry((0, 36), geometry), (0, 36));
    }

    #[test]
    fn xdg_geometry_offset_aligns_window_with_work_area() {
        // Geometry can exclude shadow pixels. The client decides whether a
        // titlebar is present, but the compositor must position its geometry.
        let geometry = Rectangle::<i32, Logical>::new((5, 10).into(), (1280, 764).into());
        assert_eq!(origin_for_maximized_geometry((0, 36), geometry), (-5, 26));
    }

    #[test]
    fn side_panel_offsets_window_geometry_horizontally() {
        let geometry = Rectangle::<i32, Logical>::new((3, 7).into(), (1200, 800).into());
        assert_eq!(origin_for_maximized_geometry((64, 0), geometry), (61, -7));
    }
}
