/// Particle system for bioluminescent visual effects.

#[derive(Debug, Clone)]
pub struct Particle {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) vx: f32,
    pub(crate) vy: f32,
    pub(crate) life: f32,
    pub(crate) max_life: f32,
    pub(crate) size: f32,
    pub(crate) hue_shift: f32,
}

pub struct ParticleSystem {
    particles: Vec<Particle>,
    max_particles: usize,
}

impl ParticleSystem {
    pub fn new(max_particles: usize) -> Self {
        Self {
            particles: Vec::with_capacity(max_particles),
            max_particles,
        }
    }

    pub fn spawn(&mut self, x: f32, y: f32, intensity: f32) {
        if self.particles.len() >= self.max_particles {
            return;
        }
        let hash = ((x * 1000.0 + y * 7.0) as u32).wrapping_mul(2654435761);
        let rand01 = (hash % 1000) as f32 / 1000.0;

        self.particles.push(Particle {
            x,
            y,
            vx: (rand01 - 0.5) * 0.3,
            vy: -(0.3 + rand01 * 0.5) * intensity,
            life: 1.0,
            max_life: 2.0 + rand01 * 1.5,
            size: 2.0 + rand01 * 3.0,
            hue_shift: rand01 * 0.3 - 0.15,
        });
    }

    pub fn update(&mut self, dt: f32) {
        for p in &mut self.particles {
            p.x += p.vx;
            p.y += p.vy;
            p.life -= dt / p.max_life;
            p.vx *= 0.995;
            p.vy *= 0.998;
        }
        self.particles.retain(|p| p.life > 0.0);
    }

    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }

    pub fn clear(&mut self) {
        self.particles.clear();
    }
}
