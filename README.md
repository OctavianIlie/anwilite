# Anwilite

**WORK IN PROGRESS: A custom Wayland compositor and hybrid window manager for [Octlitch Linux](https://octlitch.com).**

> **Project status: experimental, pre-alpha.** The nested compositor and core floating/window-placement workflows are working; native DRM/KMS, a complete desktop shell, XWayland, and several essential Wayland protocols are still planned. Anwilite is not ready to replace a daily-driver desktop session.

Anwilite is a Wayland compositor written in **Rust**, built on **[Smithay](https://github.com/Smithay/smithay)** (currently developed against Smithay 0.7). Its goal is to combine:

- **Hyprland-inspired flexibility:** customizable window management, gaps, keyboard-driven layouts, and eventually smooth transitions and hybrid tiling.
- **Omarchy-inspired workflow:** thoughtful defaults, a cohesive visual identity, and fast keyboard-centric navigation.
- **Windows-inspired productivity:** recognizable shortcuts for snapping, maximizing, restoring, and moving windows between desktops.
- **Gaming-first priorities:** eventual XWayland/Steam/Proton compatibility, true fullscreen, relative pointer and pointer confinement/locking, responsive input, efficient frame scheduling, and native GPU output.
- **A separate, customizable shell:** panels, widgets, launcher, and system controls implemented with **[Quickshell](https://quickshell.org/)** and QML rather than embedded into the compositor's window-management code.

**Anwilite** is the compositor/window manager. **Octlitch Linux** is the operating-system distribution it is being developed for. These are separate projects and should remain independently maintainable.

## At a glance

| Area | Current state |
| --- | --- |
| Nested Wayland compositor on Fedora/KDE | ✅ Working |
| Smithay XDG toplevel windows | ✅ Working |
| Keyboard and mouse input | ✅ Working |
| Click-to-focus, raise, window dragging | ✅ Working |
| Eight-direction interactive resizing | ✅ Working and tested |
| Maximize and restore with saved geometry | ✅ Working and tested |
| Ten workspaces | ✅ Working and tested |
| Move focused window between workspaces | ✅ Working and tested |
| Per-workspace keyboard focus history | ✅ Implemented and tested in current workflow |
| Windows-style half-screen snapping | ✅ Working and tested |
| Configurable inner and outer snap gaps | ✅ Working (compile-time constants) |
| Drag a snapped window into floating mode | ✅ Working and tested |
| Resizing the nested output | ✅ Updates maximized and snapped windows |
| Event-driven redraws and damage tracking | ✅ Working in nested backend |
| Automatic multi-window tiling | 🚧 Next major feature |
| Native DRM/KMS + libinput session | 📋 Planned |
| Proper fullscreen and gaming input protocols | 📋 Planned |
| XWayland | 📋 Planned |
| Layer shell, Quickshell integration, compositor IPC | 📋 Planned |
| Multi-monitor/output management | 📋 Planned |



## Current user experience

Anwilite currently runs **nested inside a host Wayland desktop** via Smithay's Winit/OpenGL ES renderer. The development environment has been **Fedora 44 in VirtualBox with KDE Plasma Wayland**. You can open Weston Terminal clients inside the Anwilite output, move and resize them, switch workspaces, and snap them with keyboard shortcuts.

### Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `Super + 1` … `Super + 9` | Switch to workspaces 1–9 |
| `Super + 0` | Switch to workspace 10 |
| `Super + Shift + 1` … `Super + Shift + 9` | Move the focused window to a workspace and follow it |
| `Super + Shift + 0` | Move the focused window to workspace 10 and follow it |
| `Super + Left` | Snap the focused window to the left half |
| `Super + Right` | Snap the focused window to the right half |
| `Super + Up` | Maximize the focused window |
| `Super + Down` | Restore the focused window's original floating size and position |
| Left-click | Raise and focus the window under the pointer |
| Drag a window titlebar | Move a floating window; restore a snapped window to floating mode |
| Middle-button drag | Move a floating window, or unsnap and move a snapped window |
| Window resize handles | Resize floating windows from all eight edges/corners |

**Nested-development fallback:** KDE or the host compositor may intercept some `Super` combinations. During development, the compositor also recognizes `Ctrl + Alt` in place of `Super` for the workspace and arrow-key shortcuts; use `Ctrl + Alt + Shift + number` to move a window. These are development bindings, not a final keymap or configurable shortcut system.

**Snapping details:** The normal floating geometry is saved before placement. A window can transition from floating → left snap → right snap → maximized → restored without losing its original geometry. Snapped windows stay aligned when the nested output is resized. Dragging a snapped titlebar requests the original floating size, waits for the client to commit it, then reanchors the cursor relative to the restored geometry to avoid a visible jump.

### Ten workspaces

- Each newly created managed window belongs to the currently active workspace.
- Workspace switching changes which windows are rendered and eligible for pointer hit-testing and keyboard focus.
- The focused window can be moved to another workspace and followed with a shortcut.
- Window positions and sizes remain intact across switches.
- The window manager remembers the most recently focused surface per workspace and falls back to an available visible window if needed.

### Window gaps

The snapping layout engine supports two independent gap parameters:

- **Outer gap:** spacing between the output edge and snapped windows.
- **Inner gap:** spacing between adjacent left/right snapped windows.

The current development defaults in `src/layout.rs` are `outer: 12` and `inner: 12` (logical units). They are **compile-time** settings today. Runtime configuration, themes, tiling gaps, and per-workspace preferences are future work.

## Architecture

```text
                              Octlitch Linux
                                    |
                          Anwilite compositor
                                 (Rust)
                                    |
              +---------------------+--------------------+
              |                     |                    |
      Smithay / Wayland        WindowManager       Layout / Workspaces
      XDG protocol state       stacking, input,      geometry, snapping,
      surface commits          drag, resize          groups, focus
              |                     |                    |
              +---------------------+--------------------+
                                    |
                     Winit + GLES renderer (today)
                    DRM/KMS + GPU backend (planned)
                                    |
                        Client Wayland surfaces
                       Weston Terminal (tested)
                       Other apps / games (goal)

             Future: Quickshell/QML desktop shell
                  <--> layer shell + Anwilite IPC
```

### Source overview

```text
.
├── Cargo.toml                # Rust package and dependency declarations
├── Cargo.lock                # Locked binary dependencies; commit this file
├── README.md                 # Project status, how to run, roadmap
├── .gitignore                # Build output, local backups, secrets
└── src/
    ├── main.rs              # App state, Smithay protocol handlers,
    │                        # input, Calloop event loop, rendering
    ├── window.rs            # ManagedWindow, WindowManager, geometry,
    │                        # interactive drag/resize, placement, focus
    ├── workspace.rs         # Ten-workspace state and switching
    └── layout.rs            # Pure snapping geometry and unit tests
```

The source tree above describes the currently developed files. It may expand as we extract modules from `main.rs` and add backends, a configuration system, IPC, and protocol implementations.

### Rendering and event loop

Current nested-backend implementation includes:

1. A Wayland listening socket and client request dispatch through Smithay.
2. A **Calloop** event loop that wakes for Wayland activity, Winit input/redraw events, and pending resize work rather than rendering continuously at idle.
3. Smithay `OutputDamageTracker` to determine which screen regions actually need updating.
4. Buffer-age-aware rendering and damage-region submission when there is damage; presentation is skipped when there is none.
5. Frame callbacks to eligible surfaces on the active workspace.
6. Reconfiguration and damage-tracker reset when the nested output is resized.
7. A short configure interval during interactive resizing to avoid unnecessary resize churn.

Early **VirtualBox** experiments measured approximately **0% sampled idle CPU** after the event-driven scheduler, versus roughly **3.7% in an earlier polling implementation**. These are preliminary development observations, **not** a rigorous benchmark, a promise of zero CPU usage, or a measurement of native hardware performance. Frame-submit diagnostic counters similarly are not a universal FPS measurement.

### Wayland window sizing

The compositor does not directly resize an application's buffer. It calculates desired geometry, sends an XDG toplevel configure, and waits for the client to acknowledge/commit a new buffer. For maximize, snap, restore, and unsnap transitions, position corrections account for client-reported window geometry and decoration offsets. This distinction is important for avoiding flicker, jumps, and incorrect anchoring.

## Build and run (development)

### Requirements

- Linux host with a working Wayland graphical session; the project currently uses the Winit nested backend.
- A working Rust toolchain (`cargo`, `rustc`, `rustfmt`).
- System development libraries required by the **exact versions and features in `Cargo.toml`**, including dependencies of Smithay/Winit/GLES. Package names vary by Fedora release and enabled features; consult the first missing-library error if the environment is not already set up.
- `weston-terminal` for the current test client (usually from the distribution's Weston packages).

This README describes the **existing development workflow**, not a complete one-command installation procedure. Native install packaging and dependency documentation will be added later.

### Compile and test

```bash
cargo fmt --all -- --check
cargo check
cargo test layout::tests
cargo test
cargo run --release
```

Running `cargo run --release` opens the nested output. In the current source, Anwilite automatically attempts to launch `weston-terminal` connected to its Wayland socket.

**Socket naming note:** The original development code uses `octlitch-0` as its Wayland socket name. We are renaming the compositor project to **Anwilite**, but the socket may retain that legacy name until the runtime naming cleanup. Verify `ListeningSocketSource::with_name(...)` in `src/main.rs` rather than assuming the socket name.

To launch another test client from a separate host terminal, using the current legacy socket:

```bash
WAYLAND_DISPLAY=octlitch-0 weston-terminal
```

When the socket is deliberately renamed to `anwilite-0`, update this command, the client's environment variable in `src/main.rs`, and any local scripts together.

### Development tips

- Run and test one change at a time; preserve a working Git commit before altering window-placement logic.
- Keep **pure geometry** in `src/layout.rs`, with unit tests for odd output sizes, gaps, and edge cases.
- Test input focus, window stacking, pointer hit-testing, resizing, and rendering together whenever changing workspaces or window lifecycle.
- Verify both keyboard shortcuts and application-requested XDG actions.
- Before reporting a rendering regression, test a stationary window, an actively updating terminal, drag/resize, workspace switching, and output resizing.
- Do not confuse terminal OSC warnings with Wayland protocol or compositor-rendering faults.

## Milestone history

This is a **summary of completed learning milestones**, not a reconstructed Git commit history. The first Git commit captures the current working source at one point in time.

|  Implemented and verified |
| --- | --- |
| Nested Winit/Smithay compositor, Wayland client handling, GLES rendering, pointer and keyboard input, basic floating window manager |
|  Damage tracking, buffer-age reuse, frame callbacks, redraw-on-demand, event-driven Calloop scheduling, resize configure pacing and diagnostics |
| Eight-direction interactive window resizing (all 8 directions passed) |
|  XDG maximize/restore, saved position and size, output resize awareness (all 7 checks passed) |
| Workspace state, ten-workspace switching, per-workspace visibility/hit-testing and keyboard focus |
| Move the focused window between workspaces and follow it |
| Per-workspace focus memory, fallback focus, window close behavior; subsequent typing/focus regressions investigated and reported working |
|  Pure half-screen snapping geometry; 4/4 tests passed |
| Super+arrow snapping, maximize/restore, saved floating geometry, output resize handling; 9/9 checks passed |
|  Configurable inner/outer snapping gaps; 7/7 geometry tests and 8/8 behavior checks passed |
|  Asynchronous snap-to-floating transition and pointer anchoring while dragging; 10/10 checks passed |

## Roadmap

The sequence below expresses **planned priorities**, not guaranteed delivery dates or implemented functionality. Lesson numbers after 26D are provisional.

### Stage 1 — Hybrid window manager (next)

- [x] Floating windows, moving, resizing, maximize/restore.
- [x] Ten workspaces, workspace switching, moving windows, focus history.
- [x] Windows-style left/right snapping and original-geometry restoration.
- [x] Configurable snapped-window gaps.
- [x] Drag to unsnap without jumping.
- [ ] Automatic layout for two windows on a workspace, keeping floating behavior available.
- [ ] Generalize tiling to three or more windows, layout recalculation when opening/closing/moving clients.
- [ ] Per-workspace **floating / tiling** modes with explicit layout selection.
- [ ] Additional Windows-style placements, including quarter snapping and sensible keyboard transitions.
- [ ] Directional focus and window movement, Alt+Tab, close-window and launcher shortcuts.
- [ ] Configuration files for gaps, layouts, bindings, appearance, and workspace rules.
- [ ] Robust handling of size-constrained clients, popups, transient dialogs, and unsnap timing.

### Stage 2 — Desktop protocols and control plane

- [ ] Real XDG popup placement, grabbing, repositioning, and focus behavior (current popup hooks are placeholders).
- [ ] Fullscreen protocol and proper fullscreen stacking/focus semantics, separate from maximize.
- [ ] Layer-shell support for desktop panels, overlays, and widgets.
- [ ] Anwilite IPC API/events for workspaces, active window, layouts, focus, and configuration.
- [ ] Clipboard/data-device hardening, primary selection, drag and drop, and text input/IME where needed.
- [ ] Notifications, output information/management, screencopy/screenshot paths, and other shell protocols.

### Stage 3 — Quickshell desktop

- [ ] Anwilite's own **Quickshell/QML** configuration and visual identity.
- [ ] Top bar with the ten-workspace indicator and active-window information.
- [ ] Keyboard-driven app launcher and power/session menu.
- [ ] Notifications, notification history, system tray and quick settings.
- [ ] Volume, brightness, battery/power, network, and Bluetooth widgets where supported.
- [ ] Theming, animation, backgrounds, and cohesive Octlitch Linux integration.

### Stage 4 — Gaming and hardware

- [ ] XWayland for X11 applications and games, including Steam/Proton workflows.
- [ ] Relative pointer and pointer constraints/locking for games.
- [ ] Native **DRM/KMS**, udev, libinput, seat/session management, and GPU output.
- [ ] Multiple monitors, refresh rates, modes, scaling, and hotplug.
- [ ] Frame scheduling/presentation feedback, VSync behavior, latency measurements.
- [ ] Evaluate direct scanout when possible; preserve fallbacks when compositing is necessary.
- [ ] Hardware testing across GPU vendors, suspend/resume, and failure/recovery cases.

### Stage 5 — Distribution and release engineering

- [ ] Repeatable build, test, CI, and packaging workflows.
- [ ] Octlitch Linux packages and a dedicated login/session entry.
- [ ] Default Anwilite and Quickshell configs, optional fallback desktop/session.
- [ ] User documentation, troubleshooting, bug templates, compatibility matrix.
- [ ] Security review, real-world stress testing, and reproducible release artifacts.

## Philosophy and scope

Anwilite is **not a fork of Hyprland** and not a theme applied to KDE. It is its own Rust/Smithay compositor, developed incrementally to understand Wayland concepts and make considered architecture choices. Hyprland, Omarchy, and Windows provide **workflow inspiration**, not code provenance or guarantees of feature parity.

The intended hybrid model lets users choose automatic tiling when useful without sacrificing straightforward floating-window control. Longer term, each workspace could select its own layout—for example, a floating communication workspace, a tiled development workspace, and a fullscreen gaming workspace.

Our engineering principles are to keep code readable; teach and document the state transitions; preserve a working checkpoint; isolate pure, testable layout math; avoid unnecessary idle rendering; and validate native hardware behavior before making performance or compatibility claims.

## Contributing

This is an early-stage learning and development project. Discussions, reproducible bug reports, design feedback, and Rust/Wayland contributions are welcome once the repository is published and its contribution process is established. For bugs, include the Rust toolchain, distro/host compositor, GPU or VM backend, exact reproduction steps, and any relevant Smithay/Anwilite logs.

## License

**Not selected yet.** No open-source license is implied by publishing source code. Choose and add a `LICENSE` file before inviting external reuse or contributions. Possible license families have different obligations and should be selected deliberately.

## Related projects

- **[Octlitch Linux](https://octlitch.com)** — the intended host Linux distribution.
- **[Smithay](https://github.com/Smithay/smithay)** — Rust Wayland compositor building blocks.
- **[Quickshell](https://quickshell.org/)** — planned QML desktop-shell framework.
- **[Wayland](https://wayland.freedesktop.org/)** — compositor/client display and input protocol ecosystem.

---

**Anwilite — a Wayland desktop built for Octlitch, one tested system at a time.**
