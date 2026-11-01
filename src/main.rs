mod appearance;
mod config;
mod keybindings;
mod layout;
mod window;
mod workspace;

use appearance::border_style;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic};
use keybindings::WorkspaceCommand;
use layout::{SnapRect, focus_border_outside_tile};

use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::desktop::{
    LayerSurface as DesktopLayerSurface, WindowSurfaceType, layer_map_for_output,
};
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::shell::wlr_layer::{
    KeyboardInteractivity, Layer, LayerSurface, LayerSurfaceCachedState, WlrLayerShellHandler,
    WlrLayerShellState,
};
use smithay::wayland::socket::ListeningSocketSource;
use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent, ButtonState, InputEvent, KeyState, KeyboardKeyEvent,
            MouseButton, PointerButtonEvent,
        },
        renderer::{
            element::{
                Kind, render_elements,
                solid::{SolidColorBuffer, SolidColorRenderElement},
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
                utils::CropRenderElement,
            },
            gles::GlesRenderer,
            utils::on_commit_buffer_handler,
        },
        winit::{self, WinitEvent},
    },
    delegate_compositor, delegate_data_device, delegate_seat, delegate_shm, delegate_xdg_shell,
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::FilterResult,
        pointer::{ButtonEvent, MotionEvent},
    },
    reexports::wayland_server::{Display, protocol::wl_seat},
    utils::{Logical, Physical, Point, Rectangle, Serial, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
            TraversalAction, with_states, with_surface_tree_downward,
        },
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            },
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
};
use smithay::{delegate_layer_shell, delegate_output};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::time::Duration;
use std::{os::unix::io::OwnedFd, sync::Arc};
use wayland_protocols::xdg::shell::server::xdg_toplevel;
use wayland_server::protocol::wl_output::WlOutput;
use wayland_server::{
    Client,
    backend::{ClientData, ClientId, DisconnectReason},
    protocol::{
        wl_buffer,
        wl_surface::{self, WlSurface},
    },
};
use window::WindowManager;

const ANWILITE_SOCKET_NAME: &str = "octlitch-0";

/// Only top/overlay layer clients should automatically take desktop focus.
/// OnDemand is allowed here for application launchers opened by a shortcut;
/// None is never eligible. Background/Bottom may still receive pointer input.
fn layer_can_auto_focus(kind: Layer, mode: KeyboardInteractivity) -> bool {
    matches!(kind, Layer::Top | Layer::Overlay)
        && matches!(
            mode,
            KeyboardInteractivity::Exclusive | KeyboardInteractivity::OnDemand
        )
}

struct LayerHit {
    root: WlSurface,
    pointer_target: (WlSurface, Point<f64, Logical>),
}

render_elements! {
    AnwiliteRenderElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    ClippedSurface=CropRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>,
    FocusBorder=SolidColorRenderElement,
}

impl BufferHandler for App {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl XdgShellHandler for App {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Add the window to the current workspace.
        self.windows.add(surface.clone());
        self.needs_redraw = true;

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Activated);
        });

        surface.send_configure();
        let workspace = self.windows.current_workspace();

        if self
            .windows
            .reflow_tiled_workspace(workspace, self.output_size)
        {
            self.needs_redraw = true;
        }

        // Do not steal keyboard focus from a visible layer-shell launcher.
        if self.active_layer_keyboard.is_none()
            && let Some(keyboard) = self.seat.get_keyboard()
        {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, Some(surface.wl_surface().clone()), serial);
        }
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if self.windows.maximize(&surface, self.output_size) {
            self.needs_redraw = true;
        } else {
            // Still answer the protocol request if policy currently prevents it.
            surface.send_configure();
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if self.windows.restore(&surface) {
            self.needs_redraw = true;
        } else {
            surface.send_configure();
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if self.windows.minimize_window(&surface, self.output_size) {
            if let Some(keyboard) = self.seat.get_keyboard() {
                let current_focus = keyboard.current_focus();

                let valid = current_focus
                    .as_ref()
                    .is_some_and(|focused| self.windows.focus_is_valid(focused));
                if !valid {
                    let replacement = self.windows.focused_surface();

                    keyboard.set_focus(
                        self,
                        replacement,
                        smithay::utils::SERIAL_COUNTER.next_serial(),
                    );
                }
            }
            self.needs_redraw = true;
        }
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {
        // Handle popup creation here
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // Handle popup grab here
    }

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
        // Handle popup reposition here
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        println!("Octlitch: received XDG window move request");

        // Only handle requests for our input seat.
        if !self.seat.owns(&seat) {
            return;
        }

        let valid_press = self
            .move_press
            .as_ref()
            .is_some_and(|(press_serial, press_surface)| {
                *press_serial == serial && *press_surface == surface
            });

        if !valid_press || self.windows.is_dragging() {
            return;
        }

        let valid_grab = self
            .seat
            .get_pointer()
            .is_some_and(|pointer| pointer.has_grab(serial));

        if !valid_grab {
            return;
        }

        let (x, y) = self.pointer_position;

        self.xdg_drag_active = self.windows.start_drag_for(&surface, x, y);
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let valid_seat = self.seat.owns(&seat);

        let valid_press = self
            .move_press
            .as_ref()
            .is_some_and(|(press_serial, press_surface)| {
                *press_serial == serial && *press_surface == surface
            });

        let valid_grab = self
            .seat
            .get_pointer()
            .is_some_and(|pointer| pointer.has_grab(serial));

        if !valid_seat || !valid_press || !valid_grab {
            return;
        }

        let (x, y) = self.pointer_position;

        if self.windows.start_resize_for(&surface, x, y, edges) {
            println!("Octlitch: started {:?} resize", edges);
        }
    }
}

impl OutputHandler for App {}

impl WlrLayerShellHandler for App {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: LayerSurface,
        _output: Option<WlOutput>,
        layer: Layer,
        namespace: String,
    ) {
        println!("Anwilite: new layer: {namespace}, type={layer:?}");

        let desktop_layer = DesktopLayerSurface::new(surface, namespace);

        let result = layer_map_for_output(&self.output).map_layer(&desktop_layer);

        if let Err(error) = result {
            eprintln!("Anwilite: cannot map layer: {error}");
            return;
        }

        self.layer_surfaces.push(desktop_layer);
        self.needs_redraw = true;
    }

    fn layer_destroyed(&mut self, surface: LayerSurface) {
        self.initial_layer_configures.remove(surface.wl_surface());

        if let Some(index) = self
            .layer_surfaces
            .iter()
            .position(|layer| layer.layer_surface() == &surface)
        {
            let layer = self.layer_surfaces.remove(index);

            layer_map_for_output(&self.output).unmap_layer(&layer);
        }

        if self.active_layer_keyboard.as_ref() == Some(surface.wl_surface()) {
            if let Some(next) = self.preferred_keyboard_layer() {
                // Keep the original XDG focus saved across multiple layers.
                self.focus_layer(next);
            } else {
                self.restore_xdg_keyboard_focus();
            }
        }
        // Unmapping a panel removes its exclusive zone immediately.
        self.refresh_work_area();
        self.needs_redraw = true;
    }
}

impl SelectionHandler for App {
    type SelectionUserData = ();
}

impl DataDeviceHandler for App {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for App {}
impl ServerDndGrabHandler for App {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {}
}

impl CompositorHandler for App {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);

        // Layer-shell clients first commit without a buffer.
        // Only then do we send their initial configure.
        let mut committed_interactive_layer = false;
        let mut layer_committed = false;
        if let Some(layer) = self
            .layer_surfaces
            .iter()
            .find(|layer| layer.wl_surface() == surface)
        {
            layer_committed = true;
            layer_map_for_output(&self.output).arrange();

            if self.initial_layer_configures.insert(surface.clone()) {
                // First bufferless commit: configure, but don't steal focus yet.
                let serial = layer.layer_surface().send_configure();
                println!("Anwilite: initial layer configure: {serial:?}");
            } else {
                // A subsequent commit can present the configured layer.
                committed_interactive_layer =
                    layer_can_auto_focus(layer.layer(), Self::layer_keyboard_mode(layer));
            }
        }

        if layer_committed {
            self.refresh_work_area();
        }

        if committed_interactive_layer && self.preferred_keyboard_layer().as_ref() == Some(surface)
        {
            self.focus_layer(surface.clone());
        }

        // Preserve all existing XDG window behavior.
        self.windows.sync_resize_position(surface);
        self.windows.sync_mode_position(surface);
        self.windows
            .trace_geometry_on_commit(surface, self.output_size);

        self.windows.reflow_for_surface(surface, self.output_size);

        self.surface_commit_count += 1;
        self.needs_redraw = true;
    }
}

impl ShmHandler for App {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl SeatHandler for App {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, focused: Option<&WlSurface>) {
        self.windows.remember_focus(focused);
        self.needs_redraw = true;
    }
    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }
}

struct App {
    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    shm_state: ShmState,
    seat_state: SeatState<Self>,
    data_device_state: DataDeviceState,

    seat: Seat<Self>,
    windows: WindowManager,
    pointer_position: (f64, f64),
    move_press: Option<(Serial, ToplevelSurface)>,
    xdg_drag_active: bool,
    needs_redraw: bool,
    output_size: (i32, i32),
    surface_commit_count: u64,

    output: Output,
    layer_shell_state: WlrLayerShellState,
    layer_surfaces: Vec<DesktopLayerSurface>,
    initial_layer_configures: HashSet<WlSurface>,
    active_layer_keyboard: Option<WlSurface>,
    saved_xdg_keyboard_focus: Option<WlSurface>,
}

impl App {
    fn refresh_work_area(&mut self) -> bool {
        // Drop Smithay's LayerMap lock before entering WindowManager.
        // The work area is output-local, not a replacement for output_size.
        let zone = layer_map_for_output(&self.output).non_exclusive_zone();
        let area = SnapRect {
            x: zone.loc.x,
            y: zone.loc.y,
            width: zone.size.w,
            height: zone.size.h,
        };

        let changed = self.windows.set_work_area(area, self.output_size);
        if changed {
            self.needs_redraw = true;
        }
        changed
    }

    fn layer_keyboard_mode(layer: &DesktopLayerSurface) -> KeyboardInteractivity {
        with_states(layer.wl_surface(), |states| {
            states
                .cached_state
                .get::<LayerSurfaceCachedState>()
                .current()
                .keyboard_interactivity
        })
    }

    fn preferred_keyboard_layer(&self) -> Option<WlSurface> {
        let map = layer_map_for_output(&self.output);
        for kind in [Layer::Overlay, Layer::Top] {
            for layer in map.layers_on(kind).rev() {
                if layer.layer_surface().alive()
                    && layer_can_auto_focus(layer.layer(), Self::layer_keyboard_mode(layer))
                {
                    return Some(layer.wl_surface().clone());
                }
            }
        }
        None
    }

    fn interactive_layer_alive(&self, surface: &WlSurface) -> bool {
        self.layer_surfaces.iter().any(|layer| {
            layer.wl_surface() == surface
                && layer.layer_surface().alive()
                && layer.can_receive_keyboard_focus()
        })
    }

    fn focused_layer_is_exclusive(&self) -> bool {
        self.active_layer_keyboard.as_ref().is_some_and(|surface| {
            self.layer_surfaces.iter().any(|layer| {
                layer.wl_surface() == surface
                    && Self::layer_keyboard_mode(layer) == KeyboardInteractivity::Exclusive
            })
        })
    }

    fn focus_layer(&mut self, surface: WlSurface) {
        self.windows.end_window_cycle();
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        if self.active_layer_keyboard.as_ref() == Some(&surface)
            && keyboard.current_focus().as_ref() == Some(&surface)
        {
            return;
        }
        // An exclusive foreground layer must not lose focus to an unrelated
        // on-demand layer when its surface commits.
        if self.active_layer_keyboard.as_ref() != Some(&surface)
            && self.focused_layer_is_exclusive()
            && self.preferred_keyboard_layer().as_ref() != Some(&surface)
        {
            return;
        }
        if self.active_layer_keyboard.is_none() {
            self.saved_xdg_keyboard_focus = keyboard
                .current_focus()
                .filter(|focused| self.windows.focus_is_valid(focused))
                .or_else(|| self.windows.focused_surface());
        }
        self.active_layer_keyboard = Some(surface.clone());
        keyboard.set_focus(
            self,
            Some(surface),
            smithay::utils::SERIAL_COUNTER.next_serial(),
        );
        self.needs_redraw = true;
    }

    fn restore_xdg_keyboard_focus(&mut self) {
        self.active_layer_keyboard = None;
        let replacement = self
            .saved_xdg_keyboard_focus
            .take()
            .filter(|surface| self.windows.focus_is_valid(surface))
            .or_else(|| self.windows.focused_surface());
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(
                self,
                replacement,
                smithay::utils::SERIAL_COUNTER.next_serial(),
            );
        }
        self.needs_redraw = true;
    }

    fn focus_xdg_from_click(&mut self, surface: WlSurface, serial: Serial) {
        self.windows.end_window_cycle();
        if self.focused_layer_is_exclusive() {
            return;
        }
        self.active_layer_keyboard = None;
        self.saved_xdg_keyboard_focus = None;
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, Some(surface), serial);
        }
    }

    /// Hit-test the rendered layer surface tree using output-local coordinates.
    /// `surface_under` honors Wayland input regions and subsurface offsets.
    fn layer_hit_at(&self, kind: Layer, x: f64, y: f64) -> Option<LayerHit> {
        let map = layer_map_for_output(&self.output);
        let layer = map.layer_under(kind, (x, y))?;
        let geometry = map.layer_geometry(layer)?;
        let relative = (x - f64::from(geometry.loc.x), y - f64::from(geometry.loc.y));
        let (surface, offset) = layer.surface_under(
            relative,
            WindowSurfaceType::TOPLEVEL | WindowSurfaceType::SUBSURFACE,
        )?;
        let absolute = (
            f64::from(geometry.loc.x + offset.x),
            f64::from(geometry.loc.y + offset.y),
        );
        Some(LayerHit {
            root: layer.wl_surface().clone(),
            pointer_target: (surface, absolute.into()),
        })
    }

    fn foreground_layer_hit_at(&self, x: f64, y: f64) -> Option<LayerHit> {
        self.layer_hit_at(Layer::Overlay, x, y)
            .or_else(|| self.layer_hit_at(Layer::Top, x, y))
    }

    fn background_layer_hit_at(&self, x: f64, y: f64) -> Option<LayerHit> {
        self.layer_hit_at(Layer::Bottom, x, y)
            .or_else(|| self.layer_hit_at(Layer::Background, x, y))
    }

    fn pointer_surface_at(&self, x: f64, y: f64) -> Option<(WlSurface, Point<f64, Logical>)> {
        if let Some(hit) = self.foreground_layer_hit_at(x, y) {
            return Some(hit.pointer_target);
        }
        self.windows.surface_at(x, y).or_else(|| {
            self.background_layer_hit_at(x, y)
                .map(|hit| hit.pointer_target)
        })
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }

    run_winit()
}

fn reload_configuration(
    current: &mut config::AnwiliteConfig,
    windows: &mut WindowManager,
    output_size: (i32, i32),
) -> bool {
    let next = match config::try_load() {
        Ok(next) => next,
        Err(error) => {
            eprintln!("Anwilite: reload failed: {error}");
            return false;
        }
    };

    // Apply tiling only after the window manager validates it.
    if next.tiling != current.tiling
        && let Err(error) = windows.apply_tiling_settings(next.tiling, output_size)
    {
        eprintln!("Anwilite: reload rejected: {error}; keeping previous settings");
        return false;
    }

    match current.apply_reload(Ok(next)) {
        Ok(true) => {
            println!("Anwilite: configuration reloaded");
            true
        }
        Ok(false) => {
            println!("Anwilite: configuration unchanged");
            false
        }
        Err(error) => {
            eprintln!("Anwilite: reload failed: {error}");
            false
        }
    }
}

fn launch_configured_app(label: &str, argv: &[String], children: &mut Vec<std::process::Child>) {
    let Some((program, args)) = argv.split_first() else {
        eprintln!("Anwilite: no {label} command configured");
        return;
    };

    let result = std::process::Command::new(program)
        .args(args)
        .env("WAYLAND_DISPLAY", ANWILITE_SOCKET_NAME)
        .spawn();

    match result {
        Ok(child) => {
            println!("Anwilite: launched {label}: {program} (PID {})", child.id());

            children.push(child);
        }

        Err(error) => {
            eprintln!("Anwilite: could not launch {label} ({program}): {error}");
        }
    }
}

pub fn run_winit() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = config::load();
    let mut last_config_snapshot = config::snapshot();

    assert!(
        config.active_border.is_valid(),
        "Invalid active border configuration"
    );

    if let Some(style) = config.inactive_border {
        assert!(style.is_valid(), "Invalid inactive border configuration");
    }

    let mut display: Display<App> = Display::new()?;
    let dh = display.handle();
    let output = Output::new(
        "ANWILITE-1".into(),
        PhysicalProperties {
            size: (300, 200).into(),
            subpixel: Subpixel::Unknown,
            make: "Octlitch".into(),
            model: "Anwilite Nested Output".into(),
        },
    );

    let _output_global = output.create_global::<App>(&dh);
    // Waybar needs XDG logical-output information in addition to wl_output.
    let _output_manager_state = OutputManagerState::new_with_xdg_output::<App>(&dh);

    let layer_shell_state = WlrLayerShellState::new::<App>(&dh);

    let compositor_state = CompositorState::new::<App>(&dh);
    let shm_state = ShmState::new::<App>(&dh, vec![]);
    let mut seat_state = SeatState::new();
    let seat = seat_state.new_wl_seat(&dh, "winit");

    let mut state = {
        App {
            compositor_state,
            xdg_shell_state: XdgShellState::new::<App>(&dh),
            shm_state,
            seat_state,
            data_device_state: DataDeviceState::new::<App>(&dh),
            seat,
            windows: WindowManager::default(),
            pointer_position: (0.0, 0.0),
            move_press: None,
            xdg_drag_active: false,
            needs_redraw: true,
            output_size: (1, 1), // Updated immediately after Winit initializes.
            surface_commit_count: 0,
            output,
            layer_shell_state,
            layer_surfaces: Vec::new(),
            initial_layer_configures: HashSet::new(),
            active_layer_keyboard: None,
            saved_xdg_keyboard_focus: None,
        }
    };
    state.windows.configure_startup(config.tiling);
    let mut event_loop = EventLoop::<Vec<UnixStream>>::try_new()?;
    const CONFIG_CHECK_INTERVAL: Duration = Duration::from_millis(500);

    let reload_due = Rc::new(Cell::new(false));
    let reload_signal = Rc::clone(&reload_due);

    event_loop.handle().insert_source(
        Timer::from_duration(CONFIG_CHECK_INTERVAL),
        move |_, _, _| {
            reload_signal.set(true);

            TimeoutAction::ToDuration(CONFIG_CHECK_INTERVAL)
        },
    )?;

    let socket = ListeningSocketSource::with_name(ANWILITE_SOCKET_NAME)?;

    event_loop
        .handle()
        .insert_source(socket, |stream, _, pending_clients| {
            // Called when a new client connects.
            pending_clients.push(stream);
        })?;

    // Wake Calloop when an existing Wayland client sends a request.
    // We duplicate the display's poll fd so Calloop owns its registered fd.
    let wayland_poll_fd = display.as_fd().try_clone_to_owned()?;
    event_loop.handle().insert_source(
        Generic::new(wayland_poll_fd, Interest::READ, Mode::Level),
        |_, _, _pending_clients| Ok(PostAction::Continue),
    )?;

    let mut pending_clients = Vec::new();
    let mut clients = Vec::new();

    let (mut backend, winit) = winit::init::<GlesRenderer>()?;
    let initial_size = backend.window_size();
    state.output_size = (initial_size.w, initial_size.h);

    let mode = OutputMode {
        size: initial_size,
        refresh: 60_000,
    };

    state.output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );

    state.output.set_preferred(mode);

    println!(
        "Anwilite: registered virtual output ANWILITE-1 ({}x{})",
        initial_size.w, initial_size.h
    );

    let mut tracked_size = backend.window_size();
    let mut damage_tracker = OutputDamageTracker::new(tracked_size, 1.0, Transform::Flipped180);
    let winit_events = Rc::new(RefCell::new(Vec::new()));
    let event_queue = Rc::clone(&winit_events);
    event_loop
        .handle()
        .insert_source(winit, move |event, _, _| {
            event_queue.borrow_mut().push(event);
        })?;

    let start_time = std::time::Instant::now();
    let mut frame_count: u64 = 0;
    let mut frame_timer = std::time::Instant::now();

    // Lesson 22C: diagnostic counters (reset every reporting interval).
    let mut age_samples: u64 = 0;
    let mut zero_age_samples: u64 = 0;
    let mut age_over_one_samples: u64 = 0;
    let mut winit_redraw_count: u64 = 0;
    let mut skipped_no_damage: u64 = 0;
    let mut damage_rectangles: u64 = 0;
    let mut damage_area: u128 = 0;
    let mut presented_area: u128 = 0;

    let mut border_buffers: HashMap<WlSurface, [SolidColorBuffer; 4]> = HashMap::new();

    let keyboard = state
        .seat
        .add_keyboard(Default::default(), 200, 200)
        .unwrap();
    let pointer = state.seat.add_pointer();
    let mut launched_children = Vec::<std::process::Child>::new();

    if config.apps.autostart_terminal {
        launch_configured_app("terminal", &config.apps.terminal, &mut launched_children);
    }

    loop {
        let wait = if state.needs_redraw {
            Some(Duration::ZERO)
        } else {
            state.windows.pending_resize_wait()
        };
        event_loop.dispatch(wait, &mut pending_clients)?;
        // Check whether applications launched by Anwilite have exited.
        launched_children.retain_mut(|child| match child.try_wait() {
            Ok(None) => true,

            Ok(Some(status)) => {
                println!("Anwilite: process {} exited with {status}", child.id());

                false
            }

            Err(error) => {
                eprintln!("Anwilite: cannot check process {}: {error}", child.id());

                false
            }
        });

        if reload_due.replace(false)
            && config::snapshot_changed(&mut last_config_snapshot, config::snapshot())
        {
            let output_size = state.output_size;

            if reload_configuration(&mut config, &mut state.windows, output_size) {
                state.needs_redraw = true;
            }
        }

        let output_size = backend.window_size();
        let output_dims = (output_size.w, output_size.h);
        if output_dims != state.output_size && output_dims.0 > 0 && output_dims.1 > 0 {
            // The nested host window acts as Anwilite's single virtual output.
            // Only update advertised modes when its dimensions actually change.
            let previous_mode = state.output.current_mode();
            let new_mode = OutputMode {
                size: output_size,
                refresh: 60_000,
            };

            state
                .output
                .change_current_state(Some(new_mode), None, None, None);
            state.output.set_preferred(new_mode);

            // wl_output cannot retract modes from already-connected clients,
            // but newly connected clients should not see resize history.
            if let Some(previous) = previous_mode
                && previous != new_mode
            {
                state.output.delete_mode(previous);
            }

            state.output_size = output_dims;

            // Refresh layer geometry against the new virtual output size.
            // LayerMap normally sends a pending configure when dimensions change.
            // Explicitly resend it here as a nested-output resize compatibility
            // safeguard for existing Waybar/Swaybg layer surfaces.
            let layer_layout_changed = layer_map_for_output(&state.output).arrange();

            // LayerMap owns the exclusive zones. Update window placement only
            // after arranging the layers for the *new* output dimensions.
            let work_area_changed = state.refresh_work_area();
            if !work_area_changed {
                // Output size can change even if the usable area is unchanged.
                state.windows.resize_placed_windows(output_dims);
            }

            let mut configured_layers = 0;
            for layer in &state.layer_surfaces {
                let shell = layer.layer_surface();
                if shell.alive() && state.initial_layer_configures.contains(layer.wl_surface()) {
                    shell.send_configure();
                    configured_layers += 1;
                }
            }
            println!(
                "Anwilite: output resized to {}x{}; layer layout changed={}; layers reconfigured={}",
                output_dims.0, output_dims.1, layer_layout_changed, configured_layers
            );
            state.needs_redraw = true;
        }
        let events = std::mem::take(&mut *winit_events.borrow_mut());
        for event in events {
            match event {
                WinitEvent::Resized { .. } => {
                    state.needs_redraw = true;
                }
                WinitEvent::Focus(focused) => {
                    if !focused {
                        state.windows.end_window_cycle();
                    }
                    let held_keys = keyboard.pressed_keys();

                    println!(
                        "Anwilite host focus: focused={}, held_keys={:?}, modifiers_before={:?}",
                        focused,
                        held_keys,
                        keyboard.modifier_state(),
                    );

                    for keycode in held_keys {
                        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                        let time = start_time.elapsed().as_millis() as u32;

                        keyboard.input::<(), _>(
                            &mut state,
                            keycode,
                            KeyState::Released,
                            serial,
                            time,
                            |_, _, _| FilterResult::Forward,
                        );
                    }
                    println!(
                        "Anwilite host focus: modifiers_after={:?}",
                        keyboard.modifier_state(),
                    );
                }
                WinitEvent::Input(event) => match event {
                    InputEvent::Keyboard { event } => {
                        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                        let time = start_time.elapsed().as_millis() as u32;
                        if event.state() == KeyState::Pressed {
                            let focused = keyboard.current_focus();

                            println!(
                                "Anwilite keyboard: key={:?}, focus={}, valid={}, modifiers={:?}",
                                event.key_code(),
                                focused.is_some(),
                                focused.as_ref().is_some_and(|surface| {
                                    state.windows.focus_is_valid(surface)
                                }),
                                keyboard.modifier_state(),
                            );
                        }
                        let layer_has_keyboard = state.active_layer_keyboard.is_some();
                        let requested_command = keyboard.input::<WorkspaceCommand, _>(
                            &mut state,
                            event.key_code(),
                            event.state(),
                            serial,
                            time,
                            |_, modifiers, handle| {
                                let Some(symbol) = handle.raw_syms().first().map(|sym| sym.raw())
                                else {
                                    return FilterResult::Forward;
                                };

                                let action = config.keybindings.match_event(modifiers, symbol);
                                // Let Fuzzel type, navigate, and handle Escape. Keep
                                // the reserved recovery reload key available.
                                match action {
                                    Some(WorkspaceCommand::ReloadConfig) => {
                                        FilterResult::Intercept(WorkspaceCommand::ReloadConfig)
                                    }
                                    _ if layer_has_keyboard => FilterResult::Forward,
                                    Some(command) => FilterResult::Intercept(command),
                                    None => FilterResult::Forward,
                                }
                            },
                        );

                        // Releasing the initiating modifiers ends an Alt+Tab
                        // session without changing the settled MRU history.
                        let modifiers = keyboard.modifier_state();
                        if !modifiers.alt && !modifiers.logo {
                            state.windows.end_window_cycle();
                        }

                        // Switch on key press, not on key release.

                        if event.state() == KeyState::Pressed
                            && let Some(command) = requested_command
                        {
                            // Avoid switching while a mouse press is active.
                            if state.move_press.is_none() {
                                if !matches!(command, WorkspaceCommand::CycleWindow(_)) {
                                    state.windows.end_window_cycle();
                                }
                                let changed = match command {
                                    WorkspaceCommand::Minimize => {
                                        let focused = keyboard.current_focus();

                                        focused
                                            .as_ref()
                                            .and_then(|surface| {
                                                state.windows.toplevel_for_surface(surface)
                                            })
                                            .is_some_and(|surface| {
                                                state
                                                    .windows
                                                    .minimize_window(&surface, state.output_size)
                                            })
                                    }

                                    WorkspaceCommand::RestoreMinimized => {
                                        if let Some(surface) =
                                            state.windows.restore_last_minimized(state.output_size)
                                        {
                                            // Immediately focus the restored window.
                                            keyboard.set_focus(
                                                &mut state,
                                                Some(surface),
                                                smithay::utils::SERIAL_COUNTER.next_serial(),
                                            );

                                            true
                                        } else {
                                            false
                                        }
                                    }

                                    WorkspaceCommand::ToggleTiling => {
                                        state.windows.toggle_tiling(state.output_size)
                                    }

                                    WorkspaceCommand::AdjustMasterRatio(delta) => {
                                        state.windows.adjust_master_ratio(delta, state.output_size)
                                    }

                                    WorkspaceCommand::ResetMasterRatio => {
                                        state.windows.reset_master_ratio(state.output_size)
                                    }

                                    WorkspaceCommand::FocusTile(direction) => {
                                        let focused = keyboard.current_focus();
                                        if let Some(next) = state
                                            .windows
                                            .adjacent_tiled_surface(focused.as_ref(), direction)
                                        {
                                            // Focus only: do not raise or reorder any tiles.
                                            keyboard.set_focus(
                                                &mut state,
                                                Some(next),
                                                smithay::utils::SERIAL_COUNTER.next_serial(),
                                            );
                                            true
                                        } else {
                                            false
                                        }
                                    }
                                    WorkspaceCommand::FocusSpatial(direction) => {
                                        let focused = keyboard.current_focus();
                                        if let Some(next) = state
                                            .windows
                                            .spatial_surface(focused.as_ref(), direction)
                                        {
                                            state.windows.raise_focused_floating(&next);
                                            keyboard.set_focus(
                                                &mut state,
                                                Some(next),
                                                smithay::utils::SERIAL_COUNTER.next_serial(),
                                            );
                                            true
                                        } else {
                                            false
                                        }
                                    }

                                    WorkspaceCommand::CycleWindow(direction) => {
                                        let focused = keyboard.current_focus();
                                        state
                                            .windows
                                            .cycle_global_window(focused.as_ref(), direction)
                                            .is_some()
                                    }

                                    WorkspaceCommand::CloseWindow => {
                                        if let Some(surface) =
                                            keyboard.current_focus().as_ref().and_then(|focused| {
                                                state.windows.toplevel_for_surface(focused)
                                            })
                                        {
                                            // XDG close is a request, not a forced disconnect.
                                            surface.send_close();
                                        }
                                        false
                                    }

                                    WorkspaceCommand::SwapTile(direction) => {
                                        let focused = keyboard.current_focus();
                                        let output_size = state.output_size;

                                        focused.as_ref().is_some_and(|surface| {
                                            state.windows.swap_focused_tile(
                                                surface,
                                                direction,
                                                output_size,
                                            )
                                        })
                                    }

                                    WorkspaceCommand::PromoteTile => {
                                        let focused = keyboard.current_focus();
                                        let output_size = state.output_size;

                                        focused.as_ref().is_some_and(|surface| {
                                            state.windows.promote_focused_tile(surface, output_size)
                                        })
                                    }

                                    WorkspaceCommand::SwitchTo(number) => {
                                        state.windows.switch_to_workspace(number)
                                    }

                                    WorkspaceCommand::MoveAndFollow(number) => {
                                        let focused = keyboard.current_focus();
                                        let output_size = state.output_size;

                                        focused.as_ref().is_some_and(|surface| {
                                            state.windows.move_focused_to_workspace(
                                                surface,
                                                number,
                                                output_size,
                                            )
                                        })
                                    }

                                    WorkspaceCommand::Snap(side) => {
                                        let focused = keyboard.current_focus();

                                        focused.as_ref().is_some_and(|surface| {
                                            state.windows.snap(surface, side, state.output_size)
                                        })
                                    }

                                    WorkspaceCommand::Maximize => {
                                        let focused = keyboard.current_focus();

                                        focused
                                            .as_ref()
                                            .and_then(|surface| {
                                                state.windows.toplevel_for_surface(surface)
                                            })
                                            .is_some_and(|surface| {
                                                state.windows.maximize(&surface, state.output_size)
                                            })
                                    }

                                    WorkspaceCommand::Restore => {
                                        let focused = keyboard.current_focus();

                                        focused
                                            .as_ref()
                                            .and_then(|surface| {
                                                state.windows.toplevel_for_surface(surface)
                                            })
                                            .is_some_and(|surface| state.windows.restore(&surface))
                                    }

                                    WorkspaceCommand::ReloadConfig => {
                                        last_config_snapshot = config::snapshot();
                                        reload_configuration(
                                            &mut config,
                                            &mut state.windows,
                                            state.output_size,
                                        )
                                    }

                                    WorkspaceCommand::LaunchTerminal => {
                                        launch_configured_app(
                                            "terminal",
                                            &config.apps.terminal,
                                            &mut launched_children,
                                        );
                                        false
                                    }

                                    WorkspaceCommand::LaunchFileManager => {
                                        launch_configured_app(
                                            "file manager",
                                            &config.apps.file_manager,
                                            &mut launched_children,
                                        );
                                        false
                                    }
                                    WorkspaceCommand::LaunchMenu => {
                                        launch_configured_app(
                                            "application menu",
                                            &config.apps.menu,
                                            &mut launched_children,
                                        );

                                        false
                                    }

                                    WorkspaceCommand::ToggleShowDesktop => {
                                        state.windows.toggle_show_desktop()
                                    }
                                };

                                if changed {
                                    if matches!(
                                        command,
                                        WorkspaceCommand::SwitchTo(_)
                                            | WorkspaceCommand::MoveAndFollow(_)
                                            | WorkspaceCommand::ToggleTiling
                                            | WorkspaceCommand::Minimize
                                            | WorkspaceCommand::ToggleShowDesktop
                                            | WorkspaceCommand::CycleWindow(_)
                                    ) {
                                        let focus = state.windows.focused_surface();

                                        keyboard.set_focus(
                                            &mut state,
                                            focus.clone(),
                                            smithay::utils::SERIAL_COUNTER.next_serial(),
                                        );

                                        let actual_focus = keyboard.current_focus();

                                        println!(
                                            "Octlitch focus: workspace={} requested={} actual={} matched={}",
                                            state.windows.current_workspace(),
                                            focus.is_some(),
                                            actual_focus.is_some(),
                                            actual_focus == focus
                                        );

                                        // Recalculate pointer focus for visible windows.
                                        let (x, y) = state.pointer_position;
                                        let pointer_focus = state.pointer_surface_at(x, y);

                                        pointer.motion(
                                            &mut state,
                                            pointer_focus,
                                            &MotionEvent {
                                                location: (x, y).into(),
                                                serial: smithay::utils::SERIAL_COUNTER
                                                    .next_serial(),
                                                time,
                                            },
                                        );

                                        pointer.frame(&mut state);
                                    }
                                    // Force the output to update.
                                    state.needs_redraw = true;

                                    println!(
                                        "Octlitch: {:?}, active workspace {}",
                                        command,
                                        state.windows.current_workspace()
                                    );
                                }
                            }
                        }
                    }

                    InputEvent::PointerMotionAbsolute { event } => {
                        let x = event.x_transformed(output_size.w);
                        let y = event.y_transformed(output_size.h);

                        state.pointer_position = (x, y);

                        if state.windows.is_resizing() {
                            state.windows.update_resize(x, y);
                            state.needs_redraw = true;
                        } else if state.windows.is_dragging() {
                            state.windows.update_drag(x, y);
                            state.needs_redraw = true;
                        } else {
                            // Match the actual rendered stacking order, including layers.
                            let focus = state.pointer_surface_at(x, y);

                            pointer.motion(
                                &mut state,
                                focus,
                                &MotionEvent {
                                    location: (x, y).into(),
                                    serial: smithay::utils::SERIAL_COUNTER.next_serial(),
                                    time: start_time.elapsed().as_millis() as u32,
                                },
                            );

                            pointer.frame(&mut state);
                        }
                    }

                    InputEvent::PointerButton { event } => {
                        let (x, y) = state.pointer_position;

                        let serial = smithay::utils::SERIAL_COUNTER.next_serial();

                        let time = start_time.elapsed().as_millis() as u32;

                        let foreground_hit = state.foreground_layer_hit_at(x, y);
                        let ordinary_window_hit = if foreground_hit.is_none() {
                            state.windows.surface_at(x, y).is_some()
                        } else {
                            false
                        };
                        let background_hit = if foreground_hit.is_none() && !ordinary_window_hit {
                            state.background_layer_hit_at(x, y)
                        } else {
                            None
                        };

                        if event.button() == Some(MouseButton::Left)
                            && event.state() == ButtonState::Pressed
                        {
                            state.move_press = if foreground_hit.is_none() {
                                state
                                    .windows
                                    .toplevel_at(x, y)
                                    .map(|surface| (serial, surface))
                            } else {
                                None
                            };
                        }

                        if event.button() == Some(MouseButton::Left)
                            && event.state() == ButtonState::Released
                        {
                            state.move_press = None;

                            if state.xdg_drag_active {
                                state.windows.end_drag();
                                state.xdg_drag_active = false;
                            }
                            if state.windows.is_resizing() {
                                state.windows.finish_resize();
                                println!("Octlitch: finished resize");
                            }
                        }
                        if event.state() == ButtonState::Pressed {
                            if let Some(hit) = foreground_hit.as_ref().or(background_hit.as_ref()) {
                                if state.interactive_layer_alive(&hit.root) {
                                    state.focus_layer(hit.root.clone());
                                }
                            } else if ordinary_window_hit
                                && let Some(surface) = state.windows.raise_at(x, y)
                            {
                                state.focus_xdg_from_click(surface, serial);
                            }

                            state.needs_redraw = true;
                        }

                        // Octlitch's window-dragging behavior.
                        if event.button() == Some(MouseButton::Middle)
                            && foreground_hit.is_none()
                            && ordinary_window_hit
                        {
                            match event.state() {
                                ButtonState::Pressed => {
                                    state.windows.start_drag(x, y);
                                }

                                ButtonState::Released => {
                                    state.windows.end_drag();

                                    let focus = state.pointer_surface_at(x, y);

                                    pointer.motion(
                                        &mut state,
                                        focus,
                                        &MotionEvent {
                                            location: (x, y).into(),
                                            serial,
                                            time,
                                        },
                                    );

                                    pointer.frame(&mut state);
                                }
                            }
                        } else {
                            let focus = state.pointer_surface_at(x, y);

                            pointer.motion(
                                &mut state,
                                focus,
                                &MotionEvent {
                                    location: (x, y).into(),
                                    serial,
                                    time,
                                },
                            );

                            //
                            pointer.button(
                                &mut state,
                                &ButtonEvent {
                                    serial,
                                    time,
                                    button: event.button_code(),
                                    state: event.state(),
                                },
                            );

                            pointer.frame(&mut state);
                        }
                    }

                    _ => {}
                },

                WinitEvent::Redraw => {
                    winit_redraw_count += 1;
                    state.needs_redraw = true;
                }
                WinitEvent::CloseRequested => return Ok(()),
            }
        }

        state.windows.flush_resize();

        // 1. Register newly connected Wayland clients.
        for stream in pending_clients.drain(..) {
            println!("Anwilite: new Wayland client connected");

            let client = display
                .handle()
                .insert_client(stream, Arc::new(ClientState::default()))?;

            clients.push(client);
        }

        // 2. Process pending client requests FIRST.

        display.dispatch_clients(&mut state)?;

        let previous_count = state.windows.windows.len();

        state.windows.retain_live();
        border_buffers.retain(|surface, _| {
            state
                .windows
                .windows
                .iter()
                .any(|window| window.surface.wl_surface() == surface)
        });

        if state.windows.windows.len() != previous_count {
            println!(
                "Anwilite: window count changed: {} -> {}",
                previous_count,
                state.windows.windows.len()
            );

            if state.windows.reflow_all_tiled_workspaces(state.output_size) {
                state.needs_redraw = true;
            }
            state.needs_redraw = true;
        }

        // The old XDG-only recovery loop would steal focus from Fuzzel on
        // every iteration. Accept a live interactive layer as valid focus.
        if state
            .active_layer_keyboard
            .as_ref()
            .is_some_and(|surface| !state.interactive_layer_alive(surface))
        {
            state.restore_xdg_keyboard_focus();
        }

        let current_focus = keyboard.current_focus();
        let focus_valid = current_focus.as_ref().is_some_and(|surface| {
            state.windows.focus_is_valid(surface)
                || (state.active_layer_keyboard.as_ref() == Some(surface)
                    && state.interactive_layer_alive(surface))
        });
        if !focus_valid {
            let replacement = state
                .active_layer_keyboard
                .clone()
                .filter(|surface| state.interactive_layer_alive(surface))
                .or_else(|| state.windows.focused_surface());

            if current_focus != replacement {
                println!(
                    "Andwilite focus recovery: previous={}, replacement={}",
                    current_focus.is_some(),
                    replacement.is_some(),
                );

                keyboard.set_focus(
                    &mut state,
                    replacement,
                    smithay::utils::SERIAL_COUNTER.next_serial(),
                );
                state.needs_redraw = true;
            }
        }

        display.flush_clients()?;

        if state.needs_redraw {
            state.needs_redraw = false;

            let size = backend.window_size();

            if size != tracked_size {
                tracked_size = size;
                damage_tracker = OutputDamageTracker::new(size, 1.0, Transform::Flipped180);
            }

            {
                let _ = backend.bind().unwrap();
            }

            let age = backend.buffer_age().unwrap_or(0);
            age_samples += 1;
            if age == 0 {
                zero_age_samples += 1;
            } else if age > 1 {
                age_over_one_samples += 1;
            }

            let submit_damage = {
                let (renderer, mut framebuffer) = backend.bind().unwrap();

                // Keyboard focus determines each window's border style.
                let focused = keyboard.current_focus();
                let focused_rect = state.windows.focused_window_rect(focused.as_ref());

                let mut elements = Vec::<AnwiliteRenderElement>::new();

                // Foreground layer-shell surfaces are in front of normal windows.
                // The render-elements list uses front-to-back stacking order.
                {
                    let layer_map = layer_map_for_output(&state.output);
                    for kind in [Layer::Overlay, Layer::Top] {
                        for layer in layer_map.layers_on(kind).rev() {
                            let Some(geometry) = layer_map.layer_geometry(layer) else {
                                continue;
                            };

                            elements.extend(
                                render_elements_from_surface_tree(
                                    renderer,
                                    layer.wl_surface(),
                                    (geometry.loc.x, geometry.loc.y),
                                    1.0,
                                    1.0,
                                    Kind::Unspecified,
                                )
                                .into_iter()
                                .map(AnwiliteRenderElement::Surface),
                            );
                        }
                    }
                }

                // Window order is front-to-back.
                for window in state
                    .windows
                    .windows
                    .iter()
                    .filter(|window| state.windows.is_window_visible(window))
                {
                    let is_focused = focused.as_ref() == Some(window.surface.wl_surface());

                    // Use the same trusted geometry path as Lesson 29C.
                    let rect = if is_focused {
                        focused_rect
                    } else {
                        window.focus_rect()
                    };

                    // Draw this window's border, if enabled.
                    if let (Some(style), Some(rect)) = (
                        border_style(is_focused, config.active_border, config.inactive_border),
                        rect,
                    ) && let Some(edges) =
                        focus_border_outside_tile(rect, style.thickness, style.clearance)
                    {
                        let buffers = border_buffers
                            .entry(window.surface.wl_surface().clone())
                            .or_insert_with(|| {
                                std::array::from_fn(|_| SolidColorBuffer::new((1, 1), style.rgba))
                            });

                        for (buffer, edge) in buffers.iter_mut().zip(edges) {
                            buffer.update((edge.width, edge.height), style.rgba);

                            let border = SolidColorRenderElement::from_buffer(
                                buffer,
                                (edge.x, edge.y),
                                1.0,
                                1.0,
                                Kind::Unspecified,
                            );

                            elements.push(AnwiliteRenderElement::FocusBorder(border));
                        }
                    }

                    // Preserve the existing tiled/snapped client clipping.
                    let clip = window.placed_rect_for_render().map(|tile| {
                        Rectangle::<i32, Physical>::new(
                            (tile.x, tile.y).into(),
                            (tile.width, tile.height).into(),
                        )
                    });

                    // Draw the application's surfaces after its border.
                    elements.extend(
                        render_elements_from_surface_tree(
                            renderer,
                            window.surface.wl_surface(),
                            (window.x, window.y),
                            1.0,
                            1.0,
                            Kind::Unspecified,
                        )
                        .into_iter()
                        .filter_map(move |element| match clip {
                            Some(rect) => CropRenderElement::from_element(element, 1.0, rect)
                                .map(AnwiliteRenderElement::ClippedSurface),
                            None => Some(AnwiliteRenderElement::Surface(element)),
                        }),
                    );
                }

                // Bottom/background surfaces are behind ordinary windows.
                {
                    let layer_map = layer_map_for_output(&state.output);
                    for kind in [Layer::Bottom, Layer::Background] {
                        for layer in layer_map.layers_on(kind).rev() {
                            let Some(geometry) = layer_map.layer_geometry(layer) else {
                                continue;
                            };

                            elements.extend(
                                render_elements_from_surface_tree(
                                    renderer,
                                    layer.wl_surface(),
                                    (geometry.loc.x, geometry.loc.y),
                                    1.0,
                                    1.0,
                                    Kind::Unspecified,
                                )
                                .into_iter()
                                .map(AnwiliteRenderElement::Surface),
                            );
                        }
                    }
                }

                let result = damage_tracker.render_output(
                    renderer,
                    &mut framebuffer,
                    age,
                    &elements,
                    [0.1, 0.0, 0.0, 1.0],
                )?;

                result.damage.cloned()
            };

            // Submit only when the backbuffer actually changed.
            if let Some(regions) = submit_damage {
                // Sum of rectangle areas, not unique changed pixels:
                // overlapping rectangles may count the same pixel twice.
                let pixels: u128 = regions
                    .iter()
                    .map(|rect| rect.size.w.max(0) as u128 * rect.size.h.max(0) as u128)
                    .sum();
                damage_area += pixels;
                presented_area += size.w.max(0) as u128 * size.h.max(0) as u128;
                damage_rectangles += regions.len() as u64;

                backend.submit(Some(&regions))?;
                frame_count += 1;
            } else {
                skipped_no_damage += 1;
            }

            for window in state
                .windows
                .windows
                .iter()
                .filter(|window| state.windows.is_window_visible(window))
            {
                send_frames_surface_tree(
                    window.surface.wl_surface(),
                    start_time.elapsed().as_millis() as u32,
                );
            }

            // Layer-shell clients need frame callbacks to advance rendering.
            for layer in &state.layer_surfaces {
                send_frames_surface_tree(
                    layer.wl_surface(),
                    start_time.elapsed().as_millis() as u32,
                );
            }

            display.flush_clients()?;
        }

        let elapsed = frame_timer.elapsed();
        if elapsed >= std::time::Duration::from_secs(5) {
            let fps = frame_count as f64 / elapsed.as_secs_f64();
            let zero_age_pct = if age_samples == 0 {
                0.0
            } else {
                100.0 * zero_age_samples as f64 / age_samples as f64
            };
            let damage_pct = if presented_area == 0 {
                0.0
            } else {
                100.0 * damage_area as f64 / presented_area as f64
            };

            println!(
                "Octlitch: {:.1} submits/s | buffer age 0: {}/{} ({:.1}%) | \
                 submitted rectangle-area: {:.1}% of output | rects: {} | no-damage: {}",
                fps,
                zero_age_samples,
                age_samples,
                zero_age_pct,
                damage_pct,
                damage_rectangles,
                skipped_no_damage,
            );

            println!(
                "Octlitch redraw sources: surface commits={} | Winit redraw={} | buffer age >1={}/{}",
                state.surface_commit_count, winit_redraw_count, age_over_one_samples, age_samples,
            );

            state.surface_commit_count = 0;
            winit_redraw_count = 0;
            age_over_one_samples = 0;
            frame_count = 0;
            age_samples = 0;
            zero_age_samples = 0;
            skipped_no_damage = 0;
            damage_rectangles = 0;
            damage_area = 0;
            presented_area = 0;
            frame_timer = std::time::Instant::now();
        }
    }
}

pub fn send_frames_surface_tree(surface: &wl_surface::WlSurface, time: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_surf, states, &()| {
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time);
            }
        },
        |_, _, &()| true,
    );
}

#[derive(Default)]
struct ClientState {
    compositor_state: CompositorClientState,
}
impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {
        println!("initialized");
    }

    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {
        println!("disconnected");
    }
}

// Macros used to delegate protocol handling to types in the app state.
delegate_xdg_shell!(App);
delegate_compositor!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_data_device!(App);
delegate_output!(App);
delegate_layer_shell!(App);

#[cfg(test)]
mod layer_input_policy_tests {
    use super::*;

    #[test]
    fn foreground_layers_honor_keyboard_interactivity() {
        for kind in [Layer::Overlay, Layer::Top] {
            assert!(layer_can_auto_focus(kind, KeyboardInteractivity::Exclusive));
            assert!(layer_can_auto_focus(kind, KeyboardInteractivity::OnDemand));
            assert!(!layer_can_auto_focus(kind, KeyboardInteractivity::None));
        }
    }

    #[test]
    fn background_layers_never_auto_steal_focus() {
        for kind in [Layer::Background, Layer::Bottom] {
            for mode in [
                KeyboardInteractivity::None,
                KeyboardInteractivity::OnDemand,
                KeyboardInteractivity::Exclusive,
            ] {
                assert!(!layer_can_auto_focus(kind, mode));
            }
        }
    }
}
