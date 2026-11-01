use crate::appearance::{ACTIVE_BORDER, BorderStyle, INACTIVE_BORDER};
use crate::keybindings::{BindingSpec, KeyBindings};
use crate::layout::TilingSettings;
use crate::workspace::WORKSPACE_COUNT;
use serde::Deserialize;
use std::{env, fs, io, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppCommands {
    pub terminal: Vec<String>,
    pub file_manager: Vec<String>,
    pub autostart_terminal: bool,
    pub menu: Vec<String>,
}

impl Default for AppCommands {
    fn default() -> Self {
        Self {
            terminal: vec!["weston-terminal".to_string()],
            file_manager: Vec::new(),
            autostart_terminal: true,
            menu: Vec::new(),
        }
    }
}

// A rule applies only when a NEW XDG toplevel identifies itself. Rules do not
// retroactively move windows when configuration is reloaded.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowRule {
    pub app_id: String,
    pub workspace: usize,
}

pub fn matching_workspace(rules: &[WindowRule], app_id: &str) -> Option<usize> {
    rules
        .iter()
        .find(|rule| rule.app_id == app_id)
        .map(|rule| rule.workspace)
}

#[derive(Debug, Clone)]
pub struct AnwiliteConfig {
    pub active_border: BorderStyle,
    pub inactive_border: Option<BorderStyle>,
    pub tiling: TilingSettings,
    pub apps: AppCommands,
    pub window_rules: Vec<WindowRule>,
    pub keybindings: KeyBindings,
}

impl Default for AnwiliteConfig {
    fn default() -> Self {
        Self {
            active_border: ACTIVE_BORDER,
            inactive_border: INACTIVE_BORDER,
            tiling: TilingSettings::default(),
            apps: AppCommands::default(),
            window_rules: Vec::new(),
            keybindings: KeyBindings::default(),
        }
    }
}

impl AnwiliteConfig {
    pub fn apply_reload(&mut self, candidate: Result<Self, String>) -> Result<bool, String> {
        let next = candidate?;

        if self.active_border == next.active_border
            && self.inactive_border == next.inactive_border
            && self.tiling == next.tiling
            && self.apps == next.apps
            && self.window_rules == next.window_rules
            && self.keybindings == next.keybindings
        {
            return Ok(false);
        }

        *self = next;

        Ok(true)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    borders: BorderOptions,
    tiling: TilingOptions,
    apps: AppOptions,
    window_rules: Vec<WindowRule>,
    keybindings: Option<Vec<BindingSpec>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct BorderOptions {
    active_color: Option<String>,
    inactive_color: Option<String>,

    active_thickness: Option<i32>,
    inactive_thickness: Option<i32>,

    active_clearance: Option<i32>,
    inactive_clearance: Option<i32>,

    inactive_enabled: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TilingOptions {
    outer_gap: Option<i32>,
    inner_gap: Option<i32>,
    default_master_ratio: Option<u8>,
    start_tiled: Option<bool>,
    tiled_workspaces: Option<Vec<usize>>,
    grid_workspaces: Option<Vec<usize>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AppOptions {
    terminal: Option<Vec<String>>,
    file_manager: Option<Vec<String>>,
    autostart_terminal: Option<bool>,
    menu: Option<Vec<String>>,
}

fn parse_hex_color(raw: &str) -> Result<[f32; 4], String> {
    let hex = raw
        .strip_prefix('#')
        .ok_or_else(|| format!("color '{raw}' must start with #"))?;

    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("invalid color '{raw}'; use #RRGGBB or #RRGGBBAA"));
    }

    let mut rgba = [1.0; 4];

    for (i, channel) in rgba.iter_mut().enumerate().take(hex.len() / 2) {
        let start = i * 2;
        let component = &hex[start..start + 2];

        let value =
            u8::from_str_radix(component, 16).map_err(|_| format!("invalid color '{raw}'"))?;

        *channel = f32::from(value) / 255.0;
    }

    Ok(rgba)
}

fn parse_config(contents: &str) -> Result<AnwiliteConfig, String> {
    let file: FileConfig = toml::from_str(contents).map_err(|error| error.to_string())?;

    let options = file.borders;
    let mut config = AnwiliteConfig::default();

    // Active border
    if let Some(value) = options.active_thickness {
        config.active_border.thickness = value;
    }

    if let Some(value) = options.active_clearance {
        config.active_border.clearance = value;
    }

    if let Some(color) = options.active_color {
        config.active_border.rgba = parse_hex_color(&color)?;
    }

    if !config.active_border.is_valid() {
        return Err("invalid active border thickness or clearance".into());
    }

    // Inactive border
    let mut inactive = INACTIVE_BORDER.unwrap_or(BorderStyle {
        thickness: 1,
        clearance: 0,
        rgba: [0.30, 0.34, 0.40, 1.0],
    });

    if let Some(value) = options.inactive_thickness {
        inactive.thickness = value;
    }

    if let Some(value) = options.inactive_clearance {
        inactive.clearance = value;
    }

    if let Some(color) = options.inactive_color {
        inactive.rgba = parse_hex_color(&color)?;
    }

    if !inactive.is_valid() {
        return Err("invalid inactive border thickness or clearance".into());
    }

    let enabled = options
        .inactive_enabled
        .unwrap_or(INACTIVE_BORDER.is_some());

    config.inactive_border = if enabled { Some(inactive) } else { None };

    let options = file.tiling;
    let mut tiling = TilingSettings::default();

    if let Some(value) = options.outer_gap {
        tiling.gaps.outer = value;
    }

    if let Some(value) = options.inner_gap {
        tiling.gaps.inner = value;
    }

    if let Some(value) = options.default_master_ratio {
        tiling.default_master_ratio = value;
    }

    if let Some(value) = options.start_tiled {
        tiling.start_tiled = value;
    }
    if let Some(workspaces) = options.tiled_workspaces {
        let mut mask = [false; WORKSPACE_COUNT];

        for workspace in workspaces {
            if !(1..=WORKSPACE_COUNT).contains(&workspace) {
                return Err(format!(
                    "invalid workspace {workspace}; expected 1..={WORKSPACE_COUNT}"
                ));
            }

            let index = workspace - 1;

            if mask[index] {
                return Err(format!("workspace {workspace} appears more than once"));
            }

            mask[index] = true;
        }

        tiling.tiled_workspaces = Some(mask);
    }
    if let Some(workspaces) = options.grid_workspaces {
        let mut mask = [false; WORKSPACE_COUNT];
        for workspace in workspaces {
            if !(1..=WORKSPACE_COUNT).contains(&workspace) {
                return Err(format!(
                    "invalid grid workspace {workspace}; expected 1..={WORKSPACE_COUNT}"
                ));
            }
            if mask[workspace - 1] {
                return Err(format!("grid workspace {workspace} appears more than once"));
            }
            mask[workspace - 1] = true;
        }
        tiling.grid_workspaces = Some(mask);
    }

    if !tiling.is_valid() {
        return Err("invalid tiling settings: gaps must be 0..=64 \
            and master ratio must be 20..=80"
            .into());
    }

    config.tiling = tiling;
    let options = file.apps;
    let mut apps = AppCommands::default();

    if let Some(command) = options.terminal {
        apps.terminal = command;
    }

    if let Some(command) = options.file_manager {
        apps.file_manager = command;
    }

    if let Some(command) = options.menu {
        apps.menu = command;
    }

    if let Some(enabled) = options.autostart_terminal {
        apps.autostart_terminal = enabled;
    }

    // An empty vector disables a command.
    // A nonempty vector must specify a real program name.
    for (name, command) in [
        ("terminal", &apps.terminal),
        ("file_manager", &apps.file_manager),
        ("menu", &apps.menu),
    ] {
        if command
            .first()
            .is_some_and(|program| program.trim().is_empty())
        {
            return Err(format!(
                "apps.{name} must begin with a nonempty executable name"
            ));
        }
    }

    config.apps = apps;

    // Reject ambiguous rules rather than relying on declaration order.
    let mut seen = std::collections::HashSet::new();
    for rule in &file.window_rules {
        if rule.app_id.trim().is_empty() || rule.app_id != rule.app_id.trim() {
            return Err("window_rules.app_id must be a nonempty exact application ID".into());
        }
        if !(1..=WORKSPACE_COUNT).contains(&rule.workspace) {
            return Err(format!(
                "window rule for '{}': workspace must be 1..={WORKSPACE_COUNT}",
                rule.app_id
            ));
        }
        if !seen.insert(rule.app_id.as_str()) {
            return Err(format!("duplicate window rule for '{}'", rule.app_id));
        }
    }
    config.window_rules = file.window_rules;

    config.keybindings = match file.keybindings {
        Some(specs) => KeyBindings::from_explicit(&specs)?,
        None => KeyBindings::default(),
    };

    Ok(config)
}

fn config_path() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("XDG_CONFIG_HOME") {
        let path = PathBuf::from(dir);

        if path.is_absolute() {
            return Some(path.join("anwilite/config.toml"));
        }
    }

    env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/anwilite/config.toml"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigFileSnapshot {
    Missing,
    Contents(Vec<u8>),
    ReadError(io::ErrorKind),
}

pub fn snapshot() -> ConfigFileSnapshot {
    let Some(path) = config_path() else {
        return ConfigFileSnapshot::Missing;
    };

    match fs::read(path) {
        Ok(contents) => ConfigFileSnapshot::Contents(contents),

        Err(error) if error.kind() == io::ErrorKind::NotFound => ConfigFileSnapshot::Missing,

        Err(error) => ConfigFileSnapshot::ReadError(error.kind()),
    }
}

pub fn snapshot_changed(previous: &mut ConfigFileSnapshot, current: ConfigFileSnapshot) -> bool {
    if *previous == current {
        return false;
    }

    *previous = current;
    true
}

pub fn try_load() -> Result<AnwiliteConfig, String> {
    let Some(path) = config_path() else {
        return Ok(AnwiliteConfig::default());
    };

    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,

        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(AnwiliteConfig::default());
        }

        Err(error) => {
            return Err(format!("cannot read {}: {error}", path.display()));
        }
    };

    parse_config(&contents)
        .map_err(|error| format!("invalid configuration at {}: {error}", path.display()))
}

pub fn load() -> AnwiliteConfig {
    let Some(path) = config_path() else {
        eprintln!("Anwilite: no config directory found; using defaults");
        return AnwiliteConfig::default();
    };

    match fs::read_to_string(&path) {
        Ok(contents) => match parse_config(&contents) {
            Ok(config) => {
                println!("Anwilite: loaded configuration from {}", path.display());
                config
            }

            Err(error) => {
                eprintln!(
                    "Anwilite: invalid configuration at {}: {error}; using defaults",
                    path.display()
                );

                AnwiliteConfig::default()
            }
        },

        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            println!("Anwilite: no config file found; using defaults");
            AnwiliteConfig::default()
        }

        Err(error) => {
            eprintln!(
                "Anwilite: cannot read {}: {error}; using defaults",
                path.display()
            );

            AnwiliteConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_uses_defaults() {
        let config = parse_config("").unwrap();

        assert_eq!(config.active_border, ACTIVE_BORDER);
        assert_eq!(config.inactive_border, INACTIVE_BORDER);
    }

    #[test]
    fn partial_config_preserves_other_defaults() {
        let config = parse_config("[borders]\nactive_thickness = 4\n").unwrap();

        assert_eq!(config.active_border.thickness, 4);
        assert_eq!(config.active_border.clearance, ACTIVE_BORDER.clearance);
        assert_eq!(config.inactive_border, INACTIVE_BORDER);
    }

    #[test]
    fn inactive_border_can_be_disabled() {
        let config = parse_config("[borders]\ninactive_enabled = false\n").unwrap();

        assert!(config.inactive_border.is_none());
    }

    #[test]
    fn invalid_thickness_and_color_are_rejected() {
        assert!(parse_config("[borders]\nactive_thickness = 0\n").is_err());

        assert!(parse_config("[borders]\nactive_color = \"not-a-color\"\n").is_err());
    }

    #[test]
    fn unknown_settings_are_rejected() {
        assert!(parse_config("[borders]\nactive_thikness = 3\n").is_err());
    }

    #[test]
    fn hex_color_supports_alpha() {
        assert_eq!(
            parse_hex_color("#FF000080").unwrap(),
            [1.0, 0.0, 0.0, 128.0 / 255.0]
        );
    }

    #[test]
    fn reload_only_applies_changed_settings() {
        let mut config = AnwiliteConfig::default();

        // Identical settings should not count as a change.
        assert!(!config.apply_reload(Ok(config.clone())).unwrap());

        let updated = parse_config("[borders]\nactive_thickness = 4\n").unwrap();

        assert!(config.apply_reload(Ok(updated.clone())).unwrap());

        assert_eq!(config.active_border.thickness, 4);

        // Reloading the same settings should do nothing.
        assert!(!config.apply_reload(Ok(updated)).unwrap());
    }

    #[test]
    fn invalid_reload_preserves_current_settings() {
        let mut config = parse_config("[borders]\nactive_thickness = 4\n").unwrap();

        let previous = config.clone();

        let invalid = parse_config("[borders]\nactive_color = \"invalid\"\n");

        assert!(config.apply_reload(invalid).is_err());

        assert_eq!(config.active_border, previous.active_border);
        assert_eq!(config.inactive_border, previous.inactive_border);
    }

    #[test]
    fn snapshot_detects_edits_and_deletion() {
        let original = ConfigFileSnapshot::Contents(b"color=teal".to_vec());

        let mut previous = original.clone();

        // Identical contents should not trigger a reload.
        assert!(!snapshot_changed(&mut previous, original));

        // Changed contents should trigger once.
        let edited = ConfigFileSnapshot::Contents(b"color=purple".to_vec());

        assert!(snapshot_changed(&mut previous, edited.clone()));
        assert!(!snapshot_changed(&mut previous, edited));

        // Deleting the configuration is also a change.
        assert!(snapshot_changed(&mut previous, ConfigFileSnapshot::Missing,));
    }

    #[test]
    fn tiling_settings_are_parsed() {
        let config = parse_config(
            "[tiling]\n\
            outer_gap = 12\n\
            inner_gap = 8\n\
            default_master_ratio = 60\n\
            start_tiled = true\n",
        )
        .unwrap();

        assert_eq!(config.tiling.gaps.outer, 12);
        assert_eq!(config.tiling.gaps.inner, 8);
        assert_eq!(config.tiling.default_master_ratio, 60);
        assert!(config.tiling.start_tiled);
    }

    #[test]
    fn invalid_tiling_settings_are_rejected() {
        assert!(parse_config("[tiling]\nouter_gap = -5\n").is_err());

        assert!(parse_config("[tiling]\ndefault_master_ratio = 95\n").is_err());
    }

    #[test]
    fn tiling_changes_are_detected_during_reload() {
        let mut config = AnwiliteConfig::default();

        let updated = parse_config("[tiling]\nouter_gap = 12\ninner_gap = 8\n").unwrap();

        assert!(config.apply_reload(Ok(updated.clone())).unwrap());

        assert_eq!(config.tiling.gaps.outer, 12);
        assert_eq!(config.tiling.gaps.inner, 8);

        // Applying the same settings again must do nothing.
        assert!(!config.apply_reload(Ok(updated)).unwrap());
    }

    #[test]
    fn explicit_tiled_workspaces_are_parsed() {
        let config = parse_config(
            "[tiling]\n\
            start_tiled = true\n\
            tiled_workspaces = [1, 3, 10]\n",
        )
        .unwrap();

        let tiled = config.tiling.tiled_workspaces.unwrap();

        assert!(tiled[0]);
        assert!(!tiled[1]);
        assert!(tiled[2]);
        assert!(tiled[9]);
    }

    #[test]
    fn invalid_tiled_workspace_lists_are_rejected() {
        assert!(parse_config("[tiling]\ntiled_workspaces = [0]\n").is_err());

        assert!(parse_config("[tiling]\ntiled_workspaces = [11]\n").is_err());

        assert!(parse_config("[tiling]\ntiled_workspaces = [2, 2]\n").is_err());
    }

    #[test]
    fn app_defaults_keep_the_development_terminal() {
        let config = parse_config("").unwrap();

        assert_eq!(config.apps.terminal, vec!["weston-terminal"]);

        assert!(config.apps.autostart_terminal);
        assert!(config.apps.file_manager.is_empty());
    }

    #[test]
    fn application_commands_are_parsed() {
        let config = parse_config(
            "[apps]\n\
            terminal = [\"foot\", \"--title\", \"Anwilite\"]\n\
            file_manager = [\"thunar\"]\n\
            autostart_terminal = false\n",
        )
        .unwrap();

        assert_eq!(config.apps.terminal, vec!["foot", "--title", "Anwilite"]);

        assert_eq!(config.apps.file_manager, vec!["thunar"]);

        assert!(!config.apps.autostart_terminal);
    }

    #[test]
    fn application_commands_can_be_disabled() {
        let config = parse_config(
            "[apps]\n\
            terminal = []\n\
            file_manager = []\n\
            autostart_terminal = false\n",
        )
        .unwrap();

        assert!(config.apps.terminal.is_empty());
        assert!(config.apps.file_manager.is_empty());
    }

    #[test]
    fn empty_executable_names_are_rejected() {
        assert!(parse_config("[apps]\nterminal = [\"\"]\n").is_err());

        assert!(parse_config("[apps]\nfile_manager = [\"   \"]\n").is_err());
    }

    #[test]
    fn application_changes_are_detected_on_reload() {
        let mut config = AnwiliteConfig::default();

        let next = parse_config("[apps]\nterminal = [\"foot\"]\n").unwrap();

        assert!(config.apply_reload(Ok(next.clone())).unwrap());
        assert_eq!(config.apps.terminal, vec!["foot"]);

        assert!(!config.apply_reload(Ok(next)).unwrap());
    }
    #[test]
    fn custom_keybindings_are_parsed() {
        let config = parse_config(
            "[[keybindings]]\n\
            key = \"Super+Y\"\n\
            action = \"launch_terminal\"\n",
        )
        .unwrap();

        assert_ne!(config.keybindings, KeyBindings::default());
    }

    #[test]
    fn quarter_snap_actions_load_from_toml_and_validate() {
        let config = parse_config(
            "[[keybindings]]\n\
             key = \"Super+Ctrl+7\"\n\
             action = \"snap_top_left\"\n\
             [[keybindings]]\n\
             key = \"Super+Ctrl+9\"\n\
             action = \"snap_top_right\"\n",
        )
        .unwrap();

        assert_ne!(config.keybindings, KeyBindings::default());
        assert!(parse_config(
            "[[keybindings]]\nkey = \"Super+Ctrl+7\"\naction = \"snap_top_left\"\nworkspace = 1\n"
        )
        .is_err());
    }

    #[test]
    fn spatial_and_cycle_bindings_load_from_toml() {
        let config = parse_config(
            "[[keybindings]]\nkey = \"Super+Alt+Left\"\naction = \"focus_left\"\n\
             [[keybindings]]\nkey = \"Alt+Tab\"\naction = \"cycle_windows\"\n\
             [[keybindings]]\nkey = \"Super+Shift+Q\"\naction = \"close_window\"\n",
        )
        .unwrap();
        assert_ne!(config.keybindings, KeyBindings::default());
    }

    #[test]
    fn invalid_keybinding_reload_keeps_previous_config() {
        let mut config = AnwiliteConfig::default();

        let invalid = parse_config(
            "[[keybindings]]\n\
            key = \"Super+Shift+C\"\n\
            action = \"launch_terminal\"\n",
        );

        assert!(config.apply_reload(invalid).is_err());
        assert_eq!(config.keybindings, KeyBindings::default());
    }

    #[test]
    fn window_rules_match_exact_app_ids_and_workspaces() {
        let config = parse_config(
            "[[window_rules]]\napp_id = \"org.mozilla.firefox\"\nworkspace = 2\n\
             [[window_rules]]\napp_id = \"foot\"\nworkspace = 3\n",
        )
        .unwrap();

        assert_eq!(
            matching_workspace(&config.window_rules, "org.mozilla.firefox"),
            Some(2)
        );
        assert_eq!(matching_workspace(&config.window_rules, "foot"), Some(3));
        assert_eq!(matching_workspace(&config.window_rules, "Foot"), None);
        assert_eq!(matching_workspace(&config.window_rules, "firefox"), None);
    }

    #[test]
    fn invalid_window_rules_are_rejected() {
        assert!(parse_config("[[window_rules]]\napp_id = \"foo\"\nworkspace = 0\n").is_err());
        assert!(parse_config("[[window_rules]]\napp_id = \"foo\"\nworkspace = 11\n").is_err());
        assert!(parse_config("[[window_rules]]\napp_id = \"\"\nworkspace = 2\n").is_err());
        assert!(
            parse_config("[[window_rules]]\napp_id = \"foo\"\nworkspace = 2\nunknown = true\n")
                .is_err()
        );
        assert!(parse_config("[[window_rules]]\napp_id = \"foo\"\nworkspace = 2\n[[window_rules]]\napp_id = \"foo\"\nworkspace = 3\n").is_err());
    }

    #[test]
    fn window_rules_change_is_detected_on_reload() {
        let mut config = AnwiliteConfig::default();
        let next = parse_config("[[window_rules]]\napp_id = \"foot\"\nworkspace = 4\n").unwrap();
        assert!(config.apply_reload(Ok(next.clone())).unwrap());
        assert_eq!(matching_workspace(&config.window_rules, "foot"), Some(4));
        assert!(!config.apply_reload(Ok(next)).unwrap());
    }

    #[test]
    fn application_menu_command_is_parsed() {
        let config = parse_config("[apps]\nmenu = [\"weston-terminal\"]\n").unwrap();

        assert_eq!(config.apps.menu, vec!["weston-terminal"]);
    }
}

#[cfg(test)]
mod grid_config_tests {
    use super::*;
    #[test]
    fn grid_workspaces_are_independent_of_tiling_enablement() {
        let config =
            parse_config("[tiling]\ntiled_workspaces = [1, 3]\ngrid_workspaces = [3, 4]\n")
                .unwrap();
        let tiled = config.tiling.tiled_workspaces.unwrap();
        let grids = config.tiling.grid_workspaces.unwrap();
        assert!(tiled[0] && tiled[2]);
        assert!(!tiled[3] && grids[3] && grids[2]);
        assert!(!grids[0]);
    }
    #[test]
    fn bad_grid_workspace_is_rejected() {
        for list in ["[0]", "[11]", "[2, 2]"] {
            assert!(parse_config(&format!("[tiling]\ngrid_workspaces = {list}\n")).is_err());
        }
    }
}
