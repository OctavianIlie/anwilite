use crate::layout::SnapSide;
use crate::window::{SpatialDirection, TileFocusDirection, WindowCycleDirection};
use crate::workspace::WORKSPACE_COUNT;
use serde::Deserialize;
use smithay::input::keyboard::{ModifiersState, keysyms};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceCommand {
    SwitchTo(usize),
    MoveAndFollow(usize),
    Snap(SnapSide),
    Maximize,
    Restore,
    ToggleTiling,
    ToggleGrid,
    ToggleTiledMode,
    ToggleGridMode,
    SetFloating,
    SetTiled,
    SetGrid,
    AdjustMasterRatio(i8),
    ResetMasterRatio,
    FocusTile(TileFocusDirection),
    FocusSpatial(SpatialDirection),
    CycleWindow(WindowCycleDirection),
    CloseWindow,
    SwapTile(TileFocusDirection),
    PromoteTile,
    Minimize,
    RestoreMinimized,
    ReloadConfig,
    LaunchTerminal,
    LaunchFileManager,
    LaunchMenu,
    ToggleShowDesktop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Shortcut {
    logo: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
    keysym: u32,
}

fn normalized_keysym(symbol: u32) -> u32 {
    if symbol == keysyms::KEY_ISO_Left_Tab {
        keysyms::KEY_Tab
    } else if (keysyms::KEY_A..=keysyms::KEY_Z).contains(&symbol) {
        symbol + (keysyms::KEY_a - keysyms::KEY_A)
    } else {
        symbol
    }
}

fn key_from_name(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();

    if lower.len() == 1 {
        let byte = lower.as_bytes()[0];

        if byte.is_ascii_alphanumeric() {
            return Some(u32::from(byte));
        }
    }

    match lower.as_str() {
        "return" | "enter" => Some(keysyms::KEY_Return),
        "kp_enter" => Some(keysyms::KEY_KP_Enter),
        "left" => Some(keysyms::KEY_Left),
        "right" => Some(keysyms::KEY_Right),
        "up" => Some(keysyms::KEY_Up),
        "down" => Some(keysyms::KEY_Down),
        "space" => Some(keysyms::KEY_space),
        "tab" => Some(keysyms::KEY_Tab),
        _ => None,
    }
}

impl Shortcut {
    fn parse(source: &str) -> Result<Self, String> {
        let mut shortcut = Self {
            logo: false,
            ctrl: false,
            alt: false,
            shift: false,
            keysym: 0,
        };

        let mut has_key = false;

        for part in source.split('+') {
            let name = part.trim().to_ascii_lowercase();

            let modifier = match name.as_str() {
                "super" | "logo" => Some(&mut shortcut.logo),
                "ctrl" | "control" => Some(&mut shortcut.ctrl),
                "alt" => Some(&mut shortcut.alt),
                "shift" => Some(&mut shortcut.shift),
                _ => None,
            };

            if let Some(flag) = modifier {
                if *flag {
                    return Err(format!("duplicate modifier in '{source}'"));
                }

                *flag = true;
                continue;
            }

            if has_key {
                return Err(format!("multiple keys in '{source}'"));
            }

            shortcut.keysym =
                key_from_name(&name).ok_or_else(|| format!("unknown key '{part}'"))?;

            has_key = true;
        }

        if !has_key {
            return Err(format!("missing key in '{source}'"));
        }

        if !(shortcut.logo || shortcut.ctrl || shortcut.alt) {
            return Err(format!("'{source}' needs Super, Ctrl, or Alt"));
        }

        Ok(shortcut)
    }

    fn from_event(mods: &ModifiersState, keysym: u32) -> Self {
        Self {
            logo: mods.logo,
            ctrl: mods.ctrl,
            alt: mods.alt,
            shift: mods.shift,
            keysym: normalized_keysym(keysym),
        }
    }

    fn is_recovery_reload(&self) -> bool {
        self.keysym == keysyms::KEY_c
            && self.shift
            && ((self.logo && !self.ctrl && !self.alt) || (!self.logo && self.ctrl && self.alt))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingSpec {
    pub key: String,
    pub action: String,
    pub workspace: Option<usize>,
}

fn parse_action(spec: &BindingSpec) -> Result<WorkspaceCommand, String> {
    use WorkspaceCommand as Command;

    let action = spec.action.to_ascii_lowercase();

    match (action.as_str(), spec.workspace) {
        ("switch_workspace", Some(n)) if (1..=WORKSPACE_COUNT).contains(&n) => {
            Ok(Command::SwitchTo(n))
        }

        ("move_and_follow", Some(n)) if (1..=WORKSPACE_COUNT).contains(&n) => {
            Ok(Command::MoveAndFollow(n))
        }

        ("switch_workspace" | "move_and_follow", _) => Err(format!(
            "'{}' requires workspace = 1..={WORKSPACE_COUNT}",
            spec.action
        )),

        (_, Some(_)) => Err(format!(
            "'{}' does not accept a workspace number",
            spec.action
        )),

        ("launch_terminal", None) => Ok(Command::LaunchTerminal),
        ("launch_file_manager", None) => Ok(Command::LaunchFileManager),
        ("toggle_tiling", None) => Ok(Command::ToggleTiling),
        ("toggle_grid", None) => Ok(Command::ToggleGrid),
        ("toggle_tiled_mode", None) => Ok(Command::ToggleTiledMode),
        ("toggle_grid_mode", None) => Ok(Command::ToggleGridMode),
        ("set_floating", None) => Ok(Command::SetFloating),
        ("set_tiled", None) => Ok(Command::SetTiled),
        ("set_grid", None) => Ok(Command::SetGrid),
        ("minimize", None) => Ok(Command::Minimize),
        ("restore_minimized", None) => Ok(Command::RestoreMinimized),
        ("maximize", None) => Ok(Command::Maximize),
        ("restore", None) => Ok(Command::Restore),
        ("reload_config", None) => Ok(Command::ReloadConfig),

        ("decrease_master_ratio", None) => Ok(Command::AdjustMasterRatio(-5)),
        ("increase_master_ratio", None) => Ok(Command::AdjustMasterRatio(5)),
        ("reset_master_ratio", None) => Ok(Command::ResetMasterRatio),

        ("focus_previous_tile", None) => Ok(Command::FocusTile(TileFocusDirection::Previous)),
        ("focus_next_tile", None) => Ok(Command::FocusTile(TileFocusDirection::Next)),
        ("swap_previous_tile", None) => Ok(Command::SwapTile(TileFocusDirection::Previous)),
        ("swap_next_tile", None) => Ok(Command::SwapTile(TileFocusDirection::Next)),
        ("promote_tile", None) => Ok(Command::PromoteTile),
        ("focus_left", None) => Ok(Command::FocusSpatial(SpatialDirection::Left)),
        ("focus_right", None) => Ok(Command::FocusSpatial(SpatialDirection::Right)),
        ("focus_up", None) => Ok(Command::FocusSpatial(SpatialDirection::Up)),
        ("focus_down", None) => Ok(Command::FocusSpatial(SpatialDirection::Down)),
        ("cycle_windows", None) => Ok(Command::CycleWindow(WindowCycleDirection::Next)),
        ("cycle_windows_reverse", None) => Ok(Command::CycleWindow(WindowCycleDirection::Previous)),
        ("close_window", None) => Ok(Command::CloseWindow),
        ("snap_left", None) => Ok(Command::Snap(SnapSide::Left)),
        ("snap_right", None) => Ok(Command::Snap(SnapSide::Right)),
        ("snap_top_left", None) => Ok(Command::Snap(SnapSide::TopLeft)),
        ("snap_top_right", None) => Ok(Command::Snap(SnapSide::TopRight)),
        ("snap_bottom_left", None) => Ok(Command::Snap(SnapSide::BottomLeft)),
        ("snap_bottom_right", None) => Ok(Command::Snap(SnapSide::BottomRight)),

        ("launch_menu", None) => Ok(Command::LaunchMenu),
        ("toggle_show_desktop", None) => Ok(Command::ToggleShowDesktop),

        _ => Err(format!("unknown keybinding action '{}'", spec.action)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    shortcut: Shortcut,
    action: WorkspaceCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBindings {
    bindings: Vec<Binding>,
}

impl Default for KeyBindings {
    fn default() -> Self {
        use WorkspaceCommand as Command;

        let mut bindings = Vec::new();

        // Each built-in shortcut works with Super and Ctrl+Alt.
        let defaults = [
            ("M", Command::Minimize),
            ("Shift+M", Command::RestoreMinimized),
            ("Shift+F", Command::SetFloating),
            ("Shift+T", Command::ToggleTiledMode),
            ("Shift+G", Command::ToggleGridMode),
            ("Shift+H", Command::FocusTile(TileFocusDirection::Previous)),
            ("Shift+J", Command::FocusTile(TileFocusDirection::Next)),
            (
                "Shift+Left",
                Command::SwapTile(TileFocusDirection::Previous),
            ),
            ("Shift+Right", Command::SwapTile(TileFocusDirection::Next)),
            ("Shift+P", Command::PromoteTile),
            ("H", Command::AdjustMasterRatio(-5)),
            ("J", Command::AdjustMasterRatio(5)),
            ("R", Command::ResetMasterRatio),
            ("Left", Command::Snap(SnapSide::Left)),
            ("Right", Command::Snap(SnapSide::Right)),
            ("Up", Command::Maximize),
            ("Down", Command::Restore),
            ("Return", Command::LaunchTerminal),
            ("KP_Enter", Command::LaunchTerminal),
            ("E", Command::LaunchFileManager),
            ("Space", Command::LaunchMenu),
            ("D", Command::ToggleShowDesktop),
            ("Shift+Q", Command::CloseWindow),
        ];

        for (key, action) in defaults {
            for prefix in ["Super+", "Ctrl+Alt+"] {
                let shortcut =
                    Shortcut::parse(&format!("{prefix}{key}")).expect("valid built-in shortcut");

                bindings.push(Binding { shortcut, action });
            }
        }

        // Directional focus is separate from snapping and tile swapping.
        for (key, direction, fallback) in [
            ("Left", SpatialDirection::Left, "A"),
            ("Right", SpatialDirection::Right, "D"),
            ("Up", SpatialDirection::Up, "W"),
            ("Down", SpatialDirection::Down, "S"),
        ] {
            for shortcut_name in [
                format!("Super+Alt+{key}"),
                format!("Ctrl+Alt+Shift+{fallback}"),
            ] {
                bindings.push(Binding {
                    shortcut: Shortcut::parse(&shortcut_name).expect("valid spatial shortcut"),
                    action: Command::FocusSpatial(direction),
                });
            }
        }

        // Alt+Tab cycles through the global focus history. Super+Tab and
        // Ctrl+Alt+Tab are useful fallbacks in nested sessions.
        for (key, direction) in [
            ("Alt+Tab", WindowCycleDirection::Next),
            ("Alt+Shift+Tab", WindowCycleDirection::Previous),
            ("Super+Tab", WindowCycleDirection::Next),
            ("Super+Shift+Tab", WindowCycleDirection::Previous),
            ("Ctrl+Alt+Tab", WindowCycleDirection::Next),
            ("Ctrl+Alt+Shift+Tab", WindowCycleDirection::Previous),
        ] {
            bindings.push(Binding {
                shortcut: Shortcut::parse(key).expect("valid window cycle shortcut"),
                action: Command::CycleWindow(direction),
            });
        }

        // Workspace shortcuts 1..9 and 0 (workspace 10).
        for workspace in 1..=WORKSPACE_COUNT {
            let key = if workspace == 10 {
                "0".to_string()
            } else {
                workspace.to_string()
            };

            for prefix in ["Super+", "Ctrl+Alt+"] {
                for (extra, action) in [
                    ("", Command::SwitchTo(workspace)),
                    ("Shift+", Command::MoveAndFollow(workspace)),
                ] {
                    let shortcut = Shortcut::parse(&format!("{prefix}{extra}{key}"))
                        .expect("valid workspace shortcut");

                    bindings.push(Binding { shortcut, action });
                }
            }
        }

        Self { bindings }
    }
}

impl KeyBindings {
    pub fn from_explicit(specs: &[BindingSpec]) -> Result<Self, String> {
        let mut bindings = Vec::with_capacity(specs.len());
        let mut used = HashSet::new();

        for spec in specs {
            let shortcut = Shortcut::parse(&spec.key)?;
            let action = parse_action(spec)?;

            if shortcut.is_recovery_reload() {
                return Err(format!(
                    "'{}' is reserved for configuration recovery",
                    spec.key
                ));
            }

            if !used.insert(shortcut) {
                return Err(format!("duplicate keybinding: '{}'", spec.key));
            }

            bindings.push(Binding { shortcut, action });
        }

        Ok(Self { bindings })
    }
    #[cfg(test)]
    pub fn with_overrides(specs: &[BindingSpec]) -> Result<Self, String> {
        let mut custom = Vec::new();

        // Parse all overrides before modifying the defaults.
        for spec in specs {
            let shortcut = Shortcut::parse(&spec.key)?;
            let action = parse_action(spec)?;

            if shortcut.is_recovery_reload() {
                return Err(format!(
                    "'{}' is reserved for emergency configuration reload",
                    spec.key
                ));
            }

            custom.push(Binding { shortcut, action });
        }

        let mut result = Self::default();

        // If an action has a custom binding, remove that
        // action's built-in shortcuts.
        result
            .bindings
            .retain(|original| !custom.iter().any(|new| new.action == original.action));

        // Catch collisions with remaining defaults and other overrides.
        let mut used: HashSet<Shortcut> = result
            .bindings
            .iter()
            .map(|binding| binding.shortcut)
            .collect();

        for binding in custom {
            if !used.insert(binding.shortcut) {
                return Err(format!(
                    "duplicate or conflicting keybinding: {:?}",
                    binding.shortcut
                ));
            }

            result.bindings.push(binding);
        }

        Ok(result)
    }

    fn action_for(&self, shortcut: Shortcut) -> Option<WorkspaceCommand> {
        // These two shortcuts cannot be disabled or overridden.
        if shortcut.is_recovery_reload() {
            return Some(WorkspaceCommand::ReloadConfig);
        }

        self.bindings
            .iter()
            .find(|binding| binding.shortcut == shortcut)
            .map(|binding| binding.action)
    }

    pub fn match_event(&self, modifiers: &ModifiersState, keysym: u32) -> Option<WorkspaceCommand> {
        self.action_for(Shortcut::from_event(modifiers, keysym))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(key: &str, action: &str, workspace: Option<usize>) -> BindingSpec {
        BindingSpec {
            key: key.to_string(),
            action: action.to_string(),
            workspace,
        }
    }

    #[test]
    fn defaults_preserve_terminal_shortcuts() {
        let bindings = KeyBindings::default();

        assert_eq!(
            bindings.action_for(Shortcut::parse("Super+Return").unwrap()),
            Some(WorkspaceCommand::LaunchTerminal)
        );

        assert_eq!(
            bindings.action_for(Shortcut::parse("Ctrl+Alt+Return").unwrap()),
            Some(WorkspaceCommand::LaunchTerminal)
        );
    }

    #[test]
    fn custom_binding_replaces_action_defaults() {
        let bindings = KeyBindings::with_overrides(&[
            spec("Super+Y", "launch_terminal", None),
            spec("Ctrl+Alt+Y", "launch_terminal", None),
        ])
        .unwrap();

        assert_eq!(
            bindings.action_for(Shortcut::parse("Super+Y").unwrap()),
            Some(WorkspaceCommand::LaunchTerminal)
        );

        assert_eq!(
            bindings.action_for(Shortcut::parse("Super+Return").unwrap()),
            None
        );
    }

    #[test]
    fn collisions_with_existing_defaults_are_rejected() {
        let result = KeyBindings::with_overrides(&[spec("Super+E", "launch_terminal", None)]);

        assert!(result.is_err());
    }

    #[test]
    fn duplicate_custom_shortcuts_are_rejected() {
        let result = KeyBindings::with_overrides(&[
            spec("Super+Y", "launch_terminal", None),
            spec("Super+Y", "launch_file_manager", None),
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn recovery_shortcuts_cannot_be_overridden() {
        let result = KeyBindings::with_overrides(&[spec("Super+Shift+C", "launch_terminal", None)]);

        assert!(result.is_err());

        let defaults = KeyBindings::default();

        assert_eq!(
            defaults.action_for(Shortcut::parse("Ctrl+Alt+Shift+C").unwrap()),
            Some(WorkspaceCommand::ReloadConfig)
        );
    }

    #[test]
    fn workspace_actions_validate_numbers() {
        let bindings =
            KeyBindings::with_overrides(&[spec("Super+F", "switch_workspace", Some(2))]).unwrap();

        assert_eq!(
            bindings.action_for(Shortcut::parse("Super+F").unwrap()),
            Some(WorkspaceCommand::SwitchTo(2))
        );

        assert!(
            KeyBindings::with_overrides(&[spec("Super+F", "switch_workspace", Some(11)),]).is_err()
        );
    }

    #[test]
    fn invalid_keys_and_actions_are_rejected() {
        assert!(Shortcut::parse("Super+Banana").is_err());
        assert!(Shortcut::parse("Super+Super+Y").is_err());
        assert!(Shortcut::parse("Y").is_err());

        assert!(
            KeyBindings::with_overrides(&[spec("Super+Y", "nonexistent_action", None),]).is_err()
        );
    }

    #[test]
    fn super_space_is_a_valid_shortcut() {
        let shortcut = Shortcut::parse("Super+Space").unwrap();

        assert_eq!(shortcut.keysym, keysyms::KEY_space);
        assert!(shortcut.logo);
    }

    #[test]
    fn explicit_bindings_do_not_restore_unlisted_defaults() {
        let bindings = KeyBindings::from_explicit(&[
            spec("Super+D", "toggle_show_desktop", None),
            spec("Super+Space", "launch_menu", None),
        ])
        .unwrap();

        assert_eq!(
            bindings.action_for(Shortcut::parse("Super+D").unwrap()),
            Some(WorkspaceCommand::ToggleShowDesktop)
        );

        assert_eq!(
            bindings.action_for(Shortcut::parse("Super+Space").unwrap()),
            Some(WorkspaceCommand::LaunchMenu)
        );

        // Not in the list: no implicit shortcut.
        assert_eq!(
            bindings.action_for(Shortcut::parse("Super+Return").unwrap()),
            None
        );

        // Recovery remains available.
        assert_eq!(
            bindings.action_for(Shortcut::parse("Super+Shift+C").unwrap()),
            Some(WorkspaceCommand::ReloadConfig)
        );
    }

    #[test]
    fn quarter_snap_actions_can_be_bound_explicitly() {
        let placements = [
            ("Super+Ctrl+7", "snap_top_left", SnapSide::TopLeft),
            ("Super+Ctrl+9", "snap_top_right", SnapSide::TopRight),
            ("Super+Ctrl+1", "snap_bottom_left", SnapSide::BottomLeft),
            ("Super+Ctrl+3", "snap_bottom_right", SnapSide::BottomRight),
        ];

        let specs: Vec<_> = placements
            .iter()
            .map(|(key, action, _)| spec(key, action, None))
            .collect();
        let bindings = KeyBindings::from_explicit(&specs).unwrap();

        for (key, _, side) in placements {
            assert_eq!(
                bindings.action_for(Shortcut::parse(key).unwrap()),
                Some(WorkspaceCommand::Snap(side))
            );
        }

        // Explicit bindings must not silently enable unrelated defaults.
        assert_eq!(
            bindings.action_for(Shortcut::parse("Super+Left").unwrap()),
            None
        );
        assert_eq!(
            bindings.action_for(Shortcut::parse("Super+Return").unwrap()),
            None
        );

        // No implicit quarter bindings were added to the built-in defaults.
        assert_eq!(
            KeyBindings::default().action_for(Shortcut::parse("Super+Ctrl+7").unwrap()),
            None
        );
    }

    #[test]
    fn quarter_snap_actions_reject_workspace_numbers() {
        for action in [
            "snap_top_left",
            "snap_top_right",
            "snap_bottom_left",
            "snap_bottom_right",
        ] {
            assert!(KeyBindings::from_explicit(&[spec("Super+Ctrl+7", action, Some(1))]).is_err());
        }
    }

    #[test]
    fn navigation_commands_parse_and_match_explicit_bindings() {
        let bindings = KeyBindings::from_explicit(&[
            spec("Super+Alt+Left", "focus_left", None),
            spec("Super+Alt+Right", "focus_right", None),
            spec("Super+Alt+Up", "focus_up", None),
            spec("Super+Alt+Down", "focus_down", None),
            spec("Alt+Tab", "cycle_windows", None),
            spec("Alt+Shift+Tab", "cycle_windows_reverse", None),
            spec("Super+Shift+Q", "close_window", None),
        ])
        .unwrap();
        for (key, expected) in [
            (
                "Super+Alt+Left",
                WorkspaceCommand::FocusSpatial(SpatialDirection::Left),
            ),
            (
                "Super+Alt+Right",
                WorkspaceCommand::FocusSpatial(SpatialDirection::Right),
            ),
            (
                "Super+Alt+Up",
                WorkspaceCommand::FocusSpatial(SpatialDirection::Up),
            ),
            (
                "Super+Alt+Down",
                WorkspaceCommand::FocusSpatial(SpatialDirection::Down),
            ),
            (
                "Alt+Tab",
                WorkspaceCommand::CycleWindow(WindowCycleDirection::Next),
            ),
            (
                "Alt+Shift+Tab",
                WorkspaceCommand::CycleWindow(WindowCycleDirection::Previous),
            ),
            ("Super+Shift+Q", WorkspaceCommand::CloseWindow),
        ] {
            assert_eq!(
                bindings.action_for(Shortcut::parse(key).unwrap()),
                Some(expected)
            );
        }
        // With an explicit list there are no surprise defaults.
        assert_eq!(
            bindings.action_for(Shortcut::parse("Super+Return").unwrap()),
            None
        );
        assert_eq!(
            normalized_keysym(keysyms::KEY_ISO_Left_Tab),
            keysyms::KEY_Tab
        );
        assert!(KeyBindings::from_explicit(&[spec("Super+Y", "focus_up", Some(2))]).is_err());
    }

    #[test]
    fn explicit_bindings_reject_duplicates() {
        assert!(
            KeyBindings::from_explicit(&[
                spec("Super+D", "toggle_show_desktop", None),
                spec("Super+D", "launch_terminal", None),
            ])
            .is_err()
        );

        assert!(
            KeyBindings::from_explicit(&[spec("Super+Shift+C", "launch_menu", None),]).is_err()
        );
    }
}

#[cfg(test)]
mod grid_keybinding_tests {
    use super::*;
    #[test]
    fn grid_direct_shortcuts_and_legacy_toggle_both_work() {
        for name in ["Super+Shift+G", "Ctrl+Alt+Shift+G"] {
            assert_eq!(
                KeyBindings::default().action_for(Shortcut::parse(name).unwrap()),
                Some(WorkspaceCommand::ToggleGridMode)
            );
            let explicit = KeyBindings::from_explicit(&[BindingSpec {
                key: name.into(),
                action: "toggle_grid".into(),
                workspace: None,
            }])
            .unwrap();
            assert_eq!(
                explicit.action_for(Shortcut::parse(name).unwrap()),
                Some(WorkspaceCommand::ToggleGrid)
            );
        }
        assert!(
            KeyBindings::from_explicit(&[BindingSpec {
                key: "Super+Shift+G".into(),
                action: "toggle_grid".into(),
                workspace: Some(2),
            }])
            .is_err()
        );
    }
}

#[cfg(test)]
mod desktop_mode_keybinding_tests {
    use super::*;

    #[test]
    fn builtins_toggle_tiling_and_grid_against_windowed() {
        for (key, expected) in [
            ("Shift+F", WorkspaceCommand::SetFloating),
            ("Shift+T", WorkspaceCommand::ToggleTiledMode),
            ("Shift+G", WorkspaceCommand::ToggleGridMode),
        ] {
            for prefix in ["Super+", "Ctrl+Alt+"] {
                let shortcut = Shortcut::parse(&format!("{prefix}{key}")).unwrap();
                assert_eq!(KeyBindings::default().action_for(shortcut), Some(expected));
            }
        }
    }

    #[test]
    fn explicit_toggle_actions_are_supported_and_reject_workspace_numbers() {
        for (action, expected) in [
            ("toggle_tiled_mode", WorkspaceCommand::ToggleTiledMode),
            ("toggle_grid_mode", WorkspaceCommand::ToggleGridMode),
        ] {
            let binding = BindingSpec {
                key: "Super+Y".into(),
                action: action.into(),
                workspace: None,
            };
            let bindings = KeyBindings::from_explicit(&[binding]).unwrap();
            assert_eq!(
                bindings.action_for(Shortcut::parse("Super+Y").unwrap()),
                Some(expected)
            );
            let invalid = BindingSpec {
                key: "Super+Y".into(),
                action: action.into(),
                workspace: Some(1),
            };
            assert!(KeyBindings::from_explicit(&[invalid]).is_err());
        }
    }

    #[test]
    fn explicit_mode_actions_are_supported_and_reject_workspace_numbers() {
        for action in ["set_floating", "set_tiled", "set_grid"] {
            let binding = BindingSpec {
                key: "Super+Y".into(),
                action: action.into(),
                workspace: None,
            };
            assert!(KeyBindings::from_explicit(&[binding]).is_ok());
            let invalid = BindingSpec {
                key: "Super+Y".into(),
                action: action.into(),
                workspace: Some(1),
            };
            assert!(KeyBindings::from_explicit(&[invalid]).is_err());
        }
    }
}
