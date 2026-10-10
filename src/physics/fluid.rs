//! Equal-mass 3D position-based fluids (PBF), implemented with existing glam SIMD.
//! Poly6 density, Spiky gradients, compression-only density constraints and
//! symmetric XSPH viscosity. No fluid/physics package, threads or GPU required.
//! Reference: Macklin & Müller, Position Based Fluids (2013), equations 1–12.
//! https://matthias-research.github.io/pages/publications/pbf_sig_preprint.pdf
//!
//! Rigid bodies are one-way moving boundaries; this does not exert buoyancy or
//! pressure back onto them. No boundary ghost particles, surface reconstruction,
//! vorticity confinement or engineering CFD accuracy is claimed.
use super::{Body, PhysicsError, Shape};
use glam::Vec3A;
use std::{collections::HashMap, f32::consts::PI, ops::Range, time::Instant};

#[derive(Clone, Copy, Debug)]
pub struct FluidConfig {
    pub bounds_min: Vec3A,
    pub bounds_max: Vec3A,
    pub particle_radius: f32,
    pub smoothing_radius: f32,
    pub particle_mass: f32,
    pub rest_density: f32,
    pub gravity: Vec3A,
    /// Diffusion relaxation rate in inverse seconds; zero disables viscosity.
    pub viscosity: f32,
    pub solver_iterations: usize,
    pub substeps: usize,
    pub max_substeps: usize,
    /// CFM epsilon for the density constraint denominator, in inverse m².
    pub relaxation: f32,
    pub boundary_restitution: f32,
    pub boundary_friction: f32,
    /// Bounded speed for pathological inputs/position corrections; hits are counted.
    pub max_speed: f32,
}
impl Default for FluidConfig {
    fn default() -> Self {
        Self {
            bounds_min: Vec3A::new(-2., 0., -2.),
            bounds_max: Vec3A::new(2., 3., 2.),
            particle_radius: 0.05,
            smoothing_radius: 0.2,
            particle_mass: 1.,
            rest_density: 1000.,
            gravity: Vec3A::new(0., -9.81, 0.),
            viscosity: 2.,
            solver_iterations: 4,
            substeps: 2,
            max_substeps: 16,
            relaxation: 1e-4,
            boundary_restitution: 0.,
            boundary_friction: 0.05,
            max_speed: 100.,
        }
    }
}
impl FluidConfig {
    fn validate(self) -> Result<(), PhysicsError> {
        let finite_range = |v: f32, a, b| v.is_finite() && (a..=b).contains(&v);
        if !self.bounds_min.is_finite()
            || !self.bounds_max.is_finite()
            || self.bounds_min.abs().max_element() > 100_000.
            || self.bounds_max.abs().max_element() > 100_000.
            || !finite_range(self.particle_radius, 0.005, 1.)
            || !finite_range(self.smoothing_radius, self.particle_radius * 2., 4.)
            || (self.bounds_max - self.bounds_min).min_element() <= self.particle_radius * 2.
            || !finite_range(self.particle_mass, 1e-6, 100_000.)
            || !finite_range(self.rest_density, 1., 100_000.)
            || !self.gravity.is_finite()
            || self.gravity.abs().max_element() > 1000.
            || !finite_range(self.viscosity, 0., 1000.)
            || !(1..=16).contains(&self.solver_iterations)
            || !(1..=16).contains(&self.max_substeps)
            || !(1..=self.max_substeps).contains(&self.substeps)
            || !finite_range(self.relaxation, 1e-8, 10_000.)
            || !finite_range(self.boundary_restitution, 0., 1.)
            || !finite_range(self.boundary_friction, 0., 1.)
            || !finite_range(self.max_speed, 0.1, 1000.)
        {
            return Err(PhysicsError("Invalid fluid domain or solver configuration"));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug)]
pub struct ParticleDesc {
    pub position: Vec3A,
    pub velocity: Vec3A,
}
impl ParticleDesc {
    pub fn new(position: Vec3A) -> Self {
        Self {
            position,
            velocity: Vec3A::ZERO,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Particle {
    position: Vec3A,
    previous: Vec3A,
    velocity: Vec3A,
    density: f32,
}
impl Particle {
    pub fn position(&self) -> Vec3A {
        self.position
    }
    pub fn velocity(&self) -> Vec3A {
        self.velocity
    }
    pub fn density(&self) -> f32 {
        self.density
    }
    pub fn interpolated_position(&self, alpha: f32) -> Vec3A {
        self.previous.lerp(
            self.position,
            if alpha.is_finite() {
                alpha.clamp(0., 1.)
            } else {
                1.
            },
        )
    }
}
#[derive(Default, Clone, Debug)]
pub struct FluidStats {
    pub steps: u64,
    pub substeps: u64,
    pub motion_limit_hits: u64,
    pub speed_limit_hits: u64,
    pub correction_limit_hits: u64,
    pub neighbor_rebuilds: usize,
    /// Across all density iterations/substeps of the last tick, not unique edges.
    pub neighbor_pairs: usize,
    pub grid_candidates: usize,
    pub boundary_contacts: usize,
    pub max_compression_before: f32,
    pub max_compression_after: f32,
    pub min_density: f32,
    pub max_density: f32,
    pub max_speed: f32,
    pub last_step_ms: f64,
    pub total_step_ms: f64,
}

struct Kernel {
    h: f32,
    h2: f32,
    poly6: f32,
    spiky: f32,
}
impl Kernel {
    fn new(h: f32) -> Self {
        Self {
            h,
            h2: h * h,
            poly6: 315. / (64. * PI * h.powi(9)),
            spiky: -45. / (PI * h.powi(6)),
        }
    }
    fn weight(&self, r2: f32) -> f32 {
        if r2 >= self.h2 {
            0.
        } else {
            self.poly6 * (self.h2 - r2).powi(3)
        }
    }
    fn gradient(&self, delta: Vec3A) -> Vec3A {
        let r = delta.length();
        if r <= 1e-7 || r >= self.h {
            Vec3A::ZERO
        } else {
            delta * (self.spiky * (self.h - r).powi(2) / r)
        }
    }
}
#[derive(Clone, Copy)]
struct Pair {
    i: usize,
    j: usize,
    weight: f32,
    gradient: Vec3A,
}
#[derive(Clone, Copy)]
struct SolidContact {
    particle: usize,
    normal: Vec3A,
    velocity: Vec3A,
}

/// Equal-mass append-only particle system. Insert before starting a replay; adding
/// particles changes total mass by `config.particle_mass` per particle.
pub struct Fluid {
    config: FluidConfig,
    kernel: Kernel,
    particles: Vec<Particle>,
    predicted: Vec<Vec3A>,
    ballistic: Vec<Vec3A>,
    incoming_velocity: Vec<Vec3A>,
    deltas: Vec<Vec3A>,
    gradient_sum: Vec<Vec3A>,
    gradient_squared: Vec<f32>,
    lambdas: Vec<f32>,
    weight_sum: Vec<f32>,
    entries: Vec<([i32; 3], usize)>,
    cells: HashMap<[i32; 3], Range<usize>>,
    pairs: Vec<Pair>,
    solid_contacts: Vec<SolidContact>,
    stats: FluidStats,
}
impl Fluid {
    pub fn new(config: FluidConfig) -> Result<Self, PhysicsError> {
        config.validate()?;
        Ok(Self {
            kernel: Kernel::new(config.smoothing_radius),
            config,
            particles: vec![],
            predicted: vec![],
            ballistic: vec![],
            incoming_velocity: vec![],
            deltas: vec![],
            gradient_sum: vec![],
            gradient_squared: vec![],
            lambdas: vec![],
            weight_sum: vec![],
            entries: vec![],
            cells: HashMap::new(),
            pairs: vec![],
            solid_contacts: vec![],
            stats: FluidStats::default(),
        })
    }
    pub fn config(&self) -> FluidConfig {
        self.config
    }
    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }
    pub fn stats(&self) -> &FluidStats {
        &self.stats
    }
    pub fn total_mass(&self) -> f64 {
        self.particles.len() as f64 * self.config.particle_mass as f64
    }
    pub fn insert(&mut self, desc: ParticleDesc) -> Result<usize, PhysicsError> {
        let lo = self.config.bounds_min + Vec3A::splat(self.config.particle_radius);
        let hi = self.config.bounds_max - Vec3A::splat(self.config.particle_radius);
        if !desc.position.is_finite()
            || !desc.velocity.is_finite()
            || desc.velocity.length() > self.config.max_speed
            || !desc.position.cmpge(lo).all()
            || !desc.position.cmple(hi).all()
        {
            return Err(PhysicsError("Invalid fluid particle position or velocity"));
        }
        let id = self.particles.len();
        self.particles.push(Particle {
            position: desc.position,
            previous: desc.position,
            velocity: desc.velocity,
            density: self.config.particle_mass * self.kernel.weight(0.),
        });
        self.predicted.push(desc.position);
        self.ballistic.push(desc.position);
        self.incoming_velocity.push(desc.velocity);
        self.deltas.push(Vec3A::ZERO);
        self.gradient_sum.push(Vec3A::ZERO);
        self.gradient_squared.push(0.);
        self.lambdas.push(0.);
        self.weight_sum.push(0.);
        Ok(id)
    }
    /// Applies a uniform velocity change, validating every particle before mutation.
    pub fn add_velocity(&mut self, delta: Vec3A) -> Result<(), PhysicsError> {
        if !delta.is_finite()
            || self
                .particles
                .iter()
                .any(|p| (p.velocity + delta).length() > self.config.max_speed)
        {
            return Err(PhysicsError("Fluid impulse exceeds finite speed limit"));
        }
        for p in &mut self.particles {
            p.velocity += delta;
        }
        Ok(())
    }
    /// Standalone tick. Inside World, World::step supplies its timestep/gravity.
    pub fn step(&mut self, dt: f32) -> Result<(), PhysicsError> {
        if !dt.is_finite() || !(0.001..=0.05).contains(&dt) {
            return Err(PhysicsError("Invalid fluid timestep"));
        }
        self.step_in_world(dt, self.config.gravity, &[]);
        Ok(())
    }
    pub(super) fn step_in_world(&mut self, dt: f32, gravity: Vec3A, bodies: &[Body]) {
        let start = Instant::now();
        self.stats.neighbor_pairs = 0;
        self.stats.grid_candidates = 0;
        self.stats.boundary_contacts = 0;
        self.stats.neighbor_rebuilds = 0;
        self.stats.max_compression_before = 0.;
        self.stats.max_compression_after = 0.;
        let speed = self
            .particles
            .iter()
            .map(|p| p.velocity.length())
            .fold(0., f32::max);
        let requested = (((speed + gravity.length() * dt) * dt
            / (self.config.particle_radius * 0.5))
            .ceil() as usize)
            .max(self.config.substeps);
        if requested > self.config.max_substeps {
            self.stats.motion_limit_hits += 1;
        }
        let substeps = requested.min(self.config.max_substeps);
        for p in &mut self.particles {
            p.previous = p.position;
        }
        for _ in 0..substeps {
            self.substep(dt / substeps as f32, gravity, bodies);
        }
        self.stats.steps += 1;
        self.stats.substeps += substeps as u64;
        self.stats.min_density = self
            .particles
            .iter()
            .map(|p| p.density)
            .reduce(f32::min)
            .unwrap_or(0.);
        self.stats.max_density = self.particles.iter().map(|p| p.density).fold(0., f32::max);
        self.stats.max_speed = self
            .particles
            .iter()
            .map(|p| p.velocity.length())
            .fold(0., f32::max);
        self.stats.last_step_ms = start.elapsed().as_secs_f64() * 1000.;
        self.stats.total_step_ms += self.stats.last_step_ms;
    }
    fn substep(&mut self, dt: f32, gravity: Vec3A, bodies: &[Body]) {
        for (i, p) in self.particles.iter_mut().enumerate() {
            p.velocity += gravity * dt;
            self.incoming_velocity[i] = p.velocity;
            self.predicted[i] = p.position + p.velocity * dt;
            self.ballistic[i] = self.predicted[i];
        }
        self.project_boundaries(bodies);
        let mut neighbors_current = false;
        for iteration in 0..self.config.solver_iterations {
            self.find_neighbors();
            self.compute_density();
            neighbors_current = true;
            if iteration == 0 {
                self.stats.max_compression_before =
                    self.stats.max_compression_before.max(self.compression());
            }
            // Compression-only constraints are all exactly zero here. Skip the
            // remaining projections and retain this already correct graph.
            if self.compression() == 0. {
                break;
            }
            self.compute_lambdas();
            self.correct_positions();
            self.project_boundaries(bodies);
            neighbors_current = false;
        }
        // Rebuild after projection: newly entering neighbours must not be omitted
        // from viscosity or reported density. Buffers retain capacity across ticks.
        if !neighbors_current {
            self.find_neighbors();
            self.compute_density();
        }
        self.stats.max_compression_after = self.stats.max_compression_after.max(self.compression());
        for (i, p) in self.particles.iter_mut().enumerate() {
            // Apply only the constraint displacement. Reconstructing the entire
            // velocity from f32 position differences accumulates roundoff drift
            // even for an isolated particle in exact ballistic free fall.
            p.velocity += (self.predicted[i] - self.ballistic[i]) / dt;
            p.position = self.predicted[i];
        }
        self.apply_viscosity(dt);
        self.respond_to_boundaries();
        for p in &mut self.particles {
            if p.velocity.length_squared() > self.config.max_speed.powi(2) {
                p.velocity = p.velocity.clamp_length_max(self.config.max_speed);
                self.stats.speed_limit_hits += 1;
            }
        }
    }
    fn cell(&self, p: Vec3A) -> [i32; 3] {
        let v = (p / self.kernel.h).floor();
        [v.x as i32, v.y as i32, v.z as i32]
    }
    fn find_neighbors(&mut self) {
        self.entries.clear();
        self.cells.clear();
        self.pairs.clear();
        for (i, &p) in self.predicted.iter().enumerate() {
            self.entries.push((self.cell(p), i));
        }
        self.entries.sort_unstable();
        let mut start = 0;
        while start < self.entries.len() {
            let cell = self.entries[start].0;
            let mut end = start + 1;
            while end < self.entries.len() && self.entries[end].0 == cell {
                end += 1;
            }
            self.cells.insert(cell, start..end);
            start = end;
        }
        for i in 0..self.predicted.len() {
            let cell = self.cell(self.predicted[i]);
            for x in -1..=1 {
                for y in -1..=1 {
                    for z in -1..=1 {
                        if let Some(range) =
                            self.cells.get(&[cell[0] + x, cell[1] + y, cell[2] + z])
                        {
                            for entry in &self.entries[range.clone()] {
                                let j = entry.1;
                                if j <= i {
                                    continue;
                                }
                                self.stats.grid_candidates += 1;
                                let delta = self.predicted[i] - self.predicted[j];
                                let r2 = delta.length_squared();
                                if r2 < self.kernel.h2 {
                                    self.pairs.push(Pair {
                                        i,
                                        j,
                                        weight: self.kernel.weight(r2),
                                        gradient: self.kernel.gradient(delta),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        self.stats.neighbor_rebuilds += 1;
        self.stats.neighbor_pairs += self.pairs.len();
    }
    fn compute_density(&mut self) {
        let mass = self.config.particle_mass;
        for p in &mut self.particles {
            p.density = mass * self.kernel.weight(0.);
        }
        for pair in &self.pairs {
            let density = mass * pair.weight;
            self.particles[pair.i].density += density;
            self.particles[pair.j].density += density;
        }
    }
    fn compression(&self) -> f32 {
        self.particles
            .iter()
            .map(|p| (p.density / self.config.rest_density - 1.).max(0.))
            .fold(0., f32::max)
    }
    fn compute_lambdas(&mut self) {
        self.gradient_sum.fill(Vec3A::ZERO);
        self.gradient_squared.fill(0.);
        let scale = self.config.particle_mass / self.config.rest_density;
        for pair in &self.pairs {
            let grad = pair.gradient * scale;
            self.gradient_sum[pair.i] += grad;
            self.gradient_sum[pair.j] -= grad;
            let length2 = grad.length_squared();
            self.gradient_squared[pair.i] += length2;
            self.gradient_squared[pair.j] += length2;
        }
        for (i, p) in self.particles.iter().enumerate() {
            let constraint = (p.density / self.config.rest_density - 1.).max(0.);
            self.lambdas[i] = -constraint
                / (self.gradient_sum[i].length_squared()
                    + self.gradient_squared[i]
                    + self.config.relaxation);
        }
    }
    fn correct_positions(&mut self) {
        self.deltas.fill(Vec3A::ZERO);
        let scale = self.config.particle_mass / self.config.rest_density;
        for pair in &self.pairs {
            let delta = pair.gradient * ((self.lambdas[pair.i] + self.lambdas[pair.j]) * scale);
            self.deltas[pair.i] += delta;
            self.deltas[pair.j] -= delta;
        }
        let largest = self.deltas.iter().map(|v| v.length()).fold(0., f32::max);
        let limit = self.kernel.h * 0.2;
        // A global factor preserves equal/opposite pair corrections (unlike
        // independently clipping particles, which would inject net momentum).
        let factor = if largest > limit {
            self.stats.correction_limit_hits += 1;
            limit / largest
        } else {
            1.
        };
        for (p, &delta) in self.predicted.iter_mut().zip(&self.deltas) {
            *p += delta * factor;
        }
    }
    fn apply_viscosity(&mut self, dt: f32) {
        if self.config.viscosity == 0. {
            return;
        }
        self.weight_sum.fill(0.);
        self.deltas.fill(Vec3A::ZERO);
        let weight = |p: &Pair| {
            self.config.particle_mass
                * p.weight
                * 0.5
                * (self.particles[p.i].density.recip() + self.particles[p.j].density.recip())
        };
        for p in &self.pairs {
            let w = weight(p);
            self.weight_sum[p.i] += w;
            self.weight_sum[p.j] += w;
        }
        // Bound the graph diffusion update so viscosity cannot amplify energy.
        let max_row = self.weight_sum.iter().copied().fold(1., f32::max);
        let blend = (1. - (-self.config.viscosity * dt).exp()).min(0.5) / max_row;
        for p in &self.pairs {
            let dv =
                (self.particles[p.j].velocity - self.particles[p.i].velocity) * (blend * weight(p));
            self.deltas[p.i] += dv;
            self.deltas[p.j] -= dv;
        }
        for (p, &dv) in self.particles.iter_mut().zip(&self.deltas) {
            p.velocity += dv;
        }
    }
    fn project_boundaries(&mut self, bodies: &[Body]) {
        self.solid_contacts.clear();
        let radius = self.config.particle_radius;
        let lo = self.config.bounds_min + Vec3A::splat(radius);
        let hi = self.config.bounds_max - Vec3A::splat(radius);
        for (i, p) in self.predicted.iter_mut().enumerate() {
            for axis in 0..3 {
                if p[axis] < lo[axis] || p[axis] > hi[axis] {
                    self.stats.boundary_contacts += 1;
                }
            }
            *p = p.clamp(lo, hi);
            for body in bodies {
                let pose = body.pose();
                let result = match body.shape() {
                    Shape::Sphere { radius: r } => {
                        let delta = *p - pose.position;
                        let distance = delta.length();
                        if distance <= r + radius + 1e-5 {
                            let n = if distance > 1e-7 {
                                delta / distance
                            } else {
                                Vec3A::Y
                            };
                            Some((n, (r + radius - distance).max(0.)))
                        } else {
                            None
                        }
                    }
                    Shape::Box { half_extents: h } => {
                        // Conservative particle-radius expansion of the OBB.
                        let local = pose.rotation.conjugate() * (*p - pose.position);
                        let gap = h + Vec3A::splat(radius) - local.abs();
                        if gap.min_element() >= -1e-5 {
                            let axis = if gap.x <= gap.y && gap.x <= gap.z {
                                0
                            } else if gap.y <= gap.z {
                                1
                            } else {
                                2
                            };
                            let mut n = Vec3A::ZERO;
                            n[axis] = if local[axis] >= 0. { 1. } else { -1. };
                            Some((pose.rotation * n, gap[axis].max(0.)))
                        } else {
                            None
                        }
                    }
                };
                if let Some((normal, depth)) = result {
                    *p += normal * depth;
                    self.stats.boundary_contacts += 1;
                    self.solid_contacts.push(SolidContact {
                        particle: i,
                        normal,
                        velocity: body.velocity()
                            + body.angular_velocity().cross(*p - pose.position),
                    });
                }
            }
            // The domain wins if a body intrudes through a container wall.
            *p = p.clamp(lo, hi);
        }
    }
    fn respond_to_boundaries(&mut self) {
        let lo = self.config.bounds_min + Vec3A::splat(self.config.particle_radius);
        let hi = self.config.bounds_max - Vec3A::splat(self.config.particle_radius);
        let response = |v: &mut Vec3A, incoming: Vec3A, normal: Vec3A, boundary_v: Vec3A| {
            let relative = *v - boundary_v;
            let vn = relative.dot(normal);
            let incoming_vn = (incoming - boundary_v).dot(normal);
            if vn < 0. || incoming_vn < 0. {
                let tangent = relative - normal * vn;
                // Position projection may already remove the inward velocity.
                // Restitution still uses the actual pre-impact normal speed.
                let outward = vn.max((-incoming_vn * self.config.boundary_restitution).max(0.));
                *v = boundary_v + tangent * (1. - self.config.boundary_friction) + normal * outward;
            }
        };
        for c in &self.solid_contacts {
            response(
                &mut self.particles[c.particle].velocity,
                self.incoming_velocity[c.particle],
                c.normal,
                c.velocity,
            );
        }
        for (i, p) in self.particles.iter_mut().enumerate() {
            for axis in 0..3 {
                let mut n = Vec3A::ZERO;
                if p.position[axis] <= lo[axis] + 1e-5 {
                    n[axis] = 1.;
                } else if p.position[axis] >= hi[axis] - 1e-5 {
                    n[axis] = -1.;
                } else {
                    continue;
                }
                response(&mut p.velocity, self.incoming_velocity[i], n, Vec3A::ZERO);
            }
        }
    }
}

#[cfg(test)]
#[path = "fluid_tests.rs"]
mod tests;
