//! Compositor-owned appearance settings.
//! These are compile-time defaults for now; a configuration file can come later.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderStyle {
    /// Focus outline width in logical pixels.
    pub thickness: i32,
    /// Gap between the committed window geometry and the outline.
    pub clearance: i32,
    /// RGBA components in the inclusive range 0.0..=1.0.
    pub rgba: [f32; 4],
}

impl BorderStyle {
    pub fn is_valid(&self) -> bool {
        self.thickness > 0
            && self.clearance >= 0
            && self
                .rgba
                .iter()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
    }
}

/// Change these values to experiment with the focus indicator.
pub const ACTIVE_BORDER: BorderStyle = BorderStyle {
    thickness: 2,
    clearance: 0,
    rgba: [0.18, 0.85, 0.68, 1.0],
};

pub const INACTIVE_BORDER: Option<BorderStyle> = Some(BorderStyle {
    thickness: 1,
    clearance: 0,
    rgba: [0.30, 0.34, 0.40, 1.0],
});

pub fn border_style(is_focused: bool) -> Option<BorderStyle> {
    if is_focused {
        Some(ACTIVE_BORDER)
    } else {
        INACTIVE_BORDER
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_border_defaults_are_valid() {
        assert!(ACTIVE_BORDER.is_valid());
        assert_eq!(ACTIVE_BORDER.thickness, 2);
        assert_eq!(ACTIVE_BORDER.clearance, 0);
    }

    #[test]
    fn invalid_border_values_are_rejected() {
        let invalid_thickness = BorderStyle {
            thickness: 0,
            ..ACTIVE_BORDER
        };
        assert!(!invalid_thickness.is_valid());

        let invalid_clearance = BorderStyle {
            clearance: -1,
            ..ACTIVE_BORDER
        };
        assert!(!invalid_clearance.is_valid());

        let invalid_color = BorderStyle {
            rgba: [1.2, 0.85, 0.68, 1.0],
            ..ACTIVE_BORDER
        };
        assert!(!invalid_color.is_valid());
    }

    #[test]
    fn focused_window_uses_active_border() {
        assert_eq!(border_style(true), Some(ACTIVE_BORDER));
    }

    #[test]
    fn inactive_border_is_optional_and_valid() {
        assert_eq!(border_style(false), INACTIVE_BORDER);

        if let Some(style) = INACTIVE_BORDER {
            assert!(style.is_valid());
        }
    }
}
