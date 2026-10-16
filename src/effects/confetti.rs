use std::error::Error;
use std::f32::consts::FRAC_PI_2;

use tiny_skia::PixmapMut;

use crate::effect::{Area, Effect, Size, Spec};
use crate::particle::{Particle, Swarm};

pub const SPEC: Spec = Spec {
    name: "confetti",
    about: "Two cannons fire from the bottom corners",
    build: |size, count| Box::new(Confetti::new(size, count)),
};

struct Confetti {
    swarm: Swarm,
    size: Size,
}

impl Confetti {
    fn new(size: Size, count: usize) -> Self {
        let mut swarm = Swarm::new(count, size.height * 1.4, 1.2);
        for i in 0..count {
            // Alternate cannons; aim up and inwards (~60-80 degrees) with some spread.
            let left = i % 2 == 0;
            let tilt = 0.18 + fastrand::f32() * 0.35;
            let angle = if left {
                -FRAC_PI_2 + tilt
            } else {
                -FRAC_PI_2 - tilt
            };
            let speed = size.height * (1.3 + fastrand::f32() * 0.9);
            let x = if left { 0.0 } else { size.width };
            swarm.particles.push(Particle::new(
                x,
                size.height,
                angle.cos() * speed,
                angle.sin() * speed,
            ));
        }
        Self { swarm, size }
    }
}

impl Effect for Confetti {
    fn update(&mut self, dt: f32) -> Result<bool, Box<dyn Error>> {
        self.swarm.update(dt, self.size);
        Ok(!self.swarm.is_empty())
    }

    fn draw(&self, canvas: &mut PixmapMut) -> Area {
        self.swarm.draw(canvas)
    }

    fn particles(&self) -> usize {
        self.swarm.particles.len()
    }
}
