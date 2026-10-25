#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapSide {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

pub fn snap_rect(output_size: (i32, i32), side: SnapSide) -> Option<SnapRect> {
    let (output_width, output_height) = output_size;

    if output_width < 2 || output_height < 1 {
        return None;
    }

    let left_width = output_width / 2;
    let right_width = output_width - left_width;

    let rect = match side {
        SnapSide::Left => SnapRect {
            x: 0,
            y: 0,
            width: left_width,
            height: output_height,
        },
        SnapSide::Right => SnapRect {
            x: left_width,
            y: 0,
            width: right_width,
            height: output_height,
        },
    };
    Some(rect)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapGaps {
    pub outer: i32,
    pub inner: i32,
}

pub const DEFAULT_SNAP_GAPS: SnapGaps = SnapGaps { outer: 6, inner: 6 };

pub fn snap_rect_with_gaps(
    output_size: (i32, i32),
    side: SnapSide,
    gaps: SnapGaps,
) -> Option<SnapRect> {
    if gaps.outer < 0 || gaps.inner < 0 {
        return None;
    }
    let half = snap_rect(output_size, side)?;

    let inner_left = gaps.inner / 2;
    let inner_right = gaps.inner - inner_left;

    let (left_inset, right_inset) = match side {
        SnapSide::Left => (gaps.outer, inner_right),
        SnapSide::Right => (inner_left, gaps.outer),
    };

    let width = half
        .width
        .checked_sub(left_inset)?
        .checked_sub(right_inset)?;

    let height = half.height.checked_sub(gaps.outer.checked_mul(2)?)?;

    if width <= 0 || height <= 0 {
        return None;
    }
    Some(SnapRect {
        x: half.x.checked_add(left_inset)?,
        y: gaps.outer,
        width,
        height,
    })
}

fn split_columns(
    output_size: (i32, i32),
    gaps: SnapGaps,
    master_percent: u8,
) -> Option<(SnapRect, SnapRect)> {
    let (output_width, output_height) = output_size;

    if output_width < 2
        || output_height < 1
        || gaps.outer < 0
        || gaps.inner < 0
        || !(20..=80).contains(&master_percent)
    {
        return None;
    }

    // Use i64 for intermediate multiplication.
    let split =
        i32::try_from(i64::from(output_width).checked_mul(i64::from(master_percent))? / 100)
            .ok()?;

    // Match our existing gap convention.
    let inner_left = gaps.inner / 2;
    let inner_right = gaps.inner - inner_left;

    let height = output_height.checked_sub(gaps.outer.checked_mul(2)?)?;

    let master_width = split.checked_sub(gaps.outer)?.checked_sub(inner_right)?;

    let stack_width = output_width
        .checked_sub(split)?
        .checked_sub(inner_left)?
        .checked_sub(gaps.outer)?;

    if master_width <= 0 || stack_width <= 0 || height <= 0 {
        return None;
    }

    let stack_x = split.checked_add(inner_left)?;

    let master = SnapRect {
        x: gaps.outer,
        y: gaps.outer,
        width: master_width,
        height,
    };

    let stack = SnapRect {
        x: stack_x,
        y: gaps.outer,
        width: stack_width,
        height,
    };

    Some((master, stack))
}

pub fn tile_rects(
    output_size: (i32, i32),
    window_count: usize,
    gaps: SnapGaps,
) -> Option<Vec<SnapRect>> {
    tile_rects_with_ratio(output_size, window_count, gaps, 50)
}

pub fn tile_rects_with_ratio(
    output_size: (i32, i32),
    window_count: usize,
    gaps: SnapGaps,
    master_percent: u8,
) -> Option<Vec<SnapRect>> {
    if gaps.outer < 0 || gaps.inner < 0 {
        return None;
    }
    if !(20..=80).contains(&master_percent) {
        return None;
    }

    match window_count {
        // No windows.
        0 => Some(Vec::new()),

        // One window fills the area inside the outer gaps.
        1 => {
            let (output_width, output_height) = output_size;

            let total_gap = gaps.outer.checked_mul(2)?;

            let width = output_width.checked_sub(total_gap)?;
            let height = output_height.checked_sub(total_gap)?;

            if width <= 0 || height <= 0 {
                return None;
            }

            Some(vec![SnapRect {
                x: gaps.outer,
                y: gaps.outer,
                width,
                height,
            }])
        }

        // Preserve the existing two-window layout.
        2 => {
            let (master, stack) = split_columns(output_size, gaps, master_percent)?;

            Some(vec![master, stack])
        }

        n => {
            let (master, stack_area) = split_columns(output_size, gaps, master_percent)?;

            let stack_count = i32::try_from(n - 1).ok()?;

            let gap_count = stack_count.checked_sub(1)?;

            let total_inner_gaps = gaps.inner.checked_mul(gap_count)?;

            let usable_height = stack_area.height.checked_sub(total_inner_gaps)?;

            if usable_height < stack_count {
                return None;
            }

            let base_height = usable_height / stack_count;

            let remainder = usable_height % stack_count;

            let mut rects = Vec::with_capacity(n);

            rects.push(master);

            let mut y = stack_area.y;

            for i in 0..stack_count {
                let extra = if i < remainder { 1 } else { 0 };

                let height = base_height + extra;

                rects.push(SnapRect {
                    x: stack_area.x,
                    y,
                    width: stack_area.width,
                    height,
                });

                y = y.checked_add(height)?;

                if i + 1 < stack_count {
                    y = y.checked_add(gaps.inner)?;
                }
            }

            Some(rects)
        }
    }
}

pub fn focus_border_rects(rect: SnapRect, thickness: i32) -> Option<[SnapRect; 4]> {
    let double = thickness.checked_mul(2)?;

    if thickness <= 0 || rect.width <= double || rect.height <= double {
        return None;
    }

    let right = rect.x.checked_add(rect.width)?;
    let bottom = rect.y.checked_add(rect.height)?;
    let inner_y = rect.y.checked_add(thickness)?;
    let inner_height = rect.height - double;

    Some([
        // Top
        SnapRect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: thickness,
        },
        // Bottom
        SnapRect {
            x: rect.x,
            y: bottom - thickness,
            width: rect.width,
            height: thickness,
        },
        // Left
        SnapRect {
            x: rect.x,
            y: inner_y,
            width: thickness,
            height: inner_height,
        },
        // Right
        SnapRect {
            x: right - thickness,
            y: inner_y,
            width: thickness,
            height: inner_height,
        },
    ])
}

/// Draw a focus outline in the *gap outside* the compositor-owned tile.
/// `clearance` pixels remain between the tile and the outline; the outline
/// itself occupies `thickness` pixels. This avoids matching client-drawn
/// frames/shadows, whose visible pixels vary from application to application.
pub fn focus_border_outside_tile(
    tile: SnapRect,
    thickness: i32,
    clearance: i32,
) -> Option<[SnapRect; 4]> {
    if thickness <= 0 || clearance < 0 {
        return None;
    }

    let margin = thickness.checked_add(clearance)?;
    let double_margin = margin.checked_mul(2)?;

    let outside = SnapRect {
        x: tile.x.checked_sub(margin)?,
        y: tile.y.checked_sub(margin)?,
        width: tile.width.checked_add(double_margin)?,
        height: tile.height.checked_add(double_margin)?,
    };

    focus_border_rects(outside, thickness)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_snap_uses_left_half() {
        let rect = snap_rect((1200, 800), SnapSide::Left).unwrap();

        assert_eq!(rect.x, 0);
        assert_eq!(rect.y, 0);
        assert_eq!(rect.width, 600);
        assert_eq!(rect.height, 800);
    }

    #[test]
    fn right_snap_uses_right_half() {
        let rect = snap_rect((1200, 800), SnapSide::Right).unwrap();

        assert_eq!(rect.x, 600);
        assert_eq!(rect.y, 0);
        assert_eq!(rect.width, 600);
        assert_eq!(rect.height, 800);
    }

    #[test]
    fn odd_width_has_no_missing_pixels() {
        let left = snap_rect((1001, 700), SnapSide::Left).unwrap();
        let right = snap_rect((1001, 700), SnapSide::Right).unwrap();

        assert_eq!(left.width + right.width, 1001);
        assert_eq!(right.x, left.width);
    }

    #[test]
    fn tiny_output_is_rejected() {
        assert_eq!(snap_rect((1, 800), SnapSide::Left), None);
    }

    #[test]
    fn gaps_create_correct_rectangles() {
        let gaps = SnapGaps {
            outer: 12,
            inner: 12,
        };

        let left = snap_rect_with_gaps((1200, 800), SnapSide::Left, gaps).unwrap();

        let right = snap_rect_with_gaps((1200, 800), SnapSide::Right, gaps).unwrap();

        assert_eq!(
            left,
            SnapRect {
                x: 12,
                y: 12,
                width: 582,
                height: 776,
            }
        );

        assert_eq!(
            right,
            SnapRect {
                x: 606,
                y: 12,
                width: 582,
                height: 776,
            }
        );
    }

    #[test]
    fn odd_inner_gap_has_exact_spacing() {
        let gaps = SnapGaps {
            outer: 11,
            inner: 9,
        };

        let left = snap_rect_with_gaps((1001, 701), SnapSide::Left, gaps).unwrap();

        let right = snap_rect_with_gaps((1001, 701), SnapSide::Right, gaps).unwrap();

        assert_eq!(right.x - (left.x + left.width), 9);
        assert_eq!(left.x, 11);
        assert_eq!(right.x + right.width, 1001 - 11);
    }

    #[test]
    fn invalid_gaps_are_rejected() {
        // Negative gaps should always be rejected.
        let invalid = SnapGaps {
            outer: -1,
            inner: 12,
        };

        assert_eq!(
            snap_rect_with_gaps((800, 600), SnapSide::Left, invalid,),
            None
        );

        // These explicit gaps are too large for a 20x20 output.
        let oversized = SnapGaps {
            outer: 12,
            inner: 12,
        };

        assert_eq!(
            snap_rect_with_gaps((20, 20), SnapSide::Left, oversized,),
            None
        );
    }

    #[test]
    fn zero_windows_produces_empty_layout() {
        let gaps = SnapGaps {
            outer: 12,
            inner: 12,
        };

        let result = tile_rects((1200, 800), 0, gaps).unwrap();

        assert!(result.is_empty());
    }

    #[test]
    fn one_window_fills_usable_area() {
        let gaps = SnapGaps {
            outer: 12,
            inner: 12,
        };

        let result = tile_rects((1200, 800), 1, gaps).unwrap();

        assert_eq!(
            result,
            vec![SnapRect {
                x: 12,
                y: 12,
                width: 1176,
                height: 776,
            },]
        );
    }

    #[test]
    fn two_windows_share_output() {
        let gaps = SnapGaps {
            outer: 12,
            inner: 12,
        };

        let result = tile_rects((1200, 800), 2, gaps).unwrap();

        assert_eq!(
            result,
            vec![
                SnapRect {
                    x: 12,
                    y: 12,
                    width: 582,
                    height: 776,
                },
                SnapRect {
                    x: 606,
                    y: 12,
                    width: 582,
                    height: 776,
                },
            ]
        );
    }

    #[test]
    fn two_windows_respect_odd_output_width() {
        let gaps = SnapGaps {
            outer: 11,
            inner: 9,
        };

        let result = tile_rects((1001, 701), 2, gaps).unwrap();

        let left = result[0];
        let right = result[1];

        assert_eq!(right.x - (left.x + left.width), 9);
        assert_eq!(left.x, 11);
        assert_eq!(right.x + right.width, 1001 - 11);
    }

    #[test]
    fn invalid_layouts_fail() {
        let gaps = SnapGaps {
            outer: 12,
            inner: 12,
        };

        // Insufficient space for the outer gaps.
        assert_eq!(tile_rects((20, 20), 1, gaps), None);

        // Negative gap values are invalid.
        assert_eq!(
            tile_rects(
                (1200, 800),
                2,
                SnapGaps {
                    outer: -1,
                    inner: 12,
                },
            ),
            None
        );
    }

    #[test]
    fn three_windows_use_master_and_stack() {
        let rects = tile_rects((1200, 800), 3, DEFAULT_SNAP_GAPS).unwrap();

        assert_eq!(rects.len(), 3);

        assert_eq!(
            rects[0],
            SnapRect {
                x: 6,
                y: 6,
                width: 591,
                height: 788,
            }
        );

        assert_eq!(
            rects[1],
            SnapRect {
                x: 603,
                y: 6,
                width: 591,
                height: 391,
            }
        );

        assert_eq!(
            rects[2],
            SnapRect {
                x: 603,
                y: 403,
                width: 591,
                height: 391,
            }
        );
    }

    #[test]
    fn four_windows_distribute_remainder_pixels() {
        let rects = tile_rects((1200, 800), 4, DEFAULT_SNAP_GAPS).unwrap();

        assert_eq!(rects.len(), 4);

        // Master stays the same size.
        assert_eq!(rects[0].height, 788);

        // Three stacked windows share the right column.
        assert_eq!(rects[1].height, 259);
        assert_eq!(rects[2].height, 259);
        assert_eq!(rects[3].height, 258);

        // Exact 6-pixel gaps between stacked windows.
        assert_eq!(rects[2].y - (rects[1].y + rects[1].height), 6);

        assert_eq!(rects[3].y - (rects[2].y + rects[2].height), 6);

        // Final window ends at the bottom outer gap.
        assert_eq!(rects[3].y + rects[3].height, 794);
    }

    #[test]
    fn stack_rejects_insufficient_vertical_space() {
        assert_eq!(tile_rects((1200, 25), 5, DEFAULT_SNAP_GAPS,), None);
    }

    #[test]
    fn default_ratio_preserves_original_layouts() {
        for count in 0..=6 {
            let original = tile_rects((1200, 800), count, DEFAULT_SNAP_GAPS);

            let explicit = tile_rects_with_ratio((1200, 800), count, DEFAULT_SNAP_GAPS, 50);

            assert_eq!(original, explicit);
        }
    }

    #[test]
    fn sixty_percent_master_uses_correct_width() {
        let rects = tile_rects_with_ratio((1200, 800), 3, DEFAULT_SNAP_GAPS, 60).unwrap();

        assert_eq!(rects.len(), 3);

        assert_eq!(rects[0].x, 6);
        assert_eq!(rects[0].width, 711);

        assert_eq!(rects[1].x, 723);
        assert_eq!(rects[1].width, 471);

        assert_eq!(rects[2].x, 723);
        assert_eq!(rects[2].width, 471);

        // Preserve the 6-pixel horizontal gap.
        assert_eq!(rects[1].x - (rects[0].x + rects[0].width), 6,);
    }

    #[test]
    fn seventy_percent_master_preserves_stack_gaps() {
        let rects = tile_rects_with_ratio((1200, 800), 4, DEFAULT_SNAP_GAPS, 70).unwrap();

        assert_eq!(rects.len(), 4);

        assert_eq!(rects[0].width, 831);

        for rect in &rects[1..] {
            assert_eq!(rect.x, 843);
            assert_eq!(rect.width, 351);
        }

        // Existing vertical stack spacing still works.
        for pair in rects[1..].windows(2) {
            assert_eq!(pair[1].y - (pair[0].y + pair[0].height), 6,);
        }
    }

    #[test]
    fn invalid_ratios_and_tiny_columns_fail() {
        let gaps = DEFAULT_SNAP_GAPS;

        assert_eq!(tile_rects_with_ratio((1200, 800), 3, gaps, 19), None,);

        assert_eq!(tile_rects_with_ratio((1200, 800), 3, gaps, 81), None,);

        // At this width, the 20% master column cannot fit
        // the required gaps and a positive drawable width.
        assert_eq!(tile_rects_with_ratio((24, 800), 3, gaps, 20), None,);
    }

    #[test]
    fn focus_border_has_correct_edges() {
        let rect = SnapRect {
            x: 6,
            y: 6,
            width: 100,
            height: 80,
        };

        let edges = focus_border_rects(rect, 3).unwrap();

        assert_eq!(
            edges,
            [
                SnapRect {
                    x: 6,
                    y: 6,
                    width: 100,
                    height: 3,
                },
                SnapRect {
                    x: 6,
                    y: 83,
                    width: 100,
                    height: 3,
                },
                SnapRect {
                    x: 6,
                    y: 9,
                    width: 3,
                    height: 74,
                },
                SnapRect {
                    x: 103,
                    y: 9,
                    width: 3,
                    height: 74,
                },
            ]
        );
    }

    #[test]
    fn focus_border_in_gap_stays_outside_tile() {
        let tile = SnapRect {
            x: 6,
            y: 6,
            width: 100,
            height: 80,
        };

        let edges = focus_border_outside_tile(tile, 2, 1).unwrap();
        assert_eq!(
            edges[0],
            SnapRect {
                x: 3,
                y: 3,
                width: 106,
                height: 2
            }
        );
        assert_eq!(
            edges[1],
            SnapRect {
                x: 3,
                y: 87,
                width: 106,
                height: 2
            }
        );
        assert_eq!(
            edges[2],
            SnapRect {
                x: 3,
                y: 5,
                width: 2,
                height: 82
            }
        );
        assert_eq!(
            edges[3],
            SnapRect {
                x: 107,
                y: 5,
                width: 2,
                height: 82
            }
        );
    }

    #[test]
    fn focus_border_in_gap_rejects_invalid_parameters() {
        let tile = SnapRect {
            x: 6,
            y: 6,
            width: 100,
            height: 80,
        };
        assert_eq!(focus_border_outside_tile(tile, 0, 1), None);
        assert_eq!(focus_border_outside_tile(tile, 2, -1), None);
        assert_eq!(focus_border_outside_tile(tile, i32::MAX, 1), None);
    }

    #[test]
    fn focus_border_rejects_invalid_sizes() {
        let rect = SnapRect {
            x: 0,
            y: 0,
            width: 100,
            height: 80,
        };

        assert_eq!(focus_border_rects(rect, 0), None);
        assert_eq!(focus_border_rects(rect, -1), None);
        assert_eq!(focus_border_rects(rect, 50), None);
    }
}
