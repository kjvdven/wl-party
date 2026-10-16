use std::error::Error;

use tiny_skia::PixmapMut;

use crate::effect::{Area, Effect, Size, Spec};
use crate::particle::{Particle, Swarm};

pub const SPEC: Spec = Spec {
    name: "rain",
    about: "Confetti gently rains down from the top",
    build: |size, count| Box::new(Rain::new(size, count)),
};

/// Seconds over which all particles are released.
const SPAWN_TIME: f32 = 2.0;

struct Rain {
    swarm: Swarm,
    size: Size,
    elapsed: f32,
    to_spawn: usize,
    total: usize,
}

impl Rain {
    fn new(size: Size, count: usize) -> Self {
        Self {
            swarm: Swarm::new(count, size.height * 0.6, 1.8),
            size,
            elapsed: 0.0,
            to_spawn: count,
            total: count,
        }
    }

    fn spawn(&mut self, dt: f32) {
        self.elapsed += dt;
        // Progress is clamped to 0..=1 and total is at most 5000, so the casts are lossless.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let due = ((self.elapsed / SPAWN_TIME).min(1.0) * self.total as f32) as usize;
        let spawned = self.total - self.to_spawn;
        for _ in spawned..due {
            let x = fastrand::f32() * self.size.width;
            let vx = (fastrand::f32() - 0.5) * 120.0;
            let vy = 60.0 + fastrand::f32() * 120.0;
            self.swarm.particles.push(Particle::new(x, -20.0, vx, vy));
        }
        self.to_spawn = self.total - due;
    }
}

impl Effect for Rain {
    fn update(&mut self, dt: f32) -> Result<bool, Box<dyn Error>> {
        self.spawn(dt);
        self.swarm.update(dt, self.size);
        Ok(self.to_spawn > 0 || !self.swarm.is_empty())
    }

    fn draw(&self, canvas: &mut PixmapMut) -> Area {
        self.swarm.draw(canvas)
    }

    fn particles(&self) -> usize {
        self.swarm.particles.len()
    }
}
