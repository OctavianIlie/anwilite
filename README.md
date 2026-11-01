# Anwilite

**A Rust Wayland compositor and hybrid window manager for [Octlitch Linux](https://octlitch.com).**

> **Status: experimental / pre-alpha.** Anwilite runs as a nested Wayland compositor with working floating and tiled window management, configurable controls, and a basic desktop environment. It is not yet a standalone login session or a replacement for a production desktop, and gaming compatibility is still under development.

Anwilite is built with [Smithay](https://github.com/Smithay/smithay) (currently Smithay 0.7), Rust, Calloop, and OpenGL ES. It aims to provide a responsive, keyboard-driven desktop with flexible window management, a cohesive Octlitch visual identity, and a solid foundation for Linux gaming.

**Anwilite** is the compositor and window manager. **Octlitch Linux** is the distribution it is being developed for. The projects are related but independently maintainable.

## Project status

| Feature | Status |
| --- | --- |
| Nested Wayland compositor using Winit and GLES | Working |
| XDG toplevel windows, keyboard and pointer input | Working |
| Floating windows, dragging, interactive resizing, click-to-focus | Working |
| Ten workspaces, move-and-follow, workspace-local focus history | Working |
| Per-workspace floating or automatic tiling | Working |
| Master-and-stack tiling, gaps, adjustable ratios, tile navigation and swapping | Working |
| Snapping, maximizing, minimizing and restoring windows | Working |
| Focus borders and configurable window appearance | Working |
| TOML configuration, configurable application commands and keybindings | Working |
| Show desktop and application-launcher shortcuts | Working |
| Layer-shell surfaces, output information and layer input handling | Working in the nested backend |
| Waybar, Swaybg and Fuzzel integration | Working in the development session |
| Exclusive work areas for panels and docks | Implemented; manually tested with Waybar |
| Output resizing, damage tracking and redraw-on-demand | Working in the nested backend |
| True fullscreen distinct from maximize | Planned |
| Relative pointer input and pointer locking/confinement | Planned |
| XWayland, Steam and Proton workflows | Planned |
| Native DRM/KMS, libinput and independent login session | Planned |
| Multi-monitor management, variable refresh rate and direct scanout | Planned |

These results describe the current nested development environment. They are not a claim of broad hardware compatibility or finished production-quality behavior.

## Features

### Floating and tiled windows

Each of the ten workspaces can independently use floating or automatic master-and-stack tiling.

- **Floating:** drag, resize from window edges or corners, raise on click, snap to either half of the screen, maximize, restore, and minimize.
- **Tiled:** one window takes the usable area, two windows form master and stack columns, and additional windows stack vertically beside the master.
- **Configurable layout:** independent outer and inner gaps, per-workspace master-width overrides, and a configurable default master ratio. Master ratios can be adjusted between 20% and 80%.
- **Window ordering:** navigate between tiles, swap adjacent tiles, or promote a window to the master position without losing its original floating geometry.
- **Lifecycle handling:** recalculate placements when windows are opened, closed, minimized, restored, transferred between workspaces, or when the output changes size.

Tiling also checks client-reported size constraints. When a layout cannot accommodate its windows, Anwilite preserves the windows rather than deliberately forcing them into overlapping tile slots.

### Focus and window appearance

Anwilite draws active-window indicators with configurable colors, thickness, and spacing. Window focus follows the compositor's visible-window and layer-shell rules. Tiled and snapped windows use placement-aware clipping and borders.

Applications can draw their own window decorations. A client may choose to hide its titlebar buttons while maximized; Anwilite does not yet provide a complete compositor-drawn titlebar and button system.

### Panels, wallpaper and launcher

The development desktop currently uses:

- **Waybar** for a panel and clock.
- **Swaybg** for the background.
- **Fuzzel** as a keyboard-driven application launcher.

Anwilite supports `zwlr_layer_shell_v1` surfaces with the appropriate background, bottom, top and overlay stacking, as well as layer keyboard/pointer interactions. The virtual output is advertised as `ANWILITE-1` and includes XDG logical-output information for panel clients.

Panels can request **exclusive zones**. For example, a 36-pixel top Waybar on a 1280 × 800 output reserves the work area `(0, 36)` with size `1280 × 764`. Maximized, snapped and tiled windows use the remaining work area. If the panel exits, its reserved space is released; when it returns, window placement is recalculated. Background layers still cover the full output.

This is distinct from **true fullscreen**, which is a future feature intended to let games occupy the entire output independently of ordinary maximization.

### Configuration

The main configuration is located at:

```text
~/.config/anwilite/config.toml
```

It provides settings for tiling, gaps, master ratios, borders, application launch commands and keyboard bindings. Anwilite checks for configuration changes during the session and applies supported changes after validation. Some settings or active window transitions may require a restart or a later retry.

Keybindings are user-editable through `[[keybindings]]` entries. A fully specified keybinding list is authoritative: removing an entry removes that binding rather than silently restoring it. Reserved recovery bindings remain available for reloading configuration.

Waybar is configured separately through `~/.config/anwilite/waybar.jsonc` and `~/.config/anwilite/waybar.css`. For a panel that reserves its space, set `"exclusive": true` in the Waybar configuration.

## Keyboard shortcuts

The following are the familiar bindings used in the development setup. **Your `config.toml` is authoritative** if you have customized the keybinding list.

| Shortcut | Action |
| --- | --- |
| `Super + 1` … `Super + 9` | Switch to workspaces 1–9 |
| `Super + 0` | Switch to workspace 10 |
| `Super + Shift + 1` … `Super + Shift + 0` | Move the focused window to a workspace and follow it |
| `Super + Shift + T` | Toggle tiling for the current workspace |
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
│   ├── Compositor, XDG shell, seats, data device
│   ├── Virtual output and XDG output information
│   └── Layer shell (panels, overlays, backgrounds)
├── Window management
│   ├── Floating, snapping, maximize/minimize
│   ├── Workspaces, focus, tiling and layout constraints
│   └── Exclusive work areas and focus indicators
├── Configuration and application launching
├── Calloop event loop and Wayland socket
└── Winit + OpenGL ES (current nested backend)
    └── Native DRM/KMS and libinput (planned)
```

The main Rust modules include:

| File | Responsibility |
| --- | --- |
| `src/main.rs` | Compositor state, protocol handlers, input, event loop and rendering |
| `src/window.rs` | Managed windows, placement, focus, tiling and interactive operations |
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

The roadmap describes intended capabilities, not committed release dates. Features may change as the standalone compositor architecture develops.

### Desktop and application compatibility

- [x] Floating windows, snapping, workspaces and automatic tiling.
- [x] Configurable focus borders, gaps, master ratios, keybindings and application commands.
- [x] Waybar, Swaybg and Fuzzel support in a nested desktop session.
- [x] Layer-shell exclusive zones respected by placed application windows.
- [ ] True fullscreen handling, separate from maximize, with correct focus and stacking.
- [ ] Complete XDG popup handling, interactive grabs and transient-window behavior.
- [ ] More complete clipboard/drag-and-drop, primary selection, input-method and text-input support.
- [ ] Notifications, screencopy, portals and compositor IPC for external shell tools.
- [ ] Optional window-decoration policies and additional layout modes.

### Gaming and display performance

- [ ] XWayland compatibility for X11 applications and Steam/Proton game workflows.
- [ ] Relative pointer, cursor confinement/locking and reliable input capture.
- [ ] Native DRM/KMS, udev, libinput and seat/session integration.
- [ ] Display modes, refresh rates, scaling, multi-monitor hotplug and recovery.
- [ ] Better frame scheduling, presentation feedback and latency measurement.
- [ ] Variable refresh rate, explicit synchronization and direct scanout where supported.
- [ ] Gamescope interoperability and realistic GPU/game compatibility testing.

### Desktop experience and distribution

- [ ] Refined Octlitch themes, launcher, panel and coherent application styling.
- [ ] Desktop controls for volume, power, network, Bluetooth and notifications.
- [ ] Explore a dedicated programmable shell, including Quickshell/QML, without coupling it tightly to window-management internals.
- [ ] Repeatable CI, packaging, documentation and supported configuration presets.
- [ ] A standalone login-session entry and packaged integration for Octlitch Linux.
- [ ] Reliability, security and compatibility testing before any stable release.

## Design principles

Anwilite is its own Rust/Smithay compositor, not a fork of another window manager or a theme layered over KDE. Its priorities are predictable window behavior, user-configurable workflows, responsive input, readable code and a consistent desktop identity.

The compositor should remain usable for everyday work while allowing games to request uninterrupted fullscreen presentation and appropriate input handling. Visual effects should not take priority over frame consistency, and claims about gaming compatibility or low latency will require testing on real hardware.

## Contributing

Anwilite is under active early development. Reproducible bug reports, code review and design feedback are welcome. For technical issues, include your Rust version, Linux distribution, host compositor, GPU or virtual machine, steps to reproduce, and relevant Anwilite logs.

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
