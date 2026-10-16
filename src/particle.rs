use std::f32::consts::TAU;

use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, PixmapMut, Rect, Transform};

use crate::effect::{Area, Size};

/// Colors are given with red and blue swapped: tiny-skia writes RGBA bytes,
/// `wl_shm` `Argb8888` expects BGRA, so this lets us draw straight into the shm buffer.
const fn bgr(r: u8, g: u8, b: u8) -> [u8; 3] {
    [b, g, r]
}

const PALETTE: [[u8; 3]; 8] = [
    bgr(0xff, 0x4d, 0x6d),
    bgr(0xff, 0xb3, 0x47),
    bgr(0xff, 0xe0, 0x66),
    bgr(0x5c, 0xe1, 0xe6),
    bgr(0x7b, 0x61, 0xff),
    bgr(0x3d, 0xdc, 0x84),
    bgr(0xff, 0x7a, 0xc8),
    bgr(0xff, 0xff, 0xff),
];

/// Seconds before its fuse runs out that a particle starts fading.
const FADE_TIME: f32 = 0.4;

#[derive(Clone, Copy)]
pub struct Particle {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    /// Seconds left before the particle expires; infinite by default.
    pub fuse: f32,
    pub tag: u16,
    /// Index into the swarm's shapes; 0 is the confetti rectangle.
    pub shape: u16,
    angle: f32,
    spin: f32,
    flip: f32,
    flip_speed: f32,
    /// Sideways sway in pixels, applied when drawing so physics stays untouched.
    wobble: f32,
    wobble_phase: f32,
    wobble_speed: f32,
    width: f32,
    height: f32,
    color: Color,
}

impl Particle {
    pub fn new(x: f32, y: f32, vx: f32, vy: f32) -> Self {
        let [red, green, blue] = PALETTE[fastrand::usize(..PALETTE.len())];
        Self {
            x,
            y,
            vx,
            vy,
            fuse: f32::INFINITY,
            tag: 0,
            shape: 0,
            angle: fastrand::f32() * TAU,
            spin: (fastrand::f32() - 0.5) * 12.0,
            flip: fastrand::f32() * TAU,
            flip_speed: 4.0 + fastrand::f32() * 8.0,
            wobble: 0.0,
            wobble_phase: fastrand::f32() * TAU,
            wobble_speed: 1.0 + fastrand::f32() * 1.5,
            width: 8.0 + fastrand::f32() * 6.0,
            height: 4.0 + fastrand::f32() * 4.0,
            color: Color::from_rgba8(red, green, blue, 255),
        }
    }

    pub fn set_size(&mut self, size: f32) {
        self.width = size;
        self.height = size;
    }

    pub fn set_color(&mut self, [red, green, blue]: [u8; 3]) {
        let [first, second, third] = bgr(red, green, blue);
        self.color = Color::from_rgba8(first, second, third, 255);
    }

    /// Keeps the particle upright and flat, e.g. for balloons.
    pub fn stop_spin(&mut self) {
        self.angle = 0.0;
        self.spin = 0.0;
        self.flip = 0.0;
        self.flip_speed = 0.0;
    }

    pub fn set_wobble(&mut self, wobble: f32) {
        self.wobble = wobble;
    }

    pub fn step(&mut self, dt: f32, gravity: f32, drag: f32) {
        let damping = (1.0 - drag * dt).max(0.0);
        self.vx *= damping;
        self.vy = self.vy * damping + gravity * dt;
        self.x += self.vx * dt;
        self.y += self.vy * dt;
        self.angle += self.spin * dt;
        self.flip += self.flip_speed * dt;
        self.wobble_phase += self.wobble_speed * dt;
        self.fuse -= dt;
    }

    /// True once the particle has left the view and won't come back.
    /// Drag never reverses `vx`, and only upward gravity keeps a rising particle going.
    pub fn is_gone(&self, size: Size, gravity: f32) -> bool {
        let margin = self.width.max(self.height);
        (self.vy > 0.0 && self.y > size.height + margin)
            || (self.vy < 0.0 && gravity <= 0.0 && self.y < -margin)
            || (self.vx > 0.0 && self.x > size.width + margin)
            || (self.vx < 0.0 && self.x < -margin)
    }
}

/// A set of particles sharing physics and cached unit-sized shapes.
pub struct Swarm {
    pub particles: Vec<Particle>,
    /// Particles whose fuse ran out during the last `update`.
    pub expired: Vec<Particle>,
    pub gravity: f32,
    pub drag: f32,
    /// Shapes centered on the origin and fitting a 1x1 box; index 0 is the unit square.
    pub shapes: Vec<Path>,
}

impl Swarm {
    pub fn new(capacity: usize, gravity: f32, drag: f32) -> Self {
        let unit = Rect::from_xywh(-0.5, -0.5, 1.0, 1.0).expect("valid unit rect");
        Self {
            particles: Vec::with_capacity(capacity),
            expired: Vec::new(),
            gravity,
            drag,
            shapes: vec![PathBuilder::from_rect(unit)],
        }
    }

    /// Steps all particles, moves expired ones to `expired` and drops the ones that left the screen.
    pub fn update(&mut self, dt: f32, size: Size) {
        self.expired.clear();
        for particle in &mut self.particles {
            particle.step(dt, self.gravity, self.drag);
        }
        let (expired, gravity) = (&mut self.expired, self.gravity);
        self.particles.retain(|particle| {
            if particle.fuse <= 0.0 {
                expired.push(*particle);
                return false;
            }
            !particle.is_gone(size, gravity)
        });
    }

    /// Draws the particles that are in view and returns the area they cover.
    pub fn draw(&self, canvas: &mut PixmapMut) -> Area {
        let mut paint = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        #[allow(clippy::cast_precision_loss)] // screen sizes fit easily in f32
        let (canvas_width, canvas_height) = (canvas.width() as f32, canvas.height() as f32);
        let (mut left, mut top, mut right, mut bottom) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for particle in &self.particles {
            let (sway, tilt) = particle.wobble_phase.sin_cos();
            let x = particle.x + sway * particle.wobble;
            let y = particle.y;
            // Half the diagonal of the particle's box, plus a pixel of anti-aliasing.
            let reach = particle.width.max(particle.height) * 0.75 + 1.0;
            if x + reach < 0.0
                || y + reach < 0.0
                || x - reach > canvas_width
                || y - reach > canvas_height
            {
                continue;
            }
            left = left.min(x - reach);
            top = top.min(y - reach);
            right = right.max(x + reach);
            bottom = bottom.max(y + reach);

            let mut color = particle.color;
            color.apply_opacity(particle.fuse / FADE_TIME);
            paint.set_color(color);
            // scale -> rotate -> translate in one matrix; cos(flip) gives the paper-tumble look.
            // A slight tilt with the sway, like a balloon pulled by its string.
            let angle = particle.angle + tilt * (particle.wobble * 0.01).min(0.2);
            let (sin, cos) = angle.sin_cos();
            let width = particle.width;
            let height = particle.height * particle.flip.cos();
            let transform =
                Transform::from_row(width * cos, width * sin, -height * sin, height * cos, x, y);
            let shape = self
                .shapes
                .get(usize::from(particle.shape))
                .unwrap_or(&self.shapes[0]);
            canvas.fill_path(shape, &paint, FillRule::Winding, transform, None);
        }
        // Clamped to the canvas first, so the casts can't truncate or wrap.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let px = |value: f32, max: f32| value.clamp(0.0, max) as u32;
        Area {
            left: px(left.floor(), canvas_width),
            top: px(top.floor(), canvas_height),
            right: px(right.ceil(), canvas_width),
            bottom: px(bottom.ceil(), canvas_height),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }
}
