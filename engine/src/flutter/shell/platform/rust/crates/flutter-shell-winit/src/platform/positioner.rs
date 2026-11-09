//! Pure placement for desktops whose compositor does not position popups.
use super::super::{PopupAnchor, PopupPlacement};

fn axis(anchor: i32, gravity: i32, rect: (f64, f64), size: f64, offset: f64) -> f64 {
    rect.0 + (anchor + 1) as f64 * rect.1 / 2.0 + (gravity - 1) as f64 * size / 2.0 + offset
}

fn components(anchor: PopupAnchor) -> (i32, i32) {
    use PopupAnchor::*;
    match anchor {
        None => (0, 0),
        Top => (0, -1),
        Bottom => (0, 1),
        Left => (-1, 0),
        Right => (1, 0),
        TopLeft => (-1, -1),
        BottomLeft => (-1, 1),
        TopRight => (1, -1),
        BottomRight => (1, 1),
    }
}

fn constrain(
    mut position: f64,
    mut size: f64,
    flipped: f64,
    bounds: (f64, f64),
    slide: bool,
    flip: bool,
    resize: bool,
) -> (f64, f64) {
    let fits = |p: f64| p >= bounds.0 && p + size <= bounds.0 + bounds.1;
    if !fits(position) && flip && fits(flipped) {
        position = flipped;
    }
    if slide {
        position = position
            .max(bounds.0)
            .min((bounds.0 + bounds.1 - size).max(bounds.0));
    }
    if resize {
        let end = (position + size).min(bounds.0 + bounds.1);
        position = position.max(bounds.0).min(bounds.0 + bounds.1 - 1.0);
        size = (end - position).max(1.0);
    }
    (position, size)
}

/// All inputs/outputs use the parent's logical coordinate scale. Work-area
/// origin can be negative for displays left of or above the primary display.
pub(super) fn place(
    p: PopupPlacement,
    origin: (f64, f64),
    work: (f64, f64, f64, f64),
) -> (f64, f64, f64, f64) {
    let (ax, ay) = components(p.anchor);
    let (gx, gy) = components(p.gravity);
    let rect_x = (
        origin.0 + f64::from(p.anchor_rect.0),
        f64::from(p.anchor_rect.2),
    );
    let rect_y = (
        origin.1 + f64::from(p.anchor_rect.1),
        f64::from(p.anchor_rect.3),
    );
    let width = f64::from(p.width.max(1));
    let height = f64::from(p.height.max(1));
    let x = axis(ax, gx, rect_x, width, f64::from(p.offset.0));
    let y = axis(ay, gy, rect_y, height, f64::from(p.offset.1));
    let flags = p.constraint_adjustment;
    let (x, width) = constrain(
        x,
        width,
        axis(-ax, -gx, rect_x, width, -f64::from(p.offset.0)),
        (work.0, work.2),
        flags & 1 != 0,
        flags & 4 != 0,
        flags & 16 != 0,
    );
    let (y, height) = constrain(
        y,
        height,
        axis(-ay, -gy, rect_y, height, -f64::from(p.offset.1)),
        (work.1, work.3),
        flags & 2 != 0,
        flags & 8 != 0,
        flags & 32 != 0,
    );
    (x, y, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> PopupPlacement {
        PopupPlacement {
            width: 80,
            height: 60,
            anchor_rect: (90, 90, 10, 10),
            anchor: PopupAnchor::BottomRight,
            gravity: PopupAnchor::BottomRight,
            offset: (0, 0),
            constraint_adjustment: 0,
        }
    }
    #[test]
    fn preserves_unconstrained_anchor_and_offset_with_negative_screen_origin() {
        let mut p = request();
        p.offset = (7, 9);
        assert_eq!(
            place(p, (-200.0, -100.0), (-200.0, -100.0, 100.0, 100.0)),
            (-93.0, 9.0, 80.0, 60.0)
        );
    }
    #[test]
    fn flips_before_sliding_and_resizing() {
        let mut p = request();
        p.constraint_adjustment = 0x3f;
        assert_eq!(
            place(p, (0.0, 0.0), (0.0, 0.0, 120.0, 120.0)),
            (10.0, 30.0, 80.0, 60.0)
        );
        p.width = 300;
        p.height = 250;
        assert_eq!(
            place(p, (0.0, 0.0), (-20.0, -30.0, 120.0, 120.0)),
            (-20.0, -30.0, 120.0, 120.0)
        );
    }
    #[test]
    fn axes_are_constrained_independently() {
        let mut p = request();
        p.constraint_adjustment = 1;
        assert_eq!(
            place(p, (0.0, 0.0), (0.0, 0.0, 120.0, 120.0)),
            (40.0, 100.0, 80.0, 60.0)
        );
    }
}
