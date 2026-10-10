//! Small, single-threaded rigid-body engine. No physics/benchmark dependencies.
//!
//! Units are metres, kilograms, seconds. Normals point from body A to body B.
//! Vec3A uses glam's SIMD backend by default (SSE2 on x86_64); no unsafe intrinsics
//! or CPU-specific executable is required. `physics-scalar` selects scalar math.
use glam::{Mat3A, Quat, Vec3, Vec3A};
use std::{fmt, time::Instant};

pub mod fluid;

/// Append-only handle for a fluid system in this world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FluidId(usize);

const SLOP: f32 = 0.002;
const MAX_MOTION_SUBSTEPS: usize = 16;

#[derive(Clone, Copy, Debug)]
pub enum Shape {
    Sphere { radius: f32 },
    Box { half_extents: Vec3A },
}
impl Shape {
    fn valid(self) -> bool {
        match self {
            Self::Sphere { radius } => radius.is_finite() && radius >= 0.01,
            Self::Box { half_extents: h } => h.is_finite() && h.min_element() >= 0.01,
        }
    }
    fn min_extent(self) -> f32 {
        match self {
            Self::Sphere { radius } => radius,
            Self::Box { half_extents } => half_extents.min_element(),
        }
    }
    fn inverse_inertia(self, mass: f32) -> Vec3A {
        match self {
            Self::Sphere { radius } => Vec3A::splat(2.5 / (mass * radius * radius)),
            Self::Box { half_extents: h } => {
                let h2 = h * h;
                Vec3A::new(h2.y + h2.z, h2.x + h2.z, h2.x + h2.y).recip() * (3. / mass)
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BodyDesc {
    pub shape: Shape,
    pub position: Vec3A,
    pub rotation: Quat,
    /// Zero mass creates a static body.
    pub mass: f32,
    pub velocity: Vec3A,
    pub angular_velocity: Vec3A,
    pub restitution: f32,
    pub friction: f32,
    /// Enables bounded motion substeps, not an unbounded swept-shape CCD solver.
    pub motion_substeps: bool,
    pub can_sleep: bool,
}
impl BodyDesc {
    pub fn dynamic(shape: Shape, position: Vec3A, mass: f32) -> Self {
        Self {
            shape,
            position,
            mass,
            rotation: Quat::IDENTITY,
            velocity: Vec3A::ZERO,
            angular_velocity: Vec3A::ZERO,
            restitution: 0.2,
            friction: 0.6,
            motion_substeps: false,
            can_sleep: true,
        }
    }
    pub fn fixed(shape: Shape, position: Vec3A) -> Self {
        Self::dynamic(shape, position, 0.)
    }
}

/// Append-only, world-local handle. A new world invalidates all old handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyId(usize);

#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub position: Vec3A,
    pub rotation: Quat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicsError(pub &'static str);
impl fmt::Display for PhysicsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for PhysicsError {}

pub struct Body {
    shape: Shape,
    pose: Pose,
    previous: Pose,
    velocity: Vec3A,
    angular_velocity: Vec3A,
    inv_mass: f32,
    inv_inertia: Vec3A,
    world_inertia: Mat3A,
    basis: [Vec3A; 3],
    restitution: f32,
    friction: f32,
    motion_substeps: bool,
    can_sleep: bool,
    sleep_time: f32,
    sleeping: bool,
}
impl Body {
    pub fn pose(&self) -> Pose {
        self.pose
    }
    pub fn velocity(&self) -> Vec3A {
        self.velocity
    }
    pub fn angular_velocity(&self) -> Vec3A {
        self.angular_velocity
    }
    pub fn is_sleeping(&self) -> bool {
        self.sleeping
    }
    pub fn shape(&self) -> Shape {
        self.shape
    }
    pub fn is_dynamic(&self) -> bool {
        self.inv_mass > 0.
    }
    pub fn interpolated_pose(&self, alpha: f32) -> Pose {
        let alpha = if alpha.is_finite() {
            alpha.clamp(0., 1.)
        } else {
            1.
        };
        Pose {
            position: self.previous.position.lerp(self.pose.position, alpha),
            rotation: self.previous.rotation.slerp(self.pose.rotation, alpha),
        }
    }
    fn inertia(&self, v: Vec3A) -> Vec3A {
        if self.sleeping {
            return Vec3A::ZERO;
        }
        self.world_inertia * v
    }
    fn update_rotation_cache(&mut self) {
        let rotation = Mat3A::from_quat(self.pose.rotation);
        self.basis = [rotation.x_axis, rotation.y_axis, rotation.z_axis];
        self.world_inertia =
            rotation * Mat3A::from_diagonal(self.inv_inertia.into()) * rotation.transpose();
    }
    fn solve_mass(&self) -> f32 {
        if self.sleeping { 0. } else { self.inv_mass }
    }
    fn wake(&mut self) {
        self.sleeping = false;
        self.sleep_time = 0.;
    }
    fn impulse(&mut self, impulse: Vec3A, offset: Vec3A) {
        self.velocity += impulse * self.solve_mass();
        self.angular_velocity += self.inertia(offset.cross(impulse));
    }
    fn bounds(&self) -> Aabb {
        let extent = match self.shape {
            Shape::Sphere { radius } => Vec3A::splat(radius),
            Shape::Box { half_extents: h } => {
                let axes = self.basis;
                axes[0].abs() * h.x + axes[1].abs() * h.y + axes[2].abs() * h.z
            }
        };
        Aabb {
            min: self.pose.position - extent,
            max: self.pose.position + extent,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub fixed_dt: f32,
    pub gravity: Vec3A,
    pub solver_iterations: usize,
    pub max_catchup_steps: usize,
    pub max_frame_dt: f32,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            fixed_dt: 1. / 120.,
            gravity: Vec3A::new(0., -9.81, 0.),
            solver_iterations: 10,
            max_catchup_steps: 8,
            max_frame_dt: 0.25,
        }
    }
}
#[derive(Default, Clone, Debug)]
pub struct Stats {
    pub steps: u64,
    pub motion_substeps: u64,
    pub motion_limit_hits: u64,
    pub candidate_pairs: usize,
    pub contacts: usize,
    pub total_contacts: u64,
    pub active_bodies: usize,
    pub sleeping_bodies: usize,
    pub dropped_seconds: f64,
    pub last_step_ms: f64,
    pub total_step_ms: f64,
    pub fluid_particles: usize,
    pub fluid_step_ms: f64,
}

#[derive(Clone, Copy, Default)]
struct Aabb {
    min: Vec3A,
    max: Vec3A,
}
impl Aabb {
    fn overlaps(self, other: Self) -> bool {
        self.min.cmple(other.max).all() && other.min.cmple(self.max).all()
    }
}
struct Contact {
    a: usize,
    b: usize,
    normal: Vec3A,
    ra: Vec3A,
    rb: Vec3A,
    depth: f32,
    normal_mass: f32,
    tangents: [Vec3A; 2],
    tangent_mass: [f32; 3],
    angular_a: [Vec3A; 3],
    angular_b: [Vec3A; 3],
    target_velocity: f32,
    friction: f32,
    normal_impulse: f32,
    tangent_impulse: Vec3A,
    manifold_count: f32,
}

pub struct World {
    config: Config,
    bodies: Vec<Body>,
    bounds: Vec<Aabb>,
    order: Vec<usize>,
    contacts: Vec<Contact>,
    accumulator: f64,
    stats: Stats,
    fluids: Vec<fluid::Fluid>,
}
impl World {
    pub fn new(config: Config) -> Result<Self, PhysicsError> {
        if !config.fixed_dt.is_finite()
            || !(0.001..=0.05).contains(&config.fixed_dt)
            || !config.gravity.is_finite()
            || !(1..=64).contains(&config.solver_iterations)
            || !(1..=64).contains(&config.max_catchup_steps)
            || !config.max_frame_dt.is_finite()
            || config.max_frame_dt < config.fixed_dt
            || config.max_frame_dt > 1.
        {
            return Err(PhysicsError("Invalid physics configuration"));
        }
        Ok(Self {
            config,
            bodies: vec![],
            bounds: vec![],
            order: vec![],
            contacts: vec![],
            accumulator: 0.,
            stats: Stats::default(),
            fluids: vec![],
        })
    }
    pub fn insert(&mut self, desc: BodyDesc) -> Result<BodyId, PhysicsError> {
        if !desc.shape.valid()
            || !desc.position.is_finite()
            || !desc.rotation.is_finite()
            || (desc.rotation.length_squared() - 1.).abs() > 0.001
            || !desc.mass.is_finite()
            || desc.mass < 0.
            || !desc.velocity.is_finite()
            || !desc.angular_velocity.is_finite()
            || !desc.restitution.is_finite()
            || !(0. ..=1.).contains(&desc.restitution)
            || !desc.friction.is_finite()
            || !(0. ..=2.).contains(&desc.friction)
        {
            return Err(PhysicsError("Invalid body shape, pose, mass or material"));
        }
        let inv_mass = if desc.mass > 0. {
            desc.mass.recip()
        } else {
            0.
        };
        let inv_inertia = if desc.mass > 0. {
            desc.shape.inverse_inertia(desc.mass)
        } else {
            Vec3A::ZERO
        };
        if !inv_mass.is_finite() || !inv_inertia.is_finite() {
            return Err(PhysicsError("Mass is too small for finite inertia"));
        }
        let id = BodyId(self.bodies.len());
        let pose = Pose {
            position: desc.position,
            rotation: desc.rotation,
        };
        let mut body = Body {
            shape: desc.shape,
            pose,
            previous: pose,
            velocity: if inv_mass > 0. {
                desc.velocity
            } else {
                Vec3A::ZERO
            },
            angular_velocity: if inv_mass > 0. {
                desc.angular_velocity
            } else {
                Vec3A::ZERO
            },
            inv_mass,
            inv_inertia,
            world_inertia: Mat3A::ZERO,
            basis: [Vec3A::ZERO; 3],
            restitution: desc.restitution,
            friction: desc.friction,
            motion_substeps: desc.motion_substeps,
            can_sleep: desc.can_sleep,
            sleep_time: 0.,
            sleeping: false,
        };
        body.update_rotation_cache();
        self.bodies.push(body);
        self.bounds.push(Aabb::default());
        self.order.push(id.0);
        Ok(id)
    }
    pub fn add_fluid(&mut self, fluid: fluid::Fluid) -> FluidId {
        let id = FluidId(self.fluids.len());
        self.fluids.push(fluid);
        id
    }
    pub fn fluid(&self, id: FluidId) -> Option<&fluid::Fluid> {
        self.fluids.get(id.0)
    }
    pub fn fluid_mut(&mut self, id: FluidId) -> Option<&mut fluid::Fluid> {
        self.fluids.get_mut(id.0)
    }
    pub fn fluids(&self) -> impl Iterator<Item = (FluidId, &fluid::Fluid)> {
        self.fluids.iter().enumerate().map(|(i, f)| (FluidId(i), f))
    }
    pub fn body(&self, id: BodyId) -> Option<&Body> {
        self.bodies.get(id.0)
    }
    pub fn bodies(&self) -> impl Iterator<Item = (BodyId, &Body)> {
        self.bodies.iter().enumerate().map(|(i, b)| (BodyId(i), b))
    }
    pub fn config(&self) -> Config {
        self.config
    }
    pub fn stats(&self) -> &Stats {
        &self.stats
    }
    pub fn interpolation_alpha(&self) -> f32 {
        (self.accumulator / self.config.fixed_dt as f64) as f32
    }
    pub fn apply_impulse(
        &mut self,
        id: BodyId,
        impulse: Vec3A,
        point: Vec3A,
    ) -> Result<(), PhysicsError> {
        if !impulse.is_finite() || !point.is_finite() {
            return Err(PhysicsError("Non-finite impulse"));
        }
        let b = self
            .bodies
            .get_mut(id.0)
            .ok_or(PhysicsError("Unknown body"))?;
        if b.is_dynamic() {
            b.wake();
            b.impulse(impulse, point - b.pose.position);
        }
        Ok(())
    }
    /// Consume wall-clock time. Pausing retains the previous interpolation state.
    /// Excess time is counted and dropped, bounding work after a stall.
    pub fn advance(&mut self, frame_dt: f32, paused: bool) -> Result<usize, PhysicsError> {
        if !frame_dt.is_finite() || frame_dt < 0. {
            return Err(PhysicsError("Invalid frame delta"));
        }
        if paused {
            return Ok(0);
        }
        let accepted = frame_dt.min(self.config.max_frame_dt);
        self.stats.dropped_seconds += (frame_dt - accepted) as f64;
        self.accumulator += accepted as f64;
        let dt = self.config.fixed_dt as f64;
        let available = ((self.accumulator + dt * 1e-6) / dt).floor() as usize;
        let count = available.min(self.config.max_catchup_steps);
        for _ in 0..count {
            self.step();
        }
        self.accumulator = (self.accumulator - count as f64 * dt).max(0.);
        if available > count {
            let dropped = (available - count) as f64 * dt;
            self.accumulator = (self.accumulator - dropped).max(0.);
            self.stats.dropped_seconds += dropped;
        }
        Ok(count)
    }
    /// Advance exactly one fixed tick. Useful for replay, tests and benchmarks.
    pub fn step(&mut self) {
        let start = Instant::now();
        let dt = self.config.fixed_dt;
        let mut requested = 1usize;
        for b in &mut self.bodies {
            b.previous = b.pose;
            if b.motion_substeps && b.is_dynamic() && !b.sleeping {
                let radius = match b.shape {
                    Shape::Sphere { radius } => radius,
                    Shape::Box { half_extents } => half_extents.length(),
                };
                let travel = (b.velocity.length()
                    + self.config.gravity.length() * dt
                    + b.angular_velocity.length() * radius)
                    * dt;
                requested = requested.max((travel / (b.shape.min_extent() * 0.5)).ceil() as usize);
            }
        }
        if requested > MAX_MOTION_SUBSTEPS {
            self.stats.motion_limit_hits += 1;
        }
        let substeps = requested.clamp(1, MAX_MOTION_SUBSTEPS);
        self.stats.contacts = 0;
        self.stats.candidate_pairs = 0;
        for _ in 0..substeps {
            self.substep(dt / substeps as f32);
        }
        // Fluid sees the current rigid poses as one-way moving solid boundaries.
        // It shares the world clock/gravity, pause and catchup policy.
        self.stats.fluid_particles = 0;
        self.stats.fluid_step_ms = 0.;
        for fluid in &mut self.fluids {
            fluid.step_in_world(dt, self.config.gravity, &self.bodies);
            self.stats.fluid_particles += fluid.particles().len();
            self.stats.fluid_step_ms += fluid.stats().last_step_ms;
        }
        self.stats.steps += 1;
        self.stats.motion_substeps += substeps as u64;
        self.stats.active_bodies = self
            .bodies
            .iter()
            .filter(|b| b.is_dynamic() && !b.sleeping)
            .count();
        self.stats.sleeping_bodies = self
            .bodies
            .iter()
            .filter(|b| b.is_dynamic() && b.sleeping)
            .count();
        self.stats.last_step_ms = start.elapsed().as_secs_f64() * 1000.;
        self.stats.total_step_ms += self.stats.last_step_ms;
    }
    fn substep(&mut self, dt: f32) {
        for b in &mut self.bodies {
            if b.is_dynamic() && !b.sleeping {
                b.velocity += self.config.gravity * dt;
                b.pose.position += b.velocity * dt;
                b.pose.rotation = (Quat::from_scaled_axis(Vec3::from(b.angular_velocity * dt))
                    * b.pose.rotation)
                    .normalize();
                b.update_rotation_cache();
            }
        }
        self.contacts.clear();
        self.detect_contacts(dt);
        self.stats.contacts += self.contacts.len();
        self.stats.total_contacts += self.contacts.len() as u64;
        for _ in 0..self.config.solver_iterations {
            for c in &mut self.contacts {
                let (a, b) = pair_mut(&mut self.bodies, c.a, c.b);
                let rv = velocity_at(b, c.rb) - velocity_at(a, c.ra);
                let old = c.normal_impulse;
                c.normal_impulse =
                    (old + (c.target_velocity - rv.dot(c.normal)) * c.normal_mass).max(0.);
                solve_impulse(a, b, c, [c.normal_impulse - old, 0., 0.]);
                let rv = velocity_at(b, c.rb) - velocity_at(a, c.ra);
                let u = rv.dot(c.tangents[0]);
                let v = rv.dot(c.tangents[1]);
                let old = c.tangent_impulse;
                let proposed = old
                    - c.tangents[0] * (c.tangent_mass[0] * u + c.tangent_mass[1] * v)
                    - c.tangents[1] * (c.tangent_mass[1] * u + c.tangent_mass[2] * v);
                c.tangent_impulse = proposed.clamp_length_max(c.friction * c.normal_impulse);
                let impulse = c.tangent_impulse - old;
                solve_impulse(
                    a,
                    b,
                    c,
                    [0., impulse.dot(c.tangents[0]), impulse.dot(c.tangents[1])],
                );
            }
        }
        for c in &self.contacts {
            let (a, b) = pair_mut(&mut self.bodies, c.a, c.b);
            let correction = c.normal
                * (0.25 * (c.depth - SLOP).max(0.)
                    / ((a.solve_mass() + b.solve_mass()) * c.manifold_count));
            a.pose.position -= correction * a.solve_mass();
            b.pose.position += correction * b.solve_mass();
        }
        for b in &mut self.bodies {
            if !b.is_dynamic() || b.sleeping {
                continue;
            }
            if b.can_sleep
                && b.velocity.length_squared() < 0.0064
                && b.angular_velocity.length_squared() < 0.0064
            {
                b.sleep_time += dt;
                if b.sleep_time >= 0.8 {
                    b.sleeping = true;
                    b.velocity = Vec3A::ZERO;
                    b.angular_velocity = Vec3A::ZERO;
                }
            } else {
                b.sleep_time = 0.;
            }
        }
    }
    fn detect_contacts(&mut self, dt: f32) {
        let mut lo = Vec3A::splat(f32::INFINITY);
        let mut hi = Vec3A::splat(f32::NEG_INFINITY);
        for (i, b) in self.bodies.iter().enumerate() {
            self.bounds[i] = b.bounds();
            if b.is_dynamic() {
                lo = lo.min(b.pose.position);
                hi = hi.max(b.pose.position);
            }
        }
        let span = hi - lo;
        let axis = if span.x >= span.y && span.x >= span.z {
            0
        } else if span.y >= span.z {
            1
        } else {
            2
        };
        self.order.sort_unstable_by(|&a, &b| {
            self.bounds[a].min[axis]
                .total_cmp(&self.bounds[b].min[axis])
                .then(a.cmp(&b))
        });
        for i in 0..self.order.len() {
            let ai = self.order[i];
            for j in i + 1..self.order.len() {
                let bi = self.order[j];
                if self.bounds[bi].min[axis] > self.bounds[ai].max[axis] {
                    break;
                }
                let (a, b) = pair_mut(&mut self.bodies, ai, bi);
                if (!a.is_dynamic() || a.sleeping) && (!b.is_dynamic() || b.sleeping) {
                    continue;
                }
                if !self.bounds[ai].overlaps(self.bounds[bi]) {
                    continue;
                }
                self.stats.candidate_pairs += 1;
                let Some(manifold) = collide(a, b) else {
                    continue;
                };
                // An impacting active body wakes its sleeping neighbour. Resting
                // contacts alone must not repeatedly reset the sleep timer.
                if b.sleeping
                    && a.is_dynamic()
                    && (a.velocity.length_squared() > 0.0225
                        || a.angular_velocity.length_squared() > 0.0225
                        || manifold.depth > 0.02)
                {
                    b.wake();
                }
                if a.sleeping
                    && b.is_dynamic()
                    && (b.velocity.length_squared() > 0.0225
                        || b.angular_velocity.length_squared() > 0.0225
                        || manifold.depth > 0.02)
                {
                    a.wake();
                }
                // A body that remains sleeping acts as a static support in this solve.
                for &point in &manifold.points[..manifold.count] {
                    let ra = point - a.pose.position;
                    let rb = point - b.pose.position;
                    let vn = (velocity_at(b, rb) - velocity_at(a, ra)).dot(manifold.normal);
                    let bounce = if vn < -1. {
                        -a.restitution.min(b.restitution) * vn
                    } else {
                        0.
                    };
                    let bias = 0.15 * (manifold.depth - SLOP).max(0.) / dt;
                    self.contacts.push(Contact {
                        a: ai,
                        b: bi,
                        normal: manifold.normal,
                        ra,
                        rb,
                        depth: manifold.depth,
                        normal_mass: 0.,
                        tangents: [Vec3A::ZERO; 2],
                        tangent_mass: [0.; 3],
                        angular_a: [Vec3A::ZERO; 3],
                        angular_b: [Vec3A::ZERO; 3],
                        target_velocity: bounce.max(bias),
                        friction: (a.friction * b.friction).sqrt(),
                        normal_impulse: 0.,
                        tangent_impulse: Vec3A::ZERO,
                        manifold_count: manifold.count as f32,
                    });
                }
            }
        }
        // Prepare only after all impact wakeups, so inverse masses are consistent
        // across every contact in this substep. Reuse these Jacobians in the solver.
        for c in &mut self.contacts {
            let a = &self.bodies[c.a];
            let b = &self.bodies[c.b];
            let t = c
                .normal
                .cross(if c.normal.x.abs() < 0.7 {
                    Vec3A::X
                } else {
                    Vec3A::Y
                })
                .normalize();
            c.tangents = [t, c.normal.cross(t)];
            let directions = [c.normal, c.tangents[0], c.tangents[1]];
            for (i, &direction) in directions.iter().enumerate() {
                c.angular_a[i] = a.inertia(c.ra.cross(direction));
                c.angular_b[i] = b.inertia(c.rb.cross(direction));
            }
            let k = |i: usize, j: usize| {
                (a.solve_mass() + b.solve_mass()) * directions[i].dot(directions[j])
                    + c.ra.cross(directions[i]).dot(c.angular_a[j])
                    + c.rb.cross(directions[i]).dot(c.angular_b[j])
            };
            let normal_k = k(0, 0);
            c.normal_mass = if normal_k > 1e-12 {
                normal_k.recip()
            } else {
                0.
            };
            let (ku, kuv, kv) = (k(1, 1), k(1, 2), k(2, 2));
            let determinant = ku * kv - kuv * kuv;
            c.tangent_mass = if determinant > 1e-12 {
                [kv / determinant, -kuv / determinant, ku / determinant]
            } else {
                [0.; 3]
            };
        }
    }
}
fn pair_mut(bodies: &mut [Body], a: usize, b: usize) -> (&mut Body, &mut Body) {
    if a < b {
        let (l, r) = bodies.split_at_mut(b);
        (&mut l[a], &mut r[0])
    } else {
        let (l, r) = bodies.split_at_mut(a);
        (&mut r[0], &mut l[b])
    }
}
fn velocity_at(b: &Body, r: Vec3A) -> Vec3A {
    b.velocity + b.angular_velocity.cross(r)
}
fn solve_impulse(a: &mut Body, b: &mut Body, c: &Contact, impulse: [f32; 3]) {
    let linear = c.normal * impulse[0] + c.tangents[0] * impulse[1] + c.tangents[1] * impulse[2];
    a.velocity -= linear * a.solve_mass();
    b.velocity += linear * b.solve_mass();
    a.angular_velocity -=
        c.angular_a[0] * impulse[0] + c.angular_a[1] * impulse[1] + c.angular_a[2] * impulse[2];
    b.angular_velocity +=
        c.angular_b[0] * impulse[0] + c.angular_b[1] * impulse[1] + c.angular_b[2] * impulse[2];
}
struct Manifold {
    normal: Vec3A,
    depth: f32,
    points: [Vec3A; 8],
    count: usize,
}
impl Manifold {
    fn single(normal: Vec3A, depth: f32, point: Vec3A) -> Self {
        let mut points = [Vec3A::ZERO; 8];
        points[0] = point;
        Self {
            normal,
            depth,
            points,
            count: 1,
        }
    }
}
fn collide(a: &Body, b: &Body) -> Option<Manifold> {
    match (a.shape, b.shape) {
        (Shape::Sphere { radius: ar }, Shape::Sphere { radius: br }) => {
            let delta = b.pose.position - a.pose.position;
            let distance2 = delta.length_squared();
            if distance2 >= (ar + br).powi(2) {
                return None;
            }
            let distance = distance2.sqrt();
            let n = if distance > 1e-6 {
                delta / distance
            } else {
                Vec3A::Y
            };
            Some(Manifold::single(
                n,
                ar + br - distance,
                a.pose.position + n * (ar - (ar + br - distance) * 0.5),
            ))
        }
        (Shape::Sphere { radius }, Shape::Box { half_extents }) => {
            sphere_box(a, radius, b, half_extents)
        }
        (Shape::Box { half_extents }, Shape::Sphere { radius }) => {
            let mut m = sphere_box(b, radius, a, half_extents)?;
            m.normal = -m.normal;
            Some(m)
        }
        (Shape::Box { half_extents: ah }, Shape::Box { half_extents: bh }) => box_box(a, ah, b, bh),
    }
}
fn sphere_box(s: &Body, radius: f32, b: &Body, h: Vec3A) -> Option<Manifold> {
    let local = b.pose.rotation.conjugate() * (s.pose.position - b.pose.position);
    let closest = local.clamp(-h, h);
    let delta = closest - local;
    let distance2 = delta.length_squared();
    if distance2 >= radius * radius {
        return None;
    }
    if distance2 > 1e-12 {
        let distance = distance2.sqrt();
        let n = b.pose.rotation * (delta / distance);
        Some(Manifold::single(
            n,
            radius - distance,
            b.pose.position + b.pose.rotation * closest,
        ))
    } else {
        // Sphere centre inside the box: expel through its nearest face.
        let gaps = h - local.abs();
        let axis = if gaps.x <= gaps.y && gaps.x <= gaps.z {
            0
        } else if gaps.y <= gaps.z {
            1
        } else {
            2
        };
        let mut out = Vec3A::ZERO;
        out[axis] = if local[axis] >= 0. { 1. } else { -1. };
        let face = local + out * gaps[axis];
        Some(Manifold::single(
            -(b.pose.rotation * out),
            radius + gaps[axis],
            b.pose.position + b.pose.rotation * face,
        ))
    }
}
fn support(p: Vec3A, basis: [Vec3A; 3], h: Vec3A, direction: Vec3A) -> Vec3A {
    let mut result = p;
    for i in 0..3 {
        let dot = basis[i].dot(direction);
        // Centre of a support face/edge avoids spurious corner torques.
        if dot.abs() > 1e-6 {
            result += basis[i] * h[i] * dot.signum();
        }
    }
    result
}
fn box_box(a: &Body, ah: Vec3A, b: &Body, bh: Vec3A) -> Option<Manifold> {
    let aa = a.basis;
    let ba = b.basis;
    let delta = b.pose.position - a.pose.position;
    let mut depth = f32::INFINITY;
    let mut normal = Vec3A::ZERO;
    let mut best = 0;
    // 3 A face normals, 3 B face normals, 9 edge cross products (full OBB SAT).
    for index in 0..15 {
        let candidate = if index < 3 {
            aa[index]
        } else if index < 6 {
            ba[index - 3]
        } else {
            aa[(index - 6) / 3].cross(ba[(index - 6) % 3])
        };
        if candidate.length_squared() < 1e-8 {
            continue;
        }
        let n = candidate.normalize();
        let ar = (0..3).map(|i| ah[i] * aa[i].dot(n).abs()).sum::<f32>();
        let br = (0..3).map(|i| bh[i] * ba[i].dot(n).abs()).sum::<f32>();
        let overlap = ar + br - delta.dot(n).abs();
        if overlap <= 0. {
            return None;
        }
        // Prefer face manifolds in nearly parallel/near-equal cases.
        if overlap < depth - if index >= 6 { 0.001 } else { 0. } {
            depth = overlap;
            normal = if delta.dot(n) >= 0. { n } else { -n };
            best = index;
        }
    }
    if best >= 6 {
        let i = (best - 6) / 3;
        let j = (best - 6) % 3;
        let edge_center = |p: Vec3A, basis: [Vec3A; 3], h: Vec3A, axis, dir: Vec3A| {
            let mut p = p;
            for k in 0..3 {
                if k != axis {
                    p += basis[k] * h[k] * if basis[k].dot(dir) >= 0. { 1. } else { -1. };
                }
            }
            p
        };
        let ac = edge_center(a.pose.position, aa, ah, i, normal);
        let bc = edge_center(b.pose.position, ba, bh, j, -normal);
        let (ap, bp) = closest_segments(
            ac - aa[i] * ah[i],
            ac + aa[i] * ah[i],
            bc - ba[j] * bh[j],
            bc + ba[j] * bh[j],
        );
        return Some(Manifold::single(normal, depth, (ap + bp) * 0.5));
    }
    let (reference, rh, basis, incident, ih, ibasis, face_axis, outward) = if best < 3 {
        (a, ah, aa, b, bh, ba, best, normal)
    } else {
        (b, bh, ba, a, ah, aa, best - 3, -normal)
    };
    let face_center = reference.pose.position + outward * rh[face_axis];
    let mut incident_axis = 0;
    for i in 1..3 {
        if ibasis[i].dot(outward).abs() > ibasis[incident_axis].dot(outward).abs() {
            incident_axis = i;
        }
    }
    let sign = if ibasis[incident_axis].dot(outward) > 0. {
        -1.
    } else {
        1.
    };
    let center = incident.pose.position + ibasis[incident_axis] * ih[incident_axis] * sign;
    let u = (incident_axis + 1) % 3;
    let v = (incident_axis + 2) % 3;
    let du = ibasis[u] * ih[u];
    let dv = ibasis[v] * ih[v];
    let mut poly = [Vec3A::ZERO; 8];
    poly[..4].copy_from_slice(&[
        center - du - dv,
        center + du - dv,
        center + du + dv,
        center - du + dv,
    ]);
    let mut count = 4;
    for i in 0..3 {
        if i == face_axis {
            continue;
        }
        for sign in [-1., 1.] {
            let n = basis[i] * sign;
            (poly, count) = clip_polygon(poly, count, n, n.dot(face_center) + rh[i]);
        }
    }
    let mut m = Manifold {
        normal,
        depth,
        points: [Vec3A::ZERO; 8],
        count: 0,
    };
    for &p in &poly[..count] {
        let separation = outward.dot(p - face_center);
        if separation <= SLOP {
            let point = p - outward * (separation * 0.5);
            if !m.points[..m.count]
                .iter()
                .any(|&q| q.distance_squared(point) < 1e-10)
            {
                m.points[m.count] = point;
                m.count += 1;
            }
        }
    }
    if m.count == 0 {
        Some(Manifold::single(
            normal,
            depth,
            (support(a.pose.position, aa, ah, normal) + support(b.pose.position, ba, bh, -normal))
                * 0.5,
        ))
    } else {
        Some(m)
    }
}
fn clip_polygon(
    input: [Vec3A; 8],
    count: usize,
    normal: Vec3A,
    offset: f32,
) -> ([Vec3A; 8], usize) {
    let mut output = [Vec3A::ZERO; 8];
    let mut n = 0;
    if count == 0 {
        return (output, 0);
    }
    let mut previous = input[count - 1];
    let mut pd = normal.dot(previous) - offset;
    for &p in &input[..count] {
        let d = normal.dot(p) - offset;
        if (d <= 0.) != (pd <= 0.) && n < 8 {
            output[n] = previous.lerp(p, pd / (pd - d));
            n += 1;
        }
        if d <= 0. && n < 8 {
            output[n] = p;
            n += 1;
        }
        previous = p;
        pd = d;
    }
    (output, n)
}
fn closest_segments(p1: Vec3A, q1: Vec3A, p2: Vec3A, q2: Vec3A) -> (Vec3A, Vec3A) {
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let a = d1.length_squared();
    let e = d2.length_squared();
    let b = d1.dot(d2);
    let c = d1.dot(r);
    let f = d2.dot(r);
    let denominator = a * e - b * b;
    let mut s = if denominator > 1e-10 {
        ((b * f - c * e) / denominator).clamp(0., 1.)
    } else {
        0.
    };
    let mut t = (b * s + f) / e;
    if t < 0. {
        t = 0.;
        s = (-c / a).clamp(0., 1.);
    } else if t > 1. {
        t = 1.;
        s = ((b - c) / a).clamp(0., 1.);
    }
    (p1 + d1 * s, p2 + d2 * t)
}

#[cfg(test)]
#[path = "physics_tests.rs"]
mod tests;
