use crate::layout::{DEFAULT_SNAP_GAPS, SnapRect, SnapSide, snap_rect_with_gaps};
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

#[derive(Default)]
pub struct WindowManager {
    pub windows: Vec<ManagedWindow>,
    workspaces: Workspaces,

    last_focused: [Option<WlSurface>; WORKSPACE_COUNT],

    dragging: Option<DragState>,
    resizing: Option<ResizeState>,
    settling: Option<SettlingResize>,
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
            },
        );
    }

    pub fn current_workspace(&self) -> usize {
        self.workspaces.current_number()
    }

    pub fn switch_to_workspace(&mut self, number: usize) -> bool {
        if self.is_dragging() || self.is_resizing() {
            return false;
        }
        self.workspaces.switch_to(number)
    }

    pub fn move_focused_to_workspace(&mut self, focused: &WlSurface, number: usize) -> bool {
        // Don't move a window during an interactive drag or resize.
        if self.is_dragging() || self.is_resizing() {
            return false;
        }

        let current = self.current_workspace();

        // Validate the destination.
        if !(1..=WORKSPACE_COUNT).contains(&number) || number == current {
            return false;
        }

        // Find the window that actually has keyboard focus.
        let Some(index) = self.windows.iter().position(|window| {
            window.workspace == current
                && window.surface.alive()
                && window.surface.wl_surface() == focused
        }) else {
            return false;
        };

        // Switch to the destination workspace.
        if !self.workspaces.switch_to(number) {
            return false;
        }

        // Move the window and place it at the front.
        let mut window = self.windows.remove(index);
        window.workspace = number;
        self.windows.insert(0, window);

        if self.last_focused[current - 1].as_ref() == Some(focused) {
            self.last_focused[current - 1] = None;
        }

        self.last_focused[number - 1] = Some(focused.clone());

        true
    }

    pub fn focused_surface(&self) -> Option<WlSurface> {
        let current = self.current_workspace();

        if let Some(saved) = &self.last_focused[current - 1] {
            let still_valid = self.windows.iter().any(|window| {
                window.workspace == current
                    && window.surface.alive()
                    && window.surface.wl_surface() == saved
            });

            if still_valid {
                return Some(saved.clone());
            }
        }

        // Fallback: choose the frontmost living window.
        self.windows
            .iter()
            .find(|window| window.workspace == current && window.surface.alive())
            .map(|window| window.surface.wl_surface().clone())
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
        let current = self.current_workspace();

        self.windows
            .iter()
            .find(|window| window.workspace == current && Self::hit_window(window, px, py))
            .map(|window| window.surface.clone())
    }

    pub fn surface_at(&self, px: f64, py: f64) -> Option<(WlSurface, Point<f64, Logical>)> {
        let current = self.current_workspace();

        self.windows.iter().find_map(|window| {
            if window.workspace != current {
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
        let current = self.current_workspace();

        self.windows
            .iter()
            .find(|window| {
                window.workspace == current
                    && window.surface.alive()
                    && window.surface.wl_surface() == focused
            })
            .map(|window| window.surface.clone())
    }

    pub fn raise_at(&mut self, px: f64, py: f64) -> Option<WlSurface> {
        let index = self.windows.iter().position(|window| {
            window.workspace == self.current_workspace() && Self::hit_window(window, px, py)
        })?;
        let window = self.windows.remove(index);
        let surface = window.surface.wl_surface().clone();
        self.windows.insert(0, window);
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

        window.restoring = None;
        window.maximized = false;
        window.snapped = Some((side, target));

        // Position the visible window geometry.
        window.x = target.x - geometry.loc.x;
        window.y = target.y - geometry.loc.y;
        if window.pending_unsnap.is_some() {
            return false;
        }

        // Ask the Wayland client to resize its content.
        window.surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Maximized);
            state.size = Some((target.width, target.height).into());
        });

        window.surface.send_configure();

        true
    }

    /// Respond to xdg_toplevel.set_maximized for a single nested output.
    pub fn maximize(&mut self, surface: &ToplevelSurface, output_size: (i32, i32)) -> bool {
        if self.dragging.is_some() || self.resizing.is_some() {
            return false;
        }

        if self
            .settling
            .as_ref()
            .is_some_and(|s| &s.anchor.surface == surface)
        {
            self.settling = None;
        }
        let Some(window) = self.windows.iter_mut().find(|w| &w.surface == surface) else {
            return false;
        };
        let Some(geometry) = window_geometry(window) else {
            return false;
        };

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
        if window.pending_unsnap.is_some() {
            return false;
        }

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Maximized);
            state.size = Some(output_size.into());
        });
        // A maximize request must receive a configure even if already maximized.
        surface.send_configure();
        true
    }

    /// Restore both the normal geometry and the XDG maximized state.
    pub fn restore(&mut self, surface: &ToplevelSurface) -> bool {
        let Some(window) = self.windows.iter_mut().find(|w| &w.surface == surface) else {
            return false;
        };
        if !window.maximized && window.snapped.is_none() {
            return false;
        }

        let saved = window.saved_normal.take();
        if let Some(normal) = saved {
            if let Some(geometry) = window_geometry(window) {
                window.x = normal.left - geometry.loc.x;
                window.y = normal.top - geometry.loc.y;
            }
            // Keep the visual origin stable until the restored size arrives.
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

    /// Reconfigure maximized clients when the nested Winit output changes size.

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

        if window.maximized || window.pending_unsnap.is_some() {
            return false;
        }

        if window.snapped.is_some() {
            let Some(geometry) = window_geometry(window) else {
                return false;
            };

            // Retrieve the size saved before the window was snapped.
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
        if window.maximized || window.snapped.is_some() {
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
