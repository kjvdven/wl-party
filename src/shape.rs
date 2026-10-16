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
