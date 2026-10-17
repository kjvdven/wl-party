use std::cell::RefCell;
use std::collections::HashMap;
use std::f32::consts::TAU;

use tiny_skia::{
    Color, FillRule, FilterQuality, Paint, Path, PathBuilder, Pixmap, PixmapMut, PixmapPaint, Rect,
    Transform,
};

use crate::effect::{Area, Size};

/// Colors are given with red and blue swapped: tiny-skia writes RGBA bytes,
/// `wl_shm` `Argb8888` expects BGRA, so this lets us draw straight into the shm buffer.
const fn bgr(r: u8, g: u8, b: u8) -> [u8; 3] {
    [b, g, r]
}

fn color([red, green, blue, alpha]: [u8; 4]) -> Color {
    let [first, second, third] = bgr(red, green, blue);
    Color::from_rgba8(first, second, third, alpha)
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
const SHADOW_OPACITY: f32 = 90.0 / 255.0;
/// Shadow offset as a fraction of the particle's size.
const SHADOW_OFFSET: f32 = 0.08;
const GLOSS_ALPHA: u8 = 110;
/// Sprite cache bounds: at most 256 sprites of up to 128 px, about 17 MB.
const MAX_SPRITES: usize = 256;
const MAX_SPRITE_SIZE: f32 = 128.0;

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
    /// A light highlight in the top left, like on a balloon.
    pub gloss: bool,
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
    shadow: Option<Color>,
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
            gloss: false,
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
            shadow: None,
        }
    }

    pub fn set_size(&mut self, size: f32) {
        self.width = size;
        self.height = size;
    }

    pub fn set_color(&mut self, rgba: [u8; 4]) {
        self.color = color(rgba);
    }

    /// A translucent copy drawn behind the particle, offset down and right.
    pub fn set_shadow(&mut self, rgba: [u8; 4]) {
        let mut shadow = color(rgba);
        shadow.apply_opacity(SHADOW_OPACITY);
        self.shadow = Some(shadow);
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
    /// Custom shapes pre-rendered per size and color; rasterizing a complex path every frame is slow.
    sprites: RefCell<HashMap<SpriteKey, Sprite>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SpriteKey {
    shape: u16,
    width: u16,
    height: u16,
    color: [u8; 4],
    shadow: Option<[u8; 4]>,
    gloss: bool,
}

impl SpriteKey {
    fn of(particle: &Particle) -> Self {
        let rgba = |color: Color| {
            let color = color.to_color_u8();
            [color.red(), color.green(), color.blue(), color.alpha()]
        };
        // Sizes are capped at MAX_SPRITE_SIZE before a key is made, so they fit u16.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let px = |size: f32| size.round().max(1.0) as u16;
        Self {
            shape: particle.shape,
            width: px(particle.width),
            height: px(particle.height),
            color: rgba(particle.color),
            shadow: particle.shadow.map(rgba),
            gloss: particle.gloss,
        }
    }
}

/// Fills the particle's shadow, shape and gloss with `transform`, faded by `opacity`.
fn fill_layers(
    canvas: &mut PixmapMut,
    particle: &Particle,
    shape: &Path,
    transform: Transform,
    offset: f32,
    opacity: f32,
) {
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    let mut fill = |path: &Path, mut color: Color, transform: Transform| {
        color.apply_opacity(opacity);
        paint.set_color(color);
        canvas.fill_path(path, &paint, FillRule::Winding, transform, None);
    };
    if let Some(shadow) = particle.shadow {
        fill(shape, shadow, transform.post_translate(offset, offset));
    }
    fill(shape, particle.color, transform);
    if particle.gloss
        && let Some(gloss) =
            Rect::from_xywh(-0.3, -0.38, 0.14, 0.22).and_then(PathBuilder::from_oval)
    {
        fill(&gloss, color([255, 255, 255, GLOSS_ALPHA]), transform);
    }
}

/// A shape with its shadow and gloss rendered upright around `center`.
struct Sprite {
    pixmap: Pixmap,
    center: f32,
}

impl Sprite {
    fn render(shape: &Path, key: SpriteKey, particle: &Particle) -> Option<Self> {
        let (width, height) = (f32::from(key.width), f32::from(key.height));
        let extent = width.max(height);
        let offset = key
            .shadow
            .map_or(0.0, |_| (extent * SHADOW_OFFSET).max(1.0));
        let center = (extent / 2.0 + 1.0).ceil();
        // Fits in u32: extent is at most MAX_SPRITE_SIZE.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let side = (center * 2.0 + offset).ceil() as u32;
        let mut pixmap = Pixmap::new(side, side)?;
        let transform = Transform::from_row(width, 0.0, 0.0, height, center, center);
        fill_layers(
            &mut pixmap.as_mut(),
            particle,
            shape,
            transform,
            offset,
            1.0,
        );
        Some(Self { pixmap, center })
    }
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
            sprites: RefCell::default(),
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
        #[allow(clippy::cast_precision_loss)] // screen sizes fit easily in f32
        let (canvas_width, canvas_height) = (canvas.width() as f32, canvas.height() as f32);
        let (mut left, mut top, mut right, mut bottom) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for particle in &self.particles {
            let (sway, tilt) = particle.wobble_phase.sin_cos();
            let x = particle.x + sway * particle.wobble;
            let y = particle.y;
            let extent = particle.width.max(particle.height);
            let offset = particle
                .shadow
                .map_or(0.0, |_| (extent * SHADOW_OFFSET).max(1.0));
            // Half the diagonal of the particle's box, plus its shadow and a pixel of anti-aliasing.
            let reach = extent * 0.75 + offset + 1.0;
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

            // A slight tilt with the sway, like a balloon pulled by its string.
            let angle = particle.angle + tilt * (particle.wobble * 0.01).min(0.2);
            let (sin, cos) = angle.sin_cos();
            let flip = particle.flip.cos();
            let fade = particle.fuse / FADE_TIME;
            let shape = self
                .shapes
                .get(usize::from(particle.shape))
                .unwrap_or(&self.shapes[0]);
            if particle.shape != 0 && extent <= MAX_SPRITE_SIZE {
                // rotate -> squash by flip -> translate, around the sprite's center.
                let transform = Transform::from_row(cos, sin, -sin * flip, cos * flip, x, y);
                if self.draw_sprite(canvas, particle, shape, transform, fade) {
                    continue;
                }
            }

            // scale -> rotate -> translate in one matrix; cos(flip) gives the paper-tumble look.
            let width = particle.width;
            let height = particle.height * flip;
            let transform =
                Transform::from_row(width * cos, width * sin, -height * sin, height * cos, x, y);
            fill_layers(canvas, particle, shape, transform, offset, fade);
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

    /// Draws the particle from its cached sprite; `false` if the cache is full.
    fn draw_sprite(
        &self,
        canvas: &mut PixmapMut,
        particle: &Particle,
        shape: &Path,
        transform: Transform,
        fade: f32,
    ) -> bool {
        let key = SpriteKey::of(particle);
        let mut sprites = self.sprites.borrow_mut();
        if !sprites.contains_key(&key) {
            if sprites.len() >= MAX_SPRITES {
                return false;
            }
            let Some(sprite) = Sprite::render(shape, key, particle) else {
                return false;
            };
            sprites.insert(key, sprite);
        }
        let sprite = &sprites[&key];
        let paint = PixmapPaint {
            opacity: fade.clamp(0.0, 1.0),
            quality: FilterQuality::Bilinear,
            ..PixmapPaint::default()
        };
        let transform = transform.pre_translate(-sprite.center, -sprite.center);
        canvas.draw_pixmap(0, 0, sprite.pixmap.as_ref(), &paint, transform, None);
        true
    }

    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Size = Size {
        width: 800.0,
        height: 600.0,
    };

    fn at(x: f32, y: f32, vx: f32, vy: f32) -> Particle {
        Particle::new(x, y, vx, vy)
    }

    #[test]
    fn gone_once_past_an_edge_and_moving_away() {
        assert!(at(400.0, 700.0, 0.0, 10.0).is_gone(SIZE, 100.0));
        assert!(at(-100.0, 300.0, -10.0, 0.0).is_gone(SIZE, 100.0));
        assert!(at(900.0, 300.0, 10.0, 0.0).is_gone(SIZE, 100.0));
    }

    #[test]
    fn kept_while_coming_back() {
        assert!(!at(400.0, 700.0, 0.0, -10.0).is_gone(SIZE, 100.0));
        assert!(!at(-100.0, 300.0, 10.0, 0.0).is_gone(SIZE, 100.0));
        assert!(!at(400.0, 300.0, 0.0, 10.0).is_gone(SIZE, 100.0));
    }

    #[test]
    fn above_the_top_depends_on_gravity() {
        let rising = at(400.0, -100.0, 0.0, -10.0);
        assert!(!rising.is_gone(SIZE, 100.0), "gravity pulls it back down");
        assert!(rising.is_gone(SIZE, -100.0), "a balloon never comes back");
    }

    #[test]
    fn expired_particles_move_to_expired() {
        let mut swarm = Swarm::new(2, 0.0, 0.0);
        let mut short = at(100.0, 100.0, 0.0, 0.0);
        short.fuse = 0.01;
        swarm.particles.extend([short, at(200.0, 100.0, 0.0, 0.0)]);
        swarm.update(0.1, SIZE);
        assert_eq!(swarm.particles.len(), 1);
        assert_eq!(swarm.expired.len(), 1);
    }
}
