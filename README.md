# Anwilite

**A Rust Wayland compositor and hybrid window manager for [Octlitch Linux](https://octlitch.com).**

> **Status: experimental / pre-alpha.** Anwilite runs as a nested Wayland compositor with per-workspace Windowed, Tiled and Grid layouts, true XDG fullscreen, configurable controls, a basic desktop environment, and tested Wayland clipboard and Dolphin file drag-and-drop workflows. It is not yet a standalone login session or a replacement for a production desktop, and gaming compatibility is still under development.

Anwilite is built with [Smithay](https://github.com/Smithay/smithay) (currently Smithay 0.7), Rust, Calloop, and OpenGL ES. It aims to provide a responsive, keyboard-driven desktop with flexible window management, a cohesive Octlitch visual identity, and a solid foundation for Linux gaming.

**Anwilite** is the compositor and window manager. **Octlitch Linux** is the distribution it is being developed for. The projects are related but independently maintainable.

## Project status

| Feature | Status |
| --- | --- |
| Nested Wayland compositor using Winit and GLES | ✅ Working |
| XDG toplevel windows, keyboard and pointer input | ✅ Working |
| Floating windows, dragging, interactive resizing, click-to-focus | ✅ Working |
| Ten workspaces, move-and-follow, workspace-local focus history | ✅ Working |
| Independent per-workspace Windowed, Tiled and Grid modes | ✅ Working |
| Windowed ↔ Tiled and Windowed ↔ Grid toggle shortcuts, including Tiled ↔ Grid transitions | ✅ Working |
| Master-and-stack tiling, gaps, adjustable ratios, tile navigation and swapping | ✅ Working |
| Snapping, maximizing, minimizing and restoring windows | ✅ Working |
| Focus borders and configurable window appearance | ✅ Working |
| TOML configuration, configurable application commands and keybindings | ✅ Working |
| Show desktop and application-launcher shortcuts | ✅ Working |
| Layer-shell surfaces, output information and layer input handling | ✅ Working |
| Waybar, Swaybg and Fuzzel integration | ✅ Working |
| Waybar custom indicator for the active workspace’s layout mode | ✅ Working |
| Exclusive work areas for panels and docks | ✅ Working |
| Output resizing, damage tracking and redraw-on-demand | ✅ Working |
| XDG popup menus, nested submenus, dismissal and transient dialogs | ✅ Working |
| Vertical/horizontal mouse-wheel and pointer-axis scrolling | ✅ Working |
| Client-supplied pointer cursors, hotspot alignment and compositor resize cursors | ✅ Working |
| Popup click-through prevention | ✅ Working |
| Wayland clipboard copy/paste between nested applications and workspaces | ✅ Working |
| Dolphin-to-Dolphin file drag-and-drop, copy, cancel and overwrite | ✅ Working |
| Client-supplied drag-icon rendering (render-only implementation) | ✅ Working |
| Overwrite and copy-progress dialogs over tiled and grid workspaces | ✅ Working |
| Cross-application drag-and-drop, full drag-icon offset semantics | Not yet verified |
| True XDG fullscreen distinct from maximize, with layout restoration | ✅ Working |
| Relative pointer input and pointer locking/confinement | Planned |
| XWayland, Steam and Proton workflows | Planned |
| Native DRM/KMS, libinput and independent login session | Planned |
| Multi-monitor management, variable refresh rate and direct scanout | Planned |

**✅ Working** means the feature is functioning in the nested development environment. Checked roadmap boxes mark completed milestones, not production readiness.


These results describe the current nested development environment. They are not a claim of broad hardware compatibility or finished production-quality behavior.

## Features

### Windowed, Tiled and Grid layouts

Each of the ten workspaces has its own layout mode; changing one workspace does not change the others.

- **Windowed (floating):** drag, resize from window edges or corners, raise on click, snap to either half of the screen, maximize, restore, and minimize.
- **Tiled:** one window takes the usable area, two windows form master and stack columns, and additional windows stack vertically beside the master.
- **Grid:** arrange windows in a workspace-local grid, with reflow as the set of windows changes.
- **Mode shortcuts:** `Super + Shift + T` toggles between Windowed and Tiled; `Super + Shift + G` toggles between Windowed and Grid. From Tiled, the Grid shortcut enters Grid; from Grid, the Tiled shortcut enters Tiled. Pressing the same toggle again returns to Windowed.
- **Configurable layout:** independent outer and inner gaps, per-workspace master-width overrides, and a configurable default master ratio for tiling. Master ratios can be adjusted between 20% and 80%.
- **Window ordering:** navigate between tiles, swap adjacent tiles, or promote a window to the master position without losing its original floating geometry.
- **Lifecycle handling:** recalculate placements when windows are opened, closed, minimized, restored, transferred between workspaces, or when the output changes size.

Tiling also checks client-reported size constraints. When a layout cannot accommodate its windows, Anwilite preserves the windows rather than deliberately forcing them into overlapping tile slots.

### Focus and window appearance

Anwilite draws active-window indicators with configurable colors, thickness, and spacing. Window focus follows the compositor's visible-window and layer-shell rules. Tiled and snapped windows use placement-aware clipping and borders.

Applications can draw their own window decorations. A client may choose to hide its titlebar buttons while maximized; Anwilite does not yet provide a complete compositor-drawn titlebar and button system.

### Input, clipboard, popups and drag-and-drop

Anwilite forwards mouse-wheel and horizontal/vertical pointer-axis events to Wayland clients. Client-requested cursor images are handled as named host cursors or compositor-rendered Wayland cursor surfaces; the compositor retains control over external window-resize indicators. In nested manual tests, cursor, scrolling, popup and window interactions worked together without reported regressions.

The standard Wayland data device supports copy/paste between clients inside the nested session, including after switching workspaces or interacting with menus. Dolphin-to-Dolphin file drag-and-drop, cancellation, rejected drops and overwrite confirmation were exercised. The overwrite and copy-progress windows float without knocking the two Dolphin windows out of their tiled layout.

Client-supplied drag icon surfaces are drawn above application windows and follow the pointer. **The currently tested implementation is Stage 2 (render-only):** it retains the optional icon `wl_surface`, requests redraw on pointer motion, draws the surface, supplies frame callbacks, and clears it at drag completion. It intentionally does **not** modify `SurfaceAttributes::buffer_delta`; full client-controlled committed offsets/hotspots and cross-application transfers are follow-up work.

The standard nested clipboard is **not** a persistent clipboard manager or an automatic bridge to the host KDE clipboard. Similarly, supporting a drag icon does not imply that every application accepts every dragged MIME type.

### Panels, wallpaper and launcher

The development desktop currently uses:

- **Waybar** for a panel, clock and active-workspace layout-mode label.
- **Swaybg** for the background.
- **Fuzzel** as a keyboard-driven application launcher.

Anwilite supports `zwlr_layer_shell_v1` surfaces with the appropriate background, bottom, top and overlay stacking, as well as layer keyboard/pointer interactions. The virtual output is advertised as `ANWILITE-1` and includes XDG logical-output information for panel clients.

Panels can request **exclusive zones**. For example, a 36-pixel top Waybar on a 1280 × 800 output reserves the work area `(0, 36)` with size `1280 × 764`. Maximized, snapped and tiled windows use the remaining work area. If the panel exits, its reserved space is released; when it returns, window placement is recalculated. Background layers still cover the full output.

### True fullscreen

True XDG fullscreen is implemented independently of maximize and the Windowed/Tiled/Grid layout mode. In the nested Winit backend, fullscreen occupies the entire **Anwilite virtual output**, hides Waybar and unrelated desktop content, and responds to changes in virtual-output size. Exiting fullscreen restores the window to its prior placement, including its tile or grid cell. The underlying workspace mode is not changed.


### Configuration

The main configuration is located at:

```text
~/.config/anwilite/config.toml
```

It provides settings for workspace layout modes, tiling, gaps, master ratios, borders, application launch commands and keyboard bindings. Anwilite checks for configuration changes during the session and applies supported changes after validation. Some settings or active window transitions may require a restart or a later retry.

Keybindings are user-editable through `[[keybindings]]` entries. A fully specified keybinding list is authoritative: removing an entry removes that binding rather than silently restoring it. Reserved recovery bindings remain available for reloading configuration.

Waybar is configured separately through `~/.config/anwilite/waybar.jsonc` and `~/.config/anwilite/waybar.css`. For a panel that reserves its space, set `"exclusive": true` in the Waybar configuration.

## Keyboard shortcuts

The following are the familiar bindings used in the development setup. **Your `config.toml` is authoritative** if you have customized the keybinding list.

| Shortcut | Action |
| --- | --- |
| `Super + 1` … `Super + 9` | Switch to workspaces 1–9 |
| `Super + 0` | Switch to workspace 10 |
| `Super + Shift + 1` … `Super + Shift + 0` | Move the focused window to a workspace and follow it |
| `Super + Shift + T` | Toggle Tiled mode for the current workspace (or switch Grid → Tiled) |
| `Super + Shift + G` | Toggle Grid mode for the current workspace (or switch Tiled → Grid) |
| `Super + H` / `Super + J` | Decrease / increase the master ratio |
| `Super + R` | Reset the current workspace's master ratio |
| `Super + Shift + H` / `Super + Shift + J` | Focus previous / next tile |
| `Super + Shift + Left` / `Super + Shift + Right` | Swap with previous / next tile |
| `Super + Shift + P` | Promote the focused tile to master |
| `Super + Left` / `Super + Right` | Snap a floating window to either half |
| `Super + Up` / `Super + Down` | Maximize / restore |
| `Super + M` / `Super + Shift + M` | Minimize / restore most recently minimized window |
| `Super + Return` | Launch the configured terminal |
| `Super + E` | Launch the configured file manager |
| `Super + Space` | Open the configured application launcher |
| `Super + D` | Show / restore the desktop on the current workspace |

In a nested session, the **host compositor may intercept shortcuts** before Anwilite receives them. The development keymap also supports configurable `Ctrl + Alt` alternatives for many `Super` bindings. Check the actual configured shortcuts rather than assuming the host will forward every key combination.

## Build and run

### Requirements

- A Linux system with an existing Wayland desktop session for the Winit nested backend.
- A Rust toolchain with Cargo, rustfmt and Clippy.
- The system libraries required by the dependency versions and feature flags in `Cargo.toml`.
- Optional development clients such as `weston-terminal`.
- For the desktop-component supervisor: `waybar`, `swaybg`, `wayland-info` and the corresponding configuration files.

The nested implementation has been developed and tested under Fedora/KDE Wayland, including a VirtualBox development environment. Dependencies and behavior may differ on other distributions or hardware.

### Build and validate

```bash
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Run the compositor nested inside your existing Wayland session:

```bash
cargo run --release
```

The development compositor uses the Wayland socket `octlitch-0` and output name `ANWILITE-1`. From a separate host terminal, an additional test client can be started with:

```bash
env -u WAYLAND_SOCKET WAYLAND_DISPLAY=octlitch-0 weston-terminal
```

The socket name is currently retained for compatibility with development scripts. Do not assume it has been renamed to match the compositor project.

### Desktop development session

If the nested-session and component-supervisor scripts are installed in `scripts/`, first build the release binary, then start:

```bash
cargo build --release
./scripts/anwilite-nested-session \
  --private-bus \
  -- ./scripts/anwilite-session-components
```

This starts Anwilite and, once its virtual output is ready, launches Waybar and Swaybg against `octlitch-0`. To preserve the existing host session bus for development, use `--shared-bus` instead.

**Important:** The private-bus option is a development aid. It does **not** create a standalone PAM/logind login session, a security sandbox, or a production-ready desktop environment. Start it from a terminal connected to the **host** Wayland display, not from a shell targeting `octlitch-0`.

The supervisor writes diagnostic logs under:

```text
~/.cache/anwilite/compositor.log
~/.cache/anwilite/waybar.log
~/.cache/anwilite/swaybg.log
```

Desktop application activation can still interact with processes in the host session. In particular, applications that reuse an existing instance or rely on D-Bus activation may open on the host desktop. Development launch helpers can reduce some of these issues, but they do not replace a proper independent session.

## Architecture

```text
Anwilite (Rust / Smithay)
├── Wayland protocols
│   ├── Compositor, XDG shell, seats, data device (clipboard and DnD)
│   ├── Virtual output and XDG output information
│   └── Layer shell (panels, overlays, backgrounds)
├── Window management
│   ├── Windowed, Tiled and Grid, snapping, maximize/minimize
│   ├── Workspaces, focus, true fullscreen, transient placement and layout constraints
│   └── Exclusive work areas and focus indicators
├── Pointer axis, client cursor and client drag-icon rendering
├── Configuration and application launching
├── Calloop event loop and Wayland socket
└── Winit + OpenGL ES (current nested backend)
    └── Native DRM/KMS and libinput (planned)
```

The main Rust modules include:

| File | Responsibility |
| --- | --- |
| `src/main.rs` | Compositor state, protocol handlers, input, event loop and rendering |
| `src/window.rs` | Managed windows, placement, focus, Windowed/Tiled/Grid layouts, fullscreen state and interactive operations |
| `src/layout.rs` | Testable snapping, tile geometry, gaps and placement calculations |
| `src/workspace.rs` | Workspace state and switching |
| `src/config.rs` | Configuration parsing, validation and reload |
| `src/keybindings.rs` | Keyboard shortcuts and command mapping |
| `src/appearance.rs` | Focus-border appearance and validation |

### Rendering and event loop

Anwilite uses Calloop to handle client requests, input, configuration checks and resize events. Its Winit/GLES backend uses Smithay's output damage tracking, buffer-age information, and frame callbacks to avoid unnecessary submissions when the output has not changed.

Output-resize handling updates the advertised virtual output, layer geometry, placed-window layouts and rendering damage tracking. Existing layer clients are reconfigured to follow the nested window's dimensions.

These optimizations have been exercised in the development environment, **not** benchmarked as evidence of gaming performance on native hardware. Frame-submit counters are diagnostic and do not by themselves measure end-to-end latency or monitor presentation rate.

## Roadmap

The following stages describe planned capabilities, not guaranteed release dates. Checked items are implemented in the current nested development environment unless otherwise noted. An unchecked item remains open even when some related code exists. A feature being implemented does not imply production readiness.

### Stage 1 — Hybrid window manager

- [x] Floating windows, moving, interactive resizing, maximize and restore.
- [x] Ten workspaces, switching, move-and-follow, and per-workspace focus history.
- [x] Left/right half-screen snapping and original-geometry restoration.
- [x] Configurable inner and outer gaps for snapping and tiling.
- [x] Drag to unsnap without jumping.
- [x] Automatic master-and-stack tiling for one, two, and three or more windows, with dynamic reflow.
- [x] Independent per-workspace Windowed/Tiled/Grid modes, T/G toggles, and adjustable master-width ratios.
- [x] Tile focus navigation, swapping, and promotion to master.
- [x] Minimize/restore, show desktop, and configurable application-launcher shortcuts.
- [x] Configurable keybindings, application commands, appearance, and startup tiling preferences.
- [x] Focus borders and exclusive work areas respected by maximized, snapped, and tiled windows.
- [x] Quarter-screen snapping and additional placement transitions.
- [x] Spatial directional focus, global Alt+Tab, and further window-management shortcuts.
- [x] Configurable automatic window/workspace rules and additional layout types (including grid).
- [x] Parent-associated Dolphin overwrite dialogs float over tiled/grid windows without switching the workspace to floating mode; repeated overwrite/cancel tests passed.
- [ ] Broader size-constraint, transient-parent, fullscreen-transition, and uncommon window-lifecycle hardening (beyond the tested Dolphin scenario).
- [ ] Optional decoration policies, including compositor-drawn controls where appropriate.

### Stage 2 — Desktop protocols and control plane

- [x] Layer-shell support for background, bottom, top, and overlay surfaces in the nested backend.
- [x] Layer-shell exclusive zones, keyboard/pointer focus handling, and virtual output information.
- [x] XDG popup placement, grabbing, repositioning, nested submenus, outside-click and Escape dismissal; Dolphin menus and transient dialogs tested.
- [x] Pointer-axis scrolling (vertical and horizontal) without synthetic motion on every scroll event.
- [x] Application-requested cursor rendering, cursor hotspots, and compositor resize-cursor precedence.
- [x] Popup click-through protection for outside-click dismissals.
- [x] Standard Wayland clipboard focus and copy/paste between nested clients.
- [x] Client-originated file drag-and-drop with Dolphin copy, overwrite, cancel and rejected-drop paths.
- [x] Client-supplied drag icon surface rendering, pointer tracking and removal on drop/cancel (Stage 2 render-only).
- [ ] Complete drag-icon offset/hotspot semantics, comprehensive destruction/cancel cleanup, and drag animations beyond current tests.
- [ ] Test and harden cross-application drag-and-drop (Dolphin ↔ Kate/KWrite/Firefox and supported MIME types).
- [ ] Primary selection, middle-click paste, persistent clipboard/history and optional data-control protocol.
- [ ] Text-input/IME, input-method, tablet/touch and other desktop interaction protocols.
- [x] True fullscreen with correct stacking, input handling and restore semantics, separate from maximize (eight nested Firefox acceptance tests passed).
- [ ] Anwilite IPC API and events for workspaces, active windows, layouts, and configuration.
- [ ] Notifications, screencopy/screenshot paths, portals, and additional desktop protocols.
- [ ] Output management beyond the current single virtual output.

### Stage 3 — Desktop shell and experience

- [x] Working nested-development integration with Waybar, Swaybg, and Fuzzel.
- [x] Waybar custom status label follows the active workspace's Windowed/Tiled/Grid mode.
- [ ] Develop an Anwilite-specific Quickshell/QML desktop shell and visual identity.
- [ ] Interactive workspace indicators, active-window information and workspace switching in the finished top bar.
- [ ] Keyboard-driven app launcher and power/session menu as part of the finished shell.
- [ ] Notifications, notification history, system tray, and quick settings.
- [ ] Volume, brightness, battery/power, network, and Bluetooth controls where supported.
- [ ] Cohesive themes, backgrounds, animations, and Octlitch Linux integration.

Waybar, Swaybg, and Fuzzel are the current working development components. A working mode label is not the same as a finished interactive workspace switcher or active-window display. Quickshell/QML remains a planned shell direction; it is not implemented yet.

### Stage 4 — Gaming and hardware

- [ ] XWayland support for X11 applications and Steam/Proton game workflows.
- [ ] Relative pointer motion, cursor confinement/locking, and reliable game input capture.
- [ ] Native DRM/KMS, udev, libinput, seat/session management, and GPU output.
- [ ] Multiple monitors, refresh rates, display modes, scaling, and hotplug handling.
- [ ] Frame scheduling, presentation feedback, synchronization, and measured input-to-display latency.
- [ ] Variable refresh rate and direct scanout where supported, with compositing fallbacks.
- [ ] Gamescope interoperability and practical game compatibility testing.
- [ ] Hardware validation across GPU vendors, suspend/resume, and error recovery.

### Stage 5 — Distribution and release engineering

- [ ] Repeatable builds, automated tests, CI, and packaging workflows.
- [ ] Octlitch Linux packages and a dedicated Anwilite login/session entry.
- [ ] Default compositor and desktop-shell configurations, with a fallback desktop/session option.
- [ ] Documentation, troubleshooting guides, bug templates, and a compatibility matrix.
- [ ] Security review, stress testing, and reproducible release artifacts.


## Design principles

Anwilite is its own Rust/Smithay compositor, not a fork of another window manager or a theme layered over KDE. Its priorities are predictable window behavior, user-configurable workflows, responsive input, readable code and a consistent desktop identity.

The compositor should remain usable for everyday work while allowing games to request uninterrupted fullscreen presentation and appropriate input handling. Visual effects should not take priority over frame consistency, and claims about gaming compatibility or low latency will require testing on real hardware.


## License

**No license has been selected yet.** Publishing source code does not automatically grant permission to reuse or redistribute it. A `LICENSE` file should be added before inviting external code reuse or contributions under an open-source license.

## Related projects

- [Octlitch Linux](https://octlitch.com) — the Linux distribution Anwilite is being developed for.
- [Smithay](https://github.com/Smithay/smithay) — Rust compositor building blocks.
- [Wayland](https://wayland.freedesktop.org/) — display and input protocol ecosystem.
- [Waybar](https://github.com/Alexays/Waybar) — current development panel.
- [Fuzzel](https://codeberg.org/dnkl/fuzzel) — current development launcher.
- [Swaybg](https://github.com/swaywm/swaybg) — current development wallpaper/background tool.

---

**Anwilite — building the Octlitch desktop from the compositor up.**
