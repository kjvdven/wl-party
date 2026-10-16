use svgtypes::{SimplePathSegment, SimplifyingPathParser};
use tiny_skia::{Path, PathBuilder, Transform};

pub fn circle() -> Path {
    PathBuilder::from_circle(0.0, 0.0, 0.5).expect("valid circle")
}

/// Parses SVG path data (the `d` attribute) and fits it, centered, into a 1x1 box.
pub fn parse(data: &str) -> Option<Path> {
    // SVG coordinates are f64; f32 is plenty for on-screen shapes.
    #[allow(clippy::cast_possible_truncation)]
    let f = |value: f64| value as f32;
    let mut builder = PathBuilder::new();
    for segment in SimplifyingPathParser::from(data) {
        match segment.ok()? {
            SimplePathSegment::MoveTo { x, y } => builder.move_to(f(x), f(y)),
            SimplePathSegment::LineTo { x, y } => builder.line_to(f(x), f(y)),
            SimplePathSegment::Quadratic { x1, y1, x, y } => {
                builder.quad_to(f(x1), f(y1), f(x), f(y));
            }
            SimplePathSegment::CurveTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => builder.cubic_to(f(x1), f(y1), f(x2), f(y2), f(x), f(y)),
            SimplePathSegment::ClosePath => builder.close(),
        }
    }
    let path = builder.finish()?;
    let bounds = path.bounds();
    let scale = 1.0 / bounds.width().max(bounds.height());
    if !scale.is_finite() {
        return None;
    }
    let center_x = bounds.left() + bounds.width() / 2.0;
    let center_y = bounds.top() + bounds.height() / 2.0;
    path.transform(Transform::from_translate(-center_x, -center_y).post_scale(scale, scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(path: &Path) -> (f32, f32) {
        let bounds = path.bounds();
        (bounds.width(), bounds.height())
    }

    #[test]
    fn fits_a_unit_box_around_the_origin() {
        let path = parse("M100 100 L300 100 L300 200 L100 200 Z").unwrap();
        let bounds = path.bounds();
        assert!((bounds.width() - 1.0).abs() < 1e-5);
        assert!((bounds.height() - 0.5).abs() < 1e-5);
        assert!((bounds.left() + bounds.right()).abs() < 1e-5);
        assert!((bounds.top() + bounds.bottom()).abs() < 1e-5);
    }

    #[test]
    fn supports_curves_and_arcs() {
        let heart =
            "M12 21 C12 21 3 14 3 8 A4.5 4.5 0 0 1 12 6 A4.5 4.5 0 0 1 21 8 C21 14 12 21 12 21 Z";
        let (width, height) = size(&parse(heart).unwrap());
        assert!((width.max(height) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn rejects_invalid_or_empty_paths() {
        for bad in ["", "nonsense", "M0 0 Q nonsense", "M5 5", "M0 0 L0 0 Z"] {
            assert!(parse(bad).is_none(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn circle_fits_a_unit_box() {
        let (width, height) = size(&circle());
        assert!((width - 1.0).abs() < 1e-5 && (height - 1.0).abs() < 1e-5);
    }
}
