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

pub fn tile_rects(
    output_size: (i32, i32),
    window_count: usize,
    gaps: SnapGaps,
) -> Option<Vec<SnapRect>> {
    if gaps.outer < 0 || gaps.inner < 0 {
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
            let left = snap_rect_with_gaps(output_size, SnapSide::Left, gaps)?;

            let right = snap_rect_with_gaps(output_size, SnapSide::Right, gaps)?;

            Some(vec![left, right])
        }

        n => {
            let master = snap_rect_with_gaps(output_size, SnapSide::Left, gaps)?;

            let stack_area = snap_rect_with_gaps(output_size, SnapSide::Right, gaps)?;

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
}
