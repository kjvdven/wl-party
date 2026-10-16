use std::error::Error;

use tiny_skia::PixmapMut;

use crate::effects;

/// Logical size of the overlay in pixels.
#[derive(Clone, Copy)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

/// Pixel area `left..right` by `top..bottom`; empty when `right <= left` or `bottom <= top`.
#[derive(Clone, Copy, Default)]
pub struct Area {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl Area {
    pub fn full(width: u32, height: u32) -> Self {
        Self {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        }
    }

    pub fn is_empty(self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }
}

/// A visual effect, stepped once per compositor frame.
pub trait Effect {
    /// Advances the simulation by `dt` seconds. Returns `Ok(false)` once finished.
    fn update(&mut self, dt: f32) -> Result<bool, Box<dyn Error>>;

    /// Draws the current state onto a cleared, transparent canvas and returns the area it touched.
    fn draw(&self, canvas: &mut PixmapMut) -> Area;

    /// Number of live particles.
    fn particles(&self) -> usize;
}

/// Registry entry: how to build an effect by name.
pub struct Spec {
    pub name: &'static str,
    pub about: &'static str,
    pub build: fn(Size, usize) -> Box<dyn Effect>,
}

/// Builds an effect once an overlay knows its size; called once per screen.
pub type Build = Box<dyn Fn(Size) -> Box<dyn Effect>>;

/// All built-in effects. Adding one = new file in `effects/` + one line here.
pub const REGISTRY: &[Spec] = &[effects::confetti::SPEC, effects::rain::SPEC];

pub fn find(name: &str) -> Option<&'static Spec> {
    REGISTRY.iter().find(|spec| spec.name == name)
}
