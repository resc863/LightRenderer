//! Stable render slots for physics bodies; both DXR and mesh shaders use the same poses.
use crate::{
    physics::{
        BodyDesc, BodyId, Config, FluidId, PhysicsError, Shape, World,
        fluid::{Fluid, FluidConfig, ParticleDesc},
    },
    scene::{self, Object},
};
use glam::{Quat, Vec3, Vec3A};

pub const DEMO_BODY_COUNT: usize = 24;
pub const DEMO_FLUID_PARTICLES: usize = 216;
struct RenderBody {
    id: BodyId,
    color: Vec3,
}

pub struct PhysicsDemo {
    pub world: World,
    render_bodies: Vec<RenderBody>,
    fluid: Option<FluidId>,
}
impl PhysicsDemo {
    pub fn new() -> Result<Self, PhysicsError> {
        Self::create(false)
    }
    pub fn with_fluid() -> Result<Self, PhysicsError> {
        Self::create(true)
    }
    fn create(enable_fluid: bool) -> Result<Self, PhysicsError> {
        let mut world = World::new(Config::default())?;
        for o in scene::environment() {
            let mut desc = BodyDesc::fixed(
                Shape::Box {
                    half_extents: Vec3A::from_slice(&o.size),
                },
                Vec3A::from_slice(&o.center_kind),
            );
            desc.rotation = Quat::from_array(o.rotation);
            desc.restitution = 0.35;
            world.insert(desc)?;
        }
        // Mascot collision proxy, deliberately simpler than the animated render geometry.
        world.insert(BodyDesc::fixed(
            Shape::Box {
                half_extents: Vec3A::new(0.8, 1.3, 0.5),
            },
            Vec3A::new(0., 1.65, 0.),
        ))?;
        let mut render_bodies = Vec::with_capacity(DEMO_BODY_COUNT);
        for i in 0..DEMO_BODY_COUNT {
            let column = i % 4;
            let layer = i / 4;
            let position = Vec3A::new(
                if column % 2 == 0 { -1.65 } else { 1.65 },
                1.1 + layer as f32 * 0.65,
                if column < 2 { -1.6 } else { 1. },
            );
            let shape = if column < 2 {
                Shape::Sphere { radius: 0.22 }
            } else {
                Shape::Box {
                    half_extents: Vec3A::splat(0.22),
                }
            };
            let mut desc = BodyDesc::dynamic(shape, position, 1.);
            desc.restitution = 0.35;
            desc.motion_substeps = true;
            if column >= 2 {
                desc.rotation = Quat::from_rotation_y(layer as f32 * 0.19);
            }
            let id = world.insert(desc)?;
            let color = if column < 2 {
                Vec3::new(0.96, 0.57, 0.25)
            } else {
                Vec3::new(0.28, 0.65, 0.90)
            };
            render_bodies.push(RenderBody { id, color });
        }
        let fluid = if enable_fluid {
            let mut fluid = Fluid::new(FluidConfig {
                bounds_min: Vec3A::new(-4.2, 0.46, -3.5),
                bounds_max: Vec3A::new(-2.6, 2., -1.9),
                particle_radius: 0.04,
                smoothing_radius: 0.16,
                particle_mass: 0.512,
                ..Default::default()
            })?;
            for x in 0..6 {
                for y in 0..6 {
                    for z in 0..6 {
                        fluid.insert(ParticleDesc::new(Vec3A::new(
                            -4. + x as f32 * 0.08,
                            0.75 + y as f32 * 0.08,
                            -3.3 + z as f32 * 0.08,
                        )))?;
                    }
                }
            }
            Some(world.add_fluid(fluid))
        } else {
            None
        };
        Ok(Self {
            world,
            render_bodies,
            fluid,
        })
    }
    pub fn reset(&mut self) -> Result<(), PhysicsError> {
        *self = Self::create(self.fluid.is_some())?;
        Ok(())
    }
    /// Give every body an off-centre impulse, waking sleeping stacks.
    pub fn kick(&mut self) -> Result<(), PhysicsError> {
        for (i, slot) in self.render_bodies.iter().enumerate() {
            let p = self.world.body(slot.id).unwrap().pose().position;
            self.world.apply_impulse(
                slot.id,
                Vec3A::new(if i % 2 == 0 { -0.5 } else { 0.5 }, 2.5, -0.35),
                p + Vec3A::X * 0.1,
            )?;
        }
        if let Some(id) = self.fluid {
            self.world
                .fluid_mut(id)
                .unwrap()
                .add_velocity(Vec3A::new(0.4, 1.8, 0.25))?;
        }
        Ok(())
    }
    pub fn fluid(&self) -> Option<&Fluid> {
        self.fluid.and_then(|id| self.world.fluid(id))
    }
    pub fn append_objects(&self, objects: &mut Vec<Object>) {
        let alpha = self.world.interpolation_alpha();
        for slot in &self.render_bodies {
            let body = self.world.body(slot.id).unwrap();
            let pose = body.interpolated_pose(alpha);
            let (size, kind) = match body.shape() {
                Shape::Sphere { radius } => (Vec3::splat(radius), 0.),
                Shape::Box { half_extents } => (half_extents.into(), 1.),
            };
            objects.push(Object::new(
                pose.position.into(),
                size,
                slot.color,
                kind,
                0.08,
                pose.rotation,
            ));
        }
        if let Some(fluid) = self.fluid() {
            for particle in fluid.particles() {
                objects.push(Object::new(
                    particle.interpolated_position(alpha).into(),
                    Vec3::splat(fluid.config().particle_radius),
                    Vec3::new(0.12, 0.65, 0.85),
                    0.,
                    0.3,
                    Quat::IDENTITY,
                ));
            }
        }
    }
    pub fn report(&self) -> serde_json::Value {
        let s = self.world.stats();
        serde_json::json!({
            "engine": "LightRenderer native rigid bodies", "external_physics_dependencies": 0,
            "math_backend": if cfg!(feature="physics-scalar") { "scalar" } else { "glam Vec3A SIMD" },
            "fixed_hz": 1. / self.world.config().fixed_dt, "dynamic_bodies": self.render_bodies.len(),
            "static_bodies": self.world.bodies().filter(|(_, b)| !b.is_dynamic()).count(),
            "steps": s.steps, "motion_substeps": s.motion_substeps, "motion_limit_hits": s.motion_limit_hits,
            "contacts": s.contacts, "total_contacts": s.total_contacts,
            "active_bodies": s.active_bodies, "sleeping_bodies": s.sleeping_bodies,
            "last_step_ms": s.last_step_ms, "total_step_ms": s.total_step_ms,
            "dropped_seconds": s.dropped_seconds,
            "fluid": self.fluid().map(|fluid| {
                let s = fluid.stats();
                serde_json::json!({"method":"PBF", "particles":fluid.particles().len(),
                    "total_mass_kg":fluid.total_mass(), "steps":s.steps,"substeps":s.substeps,
                    "neighbor_pairs":s.neighbor_pairs,"grid_candidates":s.grid_candidates,
                    "boundary_contacts":s.boundary_contacts,"max_compression":s.max_compression_after,
                    "min_density":s.min_density,"max_density":s.max_density,"max_speed":s.max_speed,
                    "last_step_ms":s.last_step_ms,"total_step_ms":s.total_step_ms,
                    "motion_limit_hits":s.motion_limit_hits,"speed_limit_hits":s.speed_limit_hits,
                    "correction_limit_hits":s.correction_limit_hits,
                    "one_way_rigid_boundaries":true,
                    "positions":fluid.particles().iter().map(|p|p.position().to_array()).collect::<Vec<_>>()})
            }),
            "bodies": self.render_bodies.iter().map(|slot| {
                let b = self.world.body(slot.id).unwrap();
                serde_json::json!({"position": b.pose().position.to_array(),
                    "rotation": b.pose().rotation.to_array(), "velocity": b.velocity().to_array(),
                    "sleeping": b.is_sleeping()})
            }).collect::<Vec<_>>()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn demo_keeps_render_slots_and_resets_and_kicks() {
        let mut demo = PhysicsDemo::new().unwrap();
        let mut initial = scene::garden(0.);
        demo.append_objects(&mut initial);
        let count = initial.len();
        for _ in 0..1200 {
            demo.world.step();
        }
        let mut frame = scene::garden(10.);
        demo.append_objects(&mut frame);
        assert_eq!(frame.len(), count);
        assert!(demo.world.stats().total_contacts > 0);
        assert!(
            demo.world
                .bodies()
                .all(|(_, b)| b.pose().position.is_finite() && b.pose().rotation.is_finite())
        );
        assert!(
            frame
                .iter()
                .zip(&initial)
                .skip(count - DEMO_BODY_COUNT)
                .any(|(a, b)| a.center_kind != b.center_kind)
        );
        demo.kick().unwrap();
        assert!(
            demo.render_bodies
                .iter()
                .all(|slot| !demo.world.body(slot.id).unwrap().is_sleeping())
        );
        demo.reset().unwrap();
        let mut reset = scene::garden(0.);
        demo.append_objects(&mut reset);
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&initial),
            bytemuck::cast_slice::<_, u8>(&reset)
        );
        assert_eq!(demo.world.stats().steps, 0);
    }
    #[test]
    fn fluid_demo_render_slots_pause_splash_and_reset_preserve_mass() {
        let mut demo = PhysicsDemo::with_fluid().unwrap();
        let mut initial = scene::garden(0.);
        demo.append_objects(&mut initial);
        assert_eq!(initial.len(), 446);
        let mass = demo.fluid().unwrap().total_mass();
        for _ in 0..120 {
            demo.world.step();
        }
        let positions: Vec<_> = demo
            .fluid()
            .unwrap()
            .particles()
            .iter()
            .map(|p| p.position())
            .collect();
        demo.world.advance(1., true).unwrap();
        assert_eq!(
            positions,
            demo.fluid()
                .unwrap()
                .particles()
                .iter()
                .map(|p| p.position())
                .collect::<Vec<_>>()
        );
        let velocities: Vec<_> = demo
            .fluid()
            .unwrap()
            .particles()
            .iter()
            .map(|p| p.velocity())
            .collect();
        demo.kick().unwrap();
        for (p, before) in demo.fluid().unwrap().particles().iter().zip(velocities) {
            assert!((p.velocity() - before - Vec3A::new(0.4, 1.8, 0.25)).length() < 1e-6);
        }
        let mut frame = scene::garden(1.);
        demo.append_objects(&mut frame);
        assert_eq!(frame.len(), initial.len());
        assert_eq!(demo.fluid().unwrap().total_mass(), mass);
        demo.reset().unwrap();
        let mut reset = scene::garden(0.);
        demo.append_objects(&mut reset);
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&initial),
            bytemuck::cast_slice::<_, u8>(&reset)
        );
        assert_eq!(demo.fluid().unwrap().stats().steps, 0);
        assert_eq!(demo.fluid().unwrap().total_mass(), mass);
    }
}
