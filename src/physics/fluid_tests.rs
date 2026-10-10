use super::*;
use crate::physics::{BodyDesc, Config, World};
use glam::Quat;

fn quiet_config() -> FluidConfig {
    FluidConfig {
        bounds_min: Vec3A::splat(-10.),
        bounds_max: Vec3A::splat(10.),
        gravity: Vec3A::ZERO,
        viscosity: 0.,
        ..Default::default()
    }
}
fn lattice(config: FluidConfig, n: usize, spacing: f32) -> Fluid {
    let mut fluid = Fluid::new(config).unwrap();
    for x in 0..n {
        for y in 0..n {
            for z in 0..n {
                fluid
                    .insert(ParticleDesc::new(
                        Vec3A::new(x as f32, y as f32, z as f32) * spacing
                            - Vec3A::splat((n - 1) as f32 * spacing * 0.5),
                    ))
                    .unwrap();
            }
        }
    }
    fluid
}
fn momentum(f: &Fluid) -> Vec3A {
    f.particles
        .iter()
        .map(|p| p.velocity * f.config.particle_mass)
        .sum()
}
fn energy(f: &Fluid) -> f32 {
    f.particles
        .iter()
        .map(|p| 0.5 * f.config.particle_mass * p.velocity.length_squared())
        .sum()
}
fn centre(f: &Fluid) -> Vec3A {
    f.particles.iter().map(|p| p.position).sum::<Vec3A>() / f.particles.len() as f32
}

#[test]
fn kernels_have_compact_support_and_poly6_is_normalized() {
    let k = Kernel::new(0.2);
    assert!(k.weight(0.) > 0.);
    assert_eq!(k.weight(k.h2), 0.);
    assert_eq!(k.weight(k.h2 * 4.), 0.);
    assert_eq!(k.gradient(Vec3A::ZERO), Vec3A::ZERO);
    assert_eq!(k.gradient(Vec3A::X * 0.3), Vec3A::ZERO);
    let g = k.gradient(Vec3A::X * 0.1);
    assert!(g.x < 0. && g.y == 0. && g.z == 0.);
    assert_eq!(g, -k.gradient(Vec3A::NEG_X * 0.1));
    let dr = k.h / 10_000.;
    let integral = (0..10_000)
        .map(|i| {
            let r = (i as f32 + 0.5) * dr;
            4. * PI * r * r * k.weight(r * r) * dr
        })
        .sum::<f32>();
    assert!((integral - 1.).abs() < 1e-4, "{integral}");
}
#[test]
fn spatial_grid_exactly_matches_brute_force_including_negative_cells() {
    let mut f = Fluid::new(quiet_config()).unwrap();
    for i in 0..90 {
        let p = Vec3A::new(
            ((i * 17) % 31) as f32 * 0.037 - 0.6,
            ((i * 11) % 23) as f32 * 0.029 - 0.4,
            ((i * 7) % 19) as f32 * 0.041 - 0.3,
        );
        f.insert(ParticleDesc::new(p)).unwrap();
    }
    f.find_neighbors();
    let mut actual: Vec<_> = f.pairs.iter().map(|p| (p.i, p.j)).collect();
    let original_count = actual.len();
    actual.sort_unstable();
    actual.dedup();
    assert_eq!(actual.len(), original_count, "Duplicate neighbours");
    let mut expected = vec![];
    for i in 0..f.predicted.len() {
        for j in i + 1..f.predicted.len() {
            if f.predicted[i].distance_squared(f.predicted[j]) < f.kernel.h2 {
                expected.push((i, j));
            }
        }
    }
    assert!(!expected.is_empty());
    assert_eq!(actual, expected);
}
#[test]
fn density_includes_self_and_interior_lattice_approximates_rest_density() {
    let mut f = lattice(quiet_config(), 5, 0.1);
    f.find_neighbors();
    f.compute_density();
    let interior = &f.particles[2 * 25 + 2 * 5 + 2];
    assert!(
        (interior.density / f.config.rest_density - 1.).abs() < 0.03,
        "{}",
        interior.density
    );
    assert!(f.particles[0].density < interior.density);
    let mut isolated = Fluid::new(quiet_config()).unwrap();
    isolated.insert(ParticleDesc::new(Vec3A::ZERO)).unwrap();
    isolated.find_neighbors();
    isolated.compute_density();
    assert_eq!(
        isolated.particles[0].density,
        isolated.config.particle_mass * isolated.kernel.weight(0.)
    );
}
#[test]
fn pressure_projection_reduces_compression_and_preserves_centre() {
    let mut f = lattice(quiet_config(), 5, 0.075);
    f.find_neighbors();
    f.compute_density();
    let before = f.compression();
    let initial_centre = centre(&f);
    assert!(before > 0.5);
    f.step(1. / 120.).unwrap();
    assert!(
        f.compression() < before * 0.8,
        "{} -> {}",
        before,
        f.compression()
    );
    assert!((centre(&f) - initial_centre).length() < 1e-5);
    assert!(momentum(&f).length() < 0.01, "{:?}", momentum(&f));
}
#[test]
fn viscosity_dissipates_energy_without_changing_linear_momentum() {
    let mut f = Fluid::new(FluidConfig {
        viscosity: 20.,
        ..quiet_config()
    })
    .unwrap();
    f.insert(ParticleDesc {
        position: Vec3A::new(-0.05, 0., 0.),
        velocity: Vec3A::new(1., 2., 0.),
    })
    .unwrap();
    f.insert(ParticleDesc {
        position: Vec3A::new(0.05, 0., 0.),
        velocity: Vec3A::new(-1., 0., 0.),
    })
    .unwrap();
    f.find_neighbors();
    f.compute_density();
    let before_energy = energy(&f);
    let before_momentum = momentum(&f);
    // Isolate the velocity postprocess from the density projection.
    for _ in 0..60 {
        f.apply_viscosity(1. / 120.);
    }
    assert!(energy(&f) < before_energy * 0.6);
    assert!((momentum(&f) - before_momentum).length() < 1e-5);
}
#[test]
fn zero_viscosity_leaves_velocity_unchanged_and_high_viscosity_is_stable() {
    let mut f = lattice(quiet_config(), 3, 0.1);
    for (i, p) in f.particles.iter_mut().enumerate() {
        p.velocity = Vec3A::X * (i as f32 - 10.);
    }
    f.find_neighbors();
    f.compute_density();
    let initial: Vec<_> = f.particles.iter().map(|p| p.velocity).collect();
    f.apply_viscosity(0.05);
    assert_eq!(
        initial,
        f.particles.iter().map(|p| p.velocity).collect::<Vec<_>>()
    );
    f.config.viscosity = 1000.;
    let mut last_energy = energy(&f);
    let initial_momentum = momentum(&f);
    for _ in 0..100 {
        f.apply_viscosity(0.05);
        let current = energy(&f);
        assert!(current <= last_energy + 0.001);
        last_energy = current;
    }
    assert!((momentum(&f) - initial_momentum).length() < 0.001);
}
#[test]
fn isolated_particle_matches_gravity_and_uniform_advection() {
    let mut f = Fluid::new(FluidConfig {
        gravity: Vec3A::new(0., -9.81, 0.),
        ..quiet_config()
    })
    .unwrap();
    let id = f
        .insert(ParticleDesc {
            position: Vec3A::Y * 5.,
            velocity: Vec3A::X * 2.,
        })
        .unwrap();
    for _ in 0..120 {
        f.step(1. / 120.).unwrap();
    }
    let p = f.particles[id];
    assert!((p.velocity.y + 9.81).abs() < 0.002, "{}", p.velocity.y);
    assert!((p.position.x - 2.).abs() < 0.001);
    assert!((p.position.y - 0.08).abs() < 0.05, "{}", p.position.y);
    assert_eq!(f.stats.speed_limit_hits, 0);
}
#[test]
fn container_prevents_escape_and_enforces_wall_restitution() {
    let mut f = Fluid::new(FluidConfig {
        bounds_min: Vec3A::splat(-1.),
        bounds_max: Vec3A::splat(1.),
        boundary_restitution: 0.5,
        boundary_friction: 0.,
        max_substeps: 16,
        ..quiet_config()
    })
    .unwrap();
    f.insert(ParticleDesc {
        position: Vec3A::new(0.8, 0., 0.),
        velocity: Vec3A::X * 5.,
    })
    .unwrap();
    for _ in 0..12 {
        f.step(1. / 120.).unwrap();
    }
    let p = f.particles[0];
    assert!(p.position.x <= 0.95 && p.velocity.x < 0.);
    for _ in 0..600 {
        f.step(1. / 120.).unwrap();
    }
    assert!(
        f.particles
            .iter()
            .all(|p| p.position.cmpge(Vec3A::splat(-0.95)).all()
                && p.position.cmple(Vec3A::splat(0.95)).all())
    );
}

#[test]
fn restitution_uses_preimpact_speed_after_position_projection() {
    let mut f = Fluid::new(FluidConfig {
        bounds_min: Vec3A::splat(-1.),
        bounds_max: Vec3A::splat(1.),
        substeps: 1,
        boundary_restitution: 0.5,
        boundary_friction: 0.,
        ..quiet_config()
    })
    .unwrap();
    f.insert(ParticleDesc {
        position: Vec3A::new(0.94, 0., 0.),
        velocity: Vec3A::X * 5.,
    })
    .unwrap();
    f.step(1. / 120.).unwrap();
    assert!(
        (f.particles[0].velocity.x + 2.5).abs() < 1e-5,
        "{:?}",
        f.particles[0].velocity
    );
    let mut w = World::new(Config {
        gravity: Vec3A::ZERO,
        ..Default::default()
    })
    .unwrap();
    w.insert(BodyDesc::fixed(Shape::Sphere { radius: 0.4 }, Vec3A::ZERO))
        .unwrap();
    let mut f = Fluid::new(FluidConfig {
        substeps: 1,
        boundary_restitution: 0.5,
        boundary_friction: 0.,
        ..quiet_config()
    })
    .unwrap();
    f.insert(ParticleDesc {
        position: Vec3A::new(0.46, 0., 0.),
        velocity: Vec3A::NEG_X * 5.,
    })
    .unwrap();
    let id = w.add_fluid(f);
    w.step();
    assert!((w.fluid(id).unwrap().particles[0].velocity.x - 2.5).abs() < 1e-5);
}
#[test]
fn closed_dam_break_conserves_mass_and_spreads_under_gravity() {
    let cfg = FluidConfig {
        bounds_min: Vec3A::new(-1., 0., -1.),
        bounds_max: Vec3A::new(1., 2., 1.),
        ..Default::default()
    };
    let mut f = Fluid::new(cfg).unwrap();
    for x in 0..5 {
        for y in 0..6 {
            for z in 0..5 {
                f.insert(ParticleDesc::new(Vec3A::new(
                    -0.65 + x as f32 * 0.1,
                    0.7 + y as f32 * 0.1,
                    -0.2 + z as f32 * 0.1,
                )))
                .unwrap();
            }
        }
    }
    let mass = f.total_mass();
    let initial_y = centre(&f).y;
    let initial_width = 0.4;
    for _ in 0..240 {
        f.step(1. / 120.).unwrap();
    }
    let lo = f
        .particles
        .iter()
        .map(|p| p.position.x)
        .fold(f32::INFINITY, f32::min);
    let hi = f
        .particles
        .iter()
        .map(|p| p.position.x)
        .fold(f32::NEG_INFINITY, f32::max);
    assert_eq!(f.particles.len(), 150);
    assert_eq!(mass, f.total_mass());
    assert!(centre(&f).y < initial_y * 0.5);
    assert!(hi - lo > initial_width * 1.5, "width {}", hi - lo);
    assert!(
        f.particles
            .iter()
            .all(|p| p.position.y >= cfg.particle_radius
                && p.velocity.is_finite()
                && p.density.is_finite()
                && p.density > 0.)
    );
    assert_eq!(f.stats.speed_limit_hits, 0);
}
#[test]
fn coincident_particles_and_empty_fluid_stay_finite() {
    let mut f = Fluid::new(quiet_config()).unwrap();
    f.step(1. / 120.).unwrap();
    assert_eq!(f.stats.min_density, 0.);
    for _ in 0..12 {
        f.insert(ParticleDesc::new(Vec3A::ZERO)).unwrap();
    }
    for _ in 0..30 {
        f.step(1. / 120.).unwrap();
    }
    assert!(
        f.particles
            .iter()
            .all(|p| p.position.is_finite() && p.velocity.is_finite() && p.density.is_finite())
    );
    assert_eq!(f.particles.len(), 12);
}
#[test]
fn fixed_world_clock_pauses_fluid_and_interpolation_uses_previous_tick() {
    let mut w = World::new(Config::default()).unwrap();
    let mut f = Fluid::new(quiet_config()).unwrap();
    f.insert(ParticleDesc::new(Vec3A::Y * 5.)).unwrap();
    let id = w.add_fluid(f);
    w.advance(1. / 60., false).unwrap();
    let f = w.fluid(id).unwrap();
    assert_eq!(f.stats.steps, 2);
    // World gravity overrides the standalone zero gravity configuration.
    assert!(f.particles[0].velocity.y < 0.);
    let p = f.particles[0];
    assert_eq!(p.interpolated_position(0.), p.previous);
    assert_eq!(p.interpolated_position(1.), p.position);
    assert_eq!(w.advance(10., true).unwrap(), 0);
    assert_eq!(w.fluid(id).unwrap().particles[0].position, p.position);
    assert_eq!(w.stats().fluid_particles, 1);
}
#[test]
fn world_replay_is_identical_for_60hz_and_120hz_frames() {
    let mut a = World::new(Config::default()).unwrap();
    let mut b = World::new(Config::default()).unwrap();
    for w in [&mut a, &mut b] {
        let mut f = Fluid::new(FluidConfig::default()).unwrap();
        for i in 0..6 {
            f.insert(ParticleDesc::new(Vec3A::new(i as f32 * 0.1, 1., 0.)))
                .unwrap();
        }
        w.add_fluid(f);
    }
    for _ in 0..60 {
        a.advance(1. / 60., false).unwrap();
    }
    for _ in 0..120 {
        b.advance(1. / 120., false).unwrap();
    }
    for (x, y) in a.fluids[0].particles.iter().zip(&b.fluids[0].particles) {
        assert_eq!(x.position, y.position);
        assert_eq!(x.velocity, y.velocity);
    }
}
#[test]
fn fluid_projects_out_of_sphere_and_rotated_box_solid_boundaries() {
    let mut w = World::new(Config {
        gravity: Vec3A::ZERO,
        ..Default::default()
    })
    .unwrap();
    w.insert(BodyDesc::fixed(Shape::Sphere { radius: 0.4 }, Vec3A::ZERO))
        .unwrap();
    let mut box_desc = BodyDesc::fixed(
        Shape::Box {
            half_extents: Vec3A::splat(0.3),
        },
        Vec3A::new(1., 0., 0.),
    );
    box_desc.rotation = Quat::from_rotation_z(0.5);
    let box_id = w.insert(box_desc).unwrap();
    let mut f = Fluid::new(quiet_config()).unwrap();
    f.insert(ParticleDesc::new(Vec3A::new(0.3, 0., 0.)))
        .unwrap();
    f.insert(ParticleDesc::new(Vec3A::new(1.1, 0., 0.)))
        .unwrap();
    let fluid_id = w.add_fluid(f);
    w.step();
    let f = w.fluid(fluid_id).unwrap();
    assert!(f.particles[0].position.length() >= 0.4499);
    let pose = w.body(box_id).unwrap().pose();
    let local = pose.rotation.conjugate() * (f.particles[1].position - pose.position);
    assert!(local.abs().max_element() >= 0.3499);
    assert!(f.stats.boundary_contacts > 0);
}
#[test]
fn high_velocity_substeps_are_bounded_and_reported() {
    let mut f = Fluid::new(quiet_config()).unwrap();
    f.insert(ParticleDesc {
        position: Vec3A::ZERO,
        velocity: Vec3A::X * 100.,
    })
    .unwrap();
    f.step(0.05).unwrap();
    assert_eq!(f.stats.substeps, 16);
    assert_eq!(f.stats.motion_limit_hits, 1);
    assert!(f.particles[0].position.is_finite());
}
#[test]
fn invalid_configuration_particles_and_steps_are_transactional() {
    assert!(
        Fluid::new(FluidConfig {
            smoothing_radius: f32::NAN,
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        Fluid::new(FluidConfig {
            particle_mass: 0.,
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        Fluid::new(FluidConfig {
            max_substeps: 0,
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        Fluid::new(FluidConfig {
            bounds_max: Vec3A::ZERO,
            ..Default::default()
        })
        .is_err()
    );
    let mut f = Fluid::new(quiet_config()).unwrap();
    assert!(f.insert(ParticleDesc::new(Vec3A::splat(f32::NAN))).is_err());
    assert!(f.insert(ParticleDesc::new(Vec3A::X * 100.)).is_err());
    assert_eq!(f.particles.len(), 0);
    f.insert(ParticleDesc::new(Vec3A::ZERO)).unwrap();
    assert!(f.step(-0.1).is_err());
    assert!(f.step(f32::NAN).is_err());
    assert_eq!(f.stats.steps, 0);
    assert!(f.add_velocity(Vec3A::X * 101.).is_err());
    assert_eq!(f.particles[0].velocity, Vec3A::ZERO);
}

#[test]
fn moving_rigid_boundary_pushes_fluid_without_receiving_reaction_impulse() {
    let mut w = World::new(Config {
        gravity: Vec3A::ZERO,
        ..Default::default()
    })
    .unwrap();
    let mut d = BodyDesc::dynamic(Shape::Sphere { radius: 0.4 }, Vec3A::ZERO, 1.);
    d.velocity = Vec3A::X;
    let body = w.insert(d).unwrap();
    let mut f = Fluid::new(quiet_config()).unwrap();
    f.insert(ParticleDesc::new(Vec3A::X * 0.445)).unwrap();
    let id = w.add_fluid(f);
    w.step();
    let p = w.fluid(id).unwrap().particles()[0];
    assert!(p.position().x > 0.45 && p.velocity().x > 0.5);
    assert_eq!(w.body(body).unwrap().velocity(), Vec3A::X);
}
