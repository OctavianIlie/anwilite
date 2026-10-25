# Anwilite

**WORK IN PROGRESS: A custom Wayland compositor and hybrid window manager for [Octlitch Linux](https://octlitch.com).**

> **Project status: experimental, pre-alpha.** The nested compositor, floating-window workflows, and per-workspace automatic tiling are working and have been regression-tested. Native DRM/KMS, a complete desktop shell, XWayland, and several essential Wayland protocols are still planned. Anwilite is not ready to replace a daily-driver desktop session.

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
| Smithay XDG toplevel windows; keyboard and mouse input | ✅ Working |
| Floating windows, click-to-focus/raise, drag, and eight-direction resize | ✅ Working and tested |
| Maximize/restore and Windows-style half-screen snapping | ✅ Working and tested |
| Ten workspaces, move-and-follow, per-workspace focus history | ✅ Working and tested |
| Per-workspace floating/automatic-tiling modes | ✅ Working and tested |
| Master-and-stack tiling, automatic reflow on window changes | ✅ Working and tested |
| Per-workspace adjustable master width (20–80%, 5% steps) | ✅ Working and tested |
| Keyboard focus navigation, tile swapping, promote-to-master | ✅ Working and tested |
| Tiled-window minimize/restore and maximize/restore | ✅ Working and tested |
| Original floating geometry restored after disabling tiling | ✅ Working and tested |
| Nested output resize, event-driven redraws, damage tracking | ✅ Working and tested |
| Native DRM/KMS + libinput session | 📋 Planned |
| True fullscreen and gaming input protocols | 📋 Planned |
| XWayland | 📋 Planned |
| Layer shell, Quickshell integration, compositor IPC | 📋 Planned |
| Multi-monitor/output management | 📋 Planned |

## Current user experience

Anwilite currently runs **nested inside a host Wayland desktop** via Smithay's Winit/OpenGL ES renderer. The development environment has been **Fedora 44 in VirtualBox with KDE Plasma Wayland**. Weston Terminal clients can be opened inside the Anwilite output, floated, snapped, or automatically tiled. Each of the ten workspaces can independently use floating or tiling mode.

### Keyboard shortcuts

**Workspaces**

| Shortcut | Action |
| --- | --- |
| `Super + 1` … `Super + 9` | Switch to workspaces 1–9 |
| `Super + 0` | Switch to workspace 10 |
| `Super + Shift + 1` … `Super + Shift + 9` | Move the focused window to workspace 1–9 and follow it |
| `Super + Shift + 0` | Move the focused window to workspace 10 and follow it |

**Window placement and tiling**

| Shortcut | Action |
| --- | --- |
| `Super + Shift + T` | Toggle floating/tiling mode for the active workspace |
| `Super + H` | Narrow the tiled master column by 5% |
| `Super + J` | Widen the tiled master column by 5% |
| `Super + R` | Reset the tiled master column to 50% |
| `Super + Shift + H` | Focus the previous visible tile (wraps around) |
| `Super + Shift + J` | Focus the next visible tile (wraps around) |
| `Super + Shift + Left` | Swap the focused tile with the previous visible tile |
| `Super + Shift + Right` | Swap the focused tile with the next visible tile |
| `Super + Shift + P` | Promote the focused stack tile directly to master |
| `Super + M` | Minimize the focused window |
| `Super + Shift + M` | Restore the most recently minimized window on the active workspace |
| `Super + Left` | Snap the focused floating window to the left half |
| `Super + Right` | Snap the focused floating window to the right half |
| `Super + Up` | Maximize the focused window |
| `Super + Down` | Restore the focused window from maximized/snapped placement |

**Pointer interactions**

| Input | Action |
| --- | --- |
| Left-click | Focus a window; floating windows also rise to the front, tiled windows keep their order |
| Drag a window titlebar | Move a floating window, or restore and drag a snapped window |
| Middle-button drag | Move a floating window, or unsnap and move a snapped window |
| Window resize handles | Resize floating windows from all eight edges/corners |

**Nested-development fallback:** KDE or the host compositor may intercept `Super` combinations before Anwilite sees them. Anwilite also accepts `Ctrl + Alt` in place of `Super` for its shortcuts (and retains `Shift` when required). In the current KDE/VirtualBox development setup, `Ctrl + Alt + Shift + Left/Right` was successfully tested for tile swapping, and `Ctrl + Alt + Shift + P` for master promotion. Some corresponding `Super` combinations are reserved by the host. In particular, **`Super + L` is not an Anwilite master-ratio shortcut** because it locks the host desktop; use `Super + J` or `Ctrl + Alt + J` to widen the master column. This is a development keymap, not a configurable final binding system.

### Automatic master-and-stack tiling

Tiling is **opt-in per workspace**, toggled with `Super + Shift + T`:

- **One visible window:** fills the usable output, respecting outer gaps.
- **Two visible windows:** one master column on the left and one stack window on the right.
- **Three or more visible windows:** one left master and vertically stacked windows in the right column.
- **Adjustable master width:** each workspace starts at **50%**; `Super + H/J` adjusts it in **5%** steps, clamped to **20–80%**. `Super + R` resets to 50%.
- **Workspace independence:** the mode, selected ratio, and tile order are maintained separately for the current compositor session. No configuration file or persistence across compositor restarts is implemented yet.
- **Dynamic reflow:** creating, closing, minimizing, restoring, or transferring windows recalculates the relevant tiled workspaces. Resizing the nested output also recalculates tile geometry.
- **Navigation versus placement:** keyboard focus navigation does **not** rearrange tiles; swapping and promoting to master **do** change tile order while preserving focus on the same client.
- **Maximized tiles:** a maximized tile stays zoomed until restored; its underlying tile position reflects layout updates. Tile swaps and promotion are blocked while a tile is maximized.
- **Returning to floating:** disabling tiling restores windows' pre-tiling floating geometry instead of keeping tiled sizes.

The compositor uses each `ManagedWindow`'s position in `WindowManager.windows` to determine master/stack assignment, while `src/layout.rs` computes the rectangles. Swapping exchanges whole window records, preserving each client's saved floating state rather than assigning that state to a different window.

### Floating, snapping, and workspaces

**Snapping details:** The normal floating geometry is saved before placement. A window can transition from floating → left snap → right snap → maximized → restored without losing its original geometry. Snapped windows stay aligned when the nested output is resized. Dragging a snapped titlebar requests the original floating size, waits for the client to commit it, then reanchors the cursor relative to the restored geometry to avoid a visible jump.

**Ten workspaces:** Newly created windows belong to the active workspace. Switching workspaces updates rendering, hit-testing and keyboard-focus eligibility; move-and-follow changes a focused window's workspace and reflows tiled source/destination workspaces as needed. The window manager remembers the most recently focused surface per workspace and falls back to another visible window when necessary.

### Window gaps

The shared snapping/tiling layout engine uses independent **outer** gaps (between windows and the output edge) and **inner** gaps (between adjacent windows). The current `DEFAULT_SNAP_GAPS` in `src/layout.rs` is **`outer: 6`, `inner: 6`** logical units. These are compile-time constants; runtime configuration remains planned. Tiling master ratios are adjustable at runtime but the gap sizes are not.

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
      XDG protocol state       placement, focus,     snapping, tiling,
      surface commits          drag, resize, swap    workspace modes
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
├── README.md                 # Project status, shortcuts, build, roadmap
├── .gitignore                # Build output, local backups, secrets
└── src/
    ├── main.rs              # App state, Smithay protocol handlers,
    │                        # input, Calloop event loop, rendering
    ├── window.rs            # ManagedWindow, WindowManager, geometry,
    │                        # tiling/reflow, drag/resize, focus, swapping
    ├── workspace.rs         # Ten-workspace state and switching
    └── layout.rs            # Pure snapping/tiling geometry and unit tests
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

The compositor does not directly resize an application's buffer. It calculates desired geometry, sends an XDG toplevel configure, and waits for the client to acknowledge/commit a new buffer. For tiling, maximize, snap, restore, and unsnap transitions, position corrections account for client-reported window geometry and decoration offsets. This distinction is important for avoiding flicker, jumps, and incorrect anchoring.

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

Running `cargo run --release` opens the nested output. In the current source, Anwilite automatically attempts to launch `weston-terminal` connected to its Wayland socket. The Lesson 28 development session reported **32 passing unit tests** plus successful final advanced-tiling integration checks; rerun `cargo test` against your own checkout after merging.

**Socket naming note:** The original development code uses `octlitch-0` as its Wayland socket name. We are renaming the compositor project to **Anwilite**, but the socket may retain that legacy name until the runtime naming cleanup. Verify `ListeningSocketSource::with_name(...)` in `src/main.rs` rather than assuming the socket name.

To launch another test client from a separate host terminal, using the current legacy socket:

```bash
WAYLAND_DISPLAY=octlitch-0 weston-terminal
```

When the socket is deliberately renamed to `anwilite-0`, update this command, the client's environment variable in `src/main.rs`, and any local scripts together.

### Development tips

- Run and test one change at a time; preserve a working Git commit before altering window-placement logic.
- Keep **pure geometry** in `src/layout.rs`, with unit tests for odd output sizes, gaps, ratios, tile counts, and edge cases.
- Test input focus, window ordering, pointer hit-testing, resizing, and rendering together whenever changing tiling, workspaces, or the window lifecycle.
- Verify both keyboard shortcuts and application-requested XDG actions.
- Before reporting a rendering regression, test a stationary window, an actively updating terminal, drag/resize, workspace switching, and output resizing.
- Do not confuse terminal OSC warnings with Wayland protocol or compositor-rendering faults.

## Milestone history

These are **completed and tested development milestones**, grouped by feature area rather than reconstructed from Git commits.

### Compositor and rendering

- Built a nested Rust/Smithay compositor using Winit and GLES, with XDG toplevel clients, pointer input, and keyboard input.
- Added event-driven Calloop scheduling, damage tracking, buffer-age-aware submission, frame callbacks, redraw-on-demand, and resize diagnostics.

### Floating windows and snapping

- Added click-to-focus, raising, dragging, and **eight-direction interactive resizing** (all 8 directional checks passed).
- Implemented XDG maximize/restore with saved position and size and output-resize handling (7/7 checks passed).
- Added left/right half-screen snapping, with pure geometry tests (4/4) and keyboard/restore behavior checks (9/9).
- Introduced independent inner and outer gaps (7/7 geometry tests and 8/8 behavior checks).
- Implemented drag-to-unsnap with asynchronous client resizing and stable pointer anchoring (10/10 checks passed).

### Workspaces and keyboard focus

- Added ten workspaces, workspace-local window visibility and hit-testing, and move-and-follow shortcuts.
- Added per-workspace focus history, fallback focus after a window closes or becomes hidden, and focus recovery.

### Automatic and advanced tiling — Lesson 28

- Implemented automatic **master-and-stack tiling** for one or more visible windows, with independent floating/tiled modes per workspace.
- Added layout reflow when windows open, close, move between workspaces, minimize, or restore, and when the nested output resizes.
- Added independent **master-width ratios per workspace** (20–80%, adjusted in 5% steps; reset to 50%).
- Added keyboard navigation between visible tiles without changing their order.
- Added adjacent tile swaps and **promote focused window to master**, preserving focus and original floating geometry.
- Passed all **seven Lesson 28 integration checks**, covering promotion, swapping/navigation, ratio adjustments, minimize/restore, workspace isolation, floating restoration, maximize/restore, and keyboard focus.

## Roadmap

The sequence below expresses **planned priorities**, not guaranteed delivery dates or implemented functionality.

### Stage 1 — Hybrid window manager (core tiling implemented)

- [x] Floating windows, moving, resizing, maximize/restore.
- [x] Ten workspaces, workspace switching, moving windows, focus history.
- [x] Windows-style left/right snapping and original-geometry restoration.
- [x] Compile-time-configurable snapping/tiling gaps.
- [x] Drag to unsnap without jumping.
- [x] Automatic tiling for one, two, and three or more windows, with reflow as clients change.
- [x] Per-workspace **floating / tiling** modes.
- [x] Independent adjustable master-width ratios (20–80%) for tiled workspaces.
- [x] Keyboard cycling between tiles without reordering them.
- [x] Adjacent tile swapping and promote-focused-tile-to-master.
- [x] Tiled-window minimize/restore, maximize/restore, workspace transfers, and floating restoration.
- [ ] Visible active-window focus indicator / focus border (planned next).
- [ ] Additional Windows-style placements, including quarter snapping and sensible keyboard transitions.
- [ ] Spatial directional focus, global Alt+Tab, close-window and launcher shortcuts.
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

The current hybrid model lets users select floating or automatic master-and-stack tiling independently on each workspace. Longer term, configurable layouts could support a floating communication workspace, a tiled development workspace, and a fullscreen gaming workspace.

Our engineering principles are to keep code readable; teach and document the state transitions; preserve a working checkpoint; isolate pure, testable layout math; avoid unnecessary idle rendering; and validate native hardware behavior before making performance or compatibility claims.

## Contributing

This is an early-stage learning and development project. Discussions, reproducible bug reports, and design feedback are welcome; formal contribution guidelines and review processes have not been established yet. For bugs, include the Rust toolchain, distro/host compositor, GPU or VM backend, exact reproduction steps, and any relevant Smithay/Anwilite logs.

## License

**Not selected yet.** No open-source license is implied by publishing source code. Choose and add a `LICENSE` file before inviting external reuse or contributions. Possible license families have different obligations and should be selected deliberately.

## Related projects

- **[Octlitch Linux](https://octlitch.com)** — the intended host Linux distribution.
- **[Smithay](https://github.com/Smithay/smithay)** — Rust Wayland compositor building blocks.
- **[Quickshell](https://quickshell.org/)** — planned QML desktop-shell framework.
- **[Wayland](https://wayland.freedesktop.org/)** — compositor/client display and input protocol ecosystem.

---

**Anwilite — a Wayland desktop built for Octlitch, one tested system at a time.**
