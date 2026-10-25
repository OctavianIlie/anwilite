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
}
