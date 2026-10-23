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
        0 => Some(Vec::new()),

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

        2 => {
            let left = snap_rect_with_gaps(output_size, SnapSide::Left, gaps)?;

            let right = snap_rect_with_gaps(output_size, SnapSide::Right, gaps)?;

            Some(vec![left, right])
        }

        _ => None,
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

        let left = snap_rect_with_gaps(
            (1200, 800),
            SnapSide::Left,
            gaps,
        ).unwrap();

        let right = snap_rect_with_gaps(
            (1200, 800),
            SnapSide::Right,
            gaps,
        ).unwrap();

        assert_eq!(left, SnapRect {
            x: 12,
            y: 12,
            width: 582,
            height: 776,
        });

        assert_eq!(right, SnapRect {
            x: 606,
            y: 12,
            width: 582,
            height: 776,
        });
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
    fn unsupported_or_invalid_layouts_fail() {
        let gaps = SnapGaps {
            outer: 12,
            inner: 12,
        };

        // Three-window layouts aren't supported yet.
        assert_eq!(tile_rects((1200, 800), 3, gaps), None);

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
}
