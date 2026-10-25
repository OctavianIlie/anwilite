use crate::appearance::{ACTIVE_BORDER, BorderStyle, INACTIVE_BORDER};
use serde::Deserialize;
use std::{env, fs, io, path::PathBuf};

#[derive(Debug, Clone, Copy)]
pub struct AnwiliteConfig {
    pub active_border: BorderStyle,
    pub inactive_border: Option<BorderStyle>,
}

impl Default for AnwiliteConfig {
    fn default() -> Self {
        Self {
            active_border: ACTIVE_BORDER,
            inactive_border: INACTIVE_BORDER,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    borders: BorderOptions,
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

fn parse_hex_color(raw: &str) -> Result<[f32; 4], String> {
    let hex = raw
        .strip_prefix('#')
        .ok_or_else(|| format!("color '{raw}' must start with #"))?;

    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("invalid color '{raw}'; use #RRGGBB or #RRGGBBAA"));
    }

    let mut rgba = [1.0; 4];

    for i in 0..(hex.len() / 2) {
        let start = i * 2;
        let component = &hex[start..start + 2];

        let value =
            u8::from_str_radix(component, 16).map_err(|_| format!("invalid color '{raw}'"))?;

        rgba[i] = f32::from(value) / 255.0;
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
}
