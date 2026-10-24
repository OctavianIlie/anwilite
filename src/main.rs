mod layout;
mod window;
mod workspace;

use calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic};
use layout::SnapSide;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::wayland::socket::ListeningSocketSource;
use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent, ButtonState, InputEvent, KeyState, KeyboardKeyEvent,
            MouseButton, PointerButtonEvent,
        },
        renderer::{
            element::{
                Kind,
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            },
            gles::GlesRenderer,
            utils::on_commit_buffer_handler,
        },
        winit::{self, WinitEvent},
    },
    delegate_compositor, delegate_data_device, delegate_seat, delegate_shm, delegate_xdg_shell,
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{FilterResult, keysyms},
        pointer::{ButtonEvent, MotionEvent},
    },
    reexports::wayland_server::{Display, protocol::wl_seat},
    utils::{Serial, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
            TraversalAction, with_surface_tree_downward,
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
use std::cell::RefCell;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::time::Duration;
use std::{os::unix::io::OwnedFd, sync::Arc};
use wayland_protocols::xdg::shell::server::xdg_toplevel;
use wayland_server::{
    Client,
    backend::{ClientData, ClientId, DisconnectReason},
    protocol::{
        wl_buffer,
        wl_surface::{self, WlSurface},
    },
};
use window::WindowManager;

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

        // Give the new window actual keyboard focus.
        if let Some(keyboard) = self.seat.get_keyboard() {
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
        self.windows.sync_resize_position(surface);
        self.windows.sync_mode_position(surface);

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
    }
    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }
}

#[derive(Debug, Clone, Copy)]
enum WorkspaceCommand {
    SwitchTo(usize),
    MoveAndFollow(usize),
    Snap(SnapSide),
    Maximize,
    Restore,
    ToggleTiling,
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
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }

    run_winit()
}

pub fn run_winit() -> Result<(), Box<dyn std::error::Error>> {
    let mut display: Display<App> = Display::new()?;
    let dh = display.handle();

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
        }
    };

    let mut event_loop = EventLoop::<Vec<UnixStream>>::try_new()?;

    // Register the Octlitch Wayland socket with Calloop.
    let socket = ListeningSocketSource::with_name("octlitch-0")?;

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

    let keyboard = state
        .seat
        .add_keyboard(Default::default(), 200, 200)
        .unwrap();
    let pointer = state.seat.add_pointer();
    std::process::Command::new("weston-terminal")
        .env("WAYLAND_DISPLAY", "octlitch-0")
        .spawn()
        .ok();

    loop {
        let wait = if state.needs_redraw {
            Some(Duration::ZERO)
        } else {
            state.windows.pending_resize_wait()
        };
        event_loop.dispatch(wait, &mut pending_clients)?;

        let output_size = backend.window_size();
        let output_dims = (output_size.w, output_size.h);
        if output_dims != state.output_size {
            state.output_size = output_dims;
            state.windows.resize_placed_windows(output_dims);
            state.needs_redraw = true;
        }
        let events = std::mem::take(&mut *winit_events.borrow_mut());
        for event in events {
            match event {
                WinitEvent::Resized { .. } => {
                    state.needs_redraw = true;
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
                        let requested_command = keyboard.input::<WorkspaceCommand, _>(
                            &mut state,
                            event.key_code(),
                            event.state(),
                            serial,
                            time,
                            |_, modifiers, handle| {
                                // Super+number is our normal shortcut.
                                // Ctrl+Alt+number is a nested-VM testing fallback.
                                if !modifiers.logo && !(modifiers.ctrl && modifiers.alt) {
                                    return FilterResult::Forward;
                                }

                                // Get the unmodified keyboard symbol.
                                let symbol = handle.raw_syms().first().map(|sym| sym.raw());
                                if modifiers.shift
                                    && matches!(symbol, Some(keysyms::KEY_t | keysyms::KEY_T))
                                {
                                    return FilterResult::Intercept(WorkspaceCommand::ToggleTiling);
                                }

                                if !modifiers.shift {
                                    let placement = match symbol {
                                        Some(keysyms::KEY_Left) => {
                                            Some(WorkspaceCommand::Snap(SnapSide::Left))
                                        }
                                        Some(keysyms::KEY_Right) => {
                                            Some(WorkspaceCommand::Snap(SnapSide::Right))
                                        }
                                        Some(keysyms::KEY_Up) => Some(WorkspaceCommand::Maximize),
                                        Some(keysyms::KEY_Down) => Some(WorkspaceCommand::Restore),
                                        _ => None,
                                    };

                                    if let Some(command) = placement {
                                        return FilterResult::Intercept(command);
                                    }
                                }

                                let workspace = match symbol {
                                    Some(sym)
                                        if (keysyms::KEY_1..=keysyms::KEY_9).contains(&sym) =>
                                    {
                                        Some((sym - keysyms::KEY_1 + 1) as usize)
                                    }
                                    Some(keysyms::KEY_0) => Some(10),
                                    _ => None,
                                };

                                match workspace {
                                    Some(number) if modifiers.shift => FilterResult::Intercept(
                                        WorkspaceCommand::MoveAndFollow(number),
                                    ),

                                    Some(number) => {
                                        FilterResult::Intercept(WorkspaceCommand::SwitchTo(number))
                                    }

                                    None => FilterResult::Forward,
                                }
                            },
                        );

                        // Switch on key press, not on key release.

                        if event.state() == KeyState::Pressed {
                            if let Some(command) = requested_command {
                                // Avoid switching while a mouse press is active.
                                if state.move_press.is_none() {
                                    let changed = match command {
                                        WorkspaceCommand::ToggleTiling => {
                                            state.windows.toggle_tiling(state.output_size)
                                        }

                                        WorkspaceCommand::SwitchTo(number) => {
                                            state.windows.switch_to_workspace(number)
                                        }

                                        WorkspaceCommand::MoveAndFollow(number) => {
                                            let focused = keyboard.current_focus();

                                            focused.as_ref().is_some_and(|surface| {
                                                state
                                                    .windows
                                                    .move_focused_to_workspace(surface, number)
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
                                                    state
                                                        .windows
                                                        .maximize(&surface, state.output_size)
                                                })
                                        }

                                        WorkspaceCommand::Restore => {
                                            let focused = keyboard.current_focus();

                                            focused
                                                .as_ref()
                                                .and_then(|surface| {
                                                    state.windows.toplevel_for_surface(surface)
                                                })
                                                .is_some_and(|surface| {
                                                    state.windows.restore(&surface)
                                                })
                                        }
                                    };

                                    if changed {
                                        if matches!(
                                            command,
                                            WorkspaceCommand::SwitchTo(_)
                                                | WorkspaceCommand::MoveAndFollow(_)
                                                | WorkspaceCommand::ToggleTiling
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
                                            let pointer_focus = state.windows.surface_at(x, y);

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
                            // Find the Wayland surface under the pointer.
                            let focus = state.windows.surface_at(x, y);

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

                        if event.button() == Some(MouseButton::Left)
                            && event.state() == ButtonState::Pressed
                        {
                            state.move_press = state
                                .windows
                                .toplevel_at(x, y)
                                .map(|surface| (serial, surface));
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
                            let clicked_window = state.windows.raise_at(x, y);

                            if let Some(surface) = clicked_window {
                                keyboard.set_focus(&mut state, Some(surface.clone()), serial);
                                println!(
                                    "Anwilite focus: clicked window, focus_matched={}",
                                    keyboard.current_focus().as_ref() == Some(&surface),
                                );
                            } else {
                                println!("Anwilite focus: background click, preserving focus");
                            }
                            state.needs_redraw = true;
                        }

                        // Octlitch's window-dragging behavior.
                        if event.button() == Some(MouseButton::Middle) {
                            match event.state() {
                                ButtonState::Pressed => {
                                    state.windows.start_drag(x, y);
                                }

                                ButtonState::Released => {
                                    state.windows.end_drag();

                                    let focus = state.windows.surface_at(x, y);

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
                            let focus = state.windows.surface_at(x, y);

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
                _ => (),
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

        let current_focus = keyboard.current_focus();

        let focus_valid = current_focus
            .as_ref()
            .is_some_and(|surface| state.windows.focus_is_valid(surface));

        if !focus_valid {
            let replacement = state.windows.focused_surface();

            if current_focus != replacement {
                println!(
                    "Anwilite focus recovery: previous={}, replacement={}",
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

        state.windows.retain_live();

        if state.windows.windows.len() != previous_count {
            println!(
                "Anwilite: window count changed: {} -> {}",
                previous_count,
                state.windows.windows.len()
            );

            state.windows.reflow_all_tiled_workspaces(state.output_size);

            let focus = state.windows.focused_surface();

            if keyboard.current_focus() != focus {
                keyboard.set_focus(
                    &mut state,
                    focus,
                    smithay::utils::SERIAL_COUNTER.next_serial(),
                );
            }

            state.needs_redraw = true;
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

                let elements = state
                    .windows
                    .windows
                    .iter()
                    .filter(|window| window.workspace == state.windows.current_workspace())
                    .flat_map(|window| {
                        render_elements_from_surface_tree(
                            renderer,
                            window.surface.wl_surface(),
                            (window.x, window.y),
                            1.0,
                            1.0,
                            Kind::Unspecified,
                        )
                    })
                    .collect::<Vec<WaylandSurfaceRenderElement<GlesRenderer>>>();

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
                .filter(|window| window.workspace == state.windows.current_workspace())
            {
                send_frames_surface_tree(
                    window.surface.wl_surface(),
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
