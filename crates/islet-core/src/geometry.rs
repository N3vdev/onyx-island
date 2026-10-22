//! Small, platform-free geometry helpers shared by hit-testing and layout.

/// An axis-aligned rectangle in logical or physical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    #[must_use]
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    pub fn right(self) -> f32 {
        self.x + self.width
    }

    #[must_use]
    pub fn bottom(self) -> f32 {
        self.y + self.height
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    #[must_use]
    pub fn contains(self, x: f32, y: f32) -> bool {
        !self.is_empty() && x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    #[must_use]
    pub fn center(self) -> (f32, f32) {
        (self.x + self.width * 0.5, self.y + self.height * 0.5)
    }

    /// The smallest rectangle containing both.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }

    /// Grows the rectangle by `amount` on every side.
    #[must_use]
    pub fn inflate(self, amount: f32) -> Self {
        Self::new(
            self.x - amount,
            self.y - amount,
            self.width + amount * 2.0,
            self.height + amount * 2.0,
        )
    }
}

/// Tests a point against a rounded rectangle (the island's hit shape).
///
/// The visual uses a continuous-corner squircle. Hit-testing with the
/// enclosing rounded rectangle is deliberately slightly generous at the
/// corners: missing a click on the island is worse than a click being
/// swallowed in a transparent corner a few pixels wide.
#[must_use]
pub fn point_in_rounded_rect(x: f32, y: f32, rect: Rect, corner_radius: f32) -> bool {
    if !rect.contains(x, y) {
        return false;
    }

    let radius = corner_radius
        .max(0.0)
        .min(rect.width * 0.5)
        .min(rect.height * 0.5);
    if radius <= 0.0 {
        return true;
    }

    let (cx, cy) = rect.center();
    let dx = (x - cx).abs() - (rect.width * 0.5 - radius);
    let dy = (y - cy).abs() - (rect.height * 0.5 - radius);
    let dx = dx.max(0.0);
    let dy = dy.max(0.0);
    dx * dx + dy * dy <= radius * radius
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_contains_is_half_open() {
        let rect = Rect::new(10.0, 20.0, 100.0, 40.0);
        assert!(rect.contains(10.0, 20.0));
        assert!(rect.contains(109.9, 59.9));
        assert!(!rect.contains(110.0, 30.0));
        assert!(!rect.contains(20.0, 60.0));
        assert!(!rect.contains(9.9, 30.0));
    }

    #[test]
    fn union_and_inflate() {
        let a = Rect::new(10.0, 10.0, 20.0, 20.0);
        let b = Rect::new(25.0, 0.0, 20.0, 15.0);
        assert_eq!(a.union(b), Rect::new(10.0, 0.0, 35.0, 30.0));
        assert_eq!(a.inflate(2.0), Rect::new(8.0, 8.0, 24.0, 24.0));
    }

    #[test]
    fn empty_rect_contains_nothing() {
        let rect = Rect::new(0.0, 0.0, 0.0, 30.0);
        assert!(!rect.contains(0.0, 0.0));
        assert!(!rect.contains(0.0, 10.0));
    }

    #[test]
    fn rounded_corners_are_excluded() {
        let rect = Rect::new(0.0, 0.0, 100.0, 40.0);
        assert!(point_in_rounded_rect(50.0, 20.0, rect, 18.0));
        assert!(point_in_rounded_rect(1.0, 20.0, rect, 18.0));
        // The exact corner is outside the rounded shape.
        assert!(!point_in_rounded_rect(0.5, 0.5, rect, 18.0));
        // Just inside the rounded edge is included.
        assert!(point_in_rounded_rect(18.0, 0.5, rect, 18.0));
    }

    #[test]
    fn radius_is_clamped_to_half_the_short_side() {
        let rect = Rect::new(0.0, 0.0, 120.0, 6.0);
        // The sliver is fully pill-shaped; its center is inside and its
        // extreme corners are out.
        assert!(point_in_rounded_rect(60.0, 3.0, rect, 100.0));
        assert!(!point_in_rounded_rect(0.5, 0.5, rect, 100.0));
    }

    #[test]
    fn negative_radius_is_a_plain_rectangle() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(point_in_rounded_rect(0.0, 0.0, rect, -4.0));
        assert!(point_in_rounded_rect(9.9, 9.9, rect, -4.0));
    }
}
