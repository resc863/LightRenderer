use super::*;
fn ball(r: f32) -> Shape {
    Shape::Sphere { radius: r }
}
fn cube(h: f32) -> Shape {
    Shape::Box {
        half_extents: Vec3A::splat(h),
    }
}
fn floor(w: &mut World) {
    w.insert(BodyDesc::fixed(
        Shape::Box {
            half_extents: Vec3A::new(20., 0.1, 20.),
        },
        Vec3A::new(0., -0.1, 0.),
    ))
    .unwrap();
}
fn simulate(w: &mut World, ticks: usize) {
    for _ in 0..ticks {
        w.step();
    }
}
#[test]
fn gravity_matches_semi_implicit_euler() {
    let mut w = World::new(Config::default()).unwrap();
    let id = w
        .insert(BodyDesc::dynamic(ball(0.2), Vec3A::Y * 100., 1.))
        .unwrap();
    simulate(&mut w, 120);
    let b = w.body(id).unwrap();
    assert!((b.velocity().y + 9.81).abs() < 0.0001);
    assert!((b.pose().position.y - (100. - 9.81 * 121. / 240.)).abs() < 0.001);
}
#[test]
fn resting_sphere_sleeps_and_impulse_wakes_it() {
    let mut w = World::new(Config::default()).unwrap();
    floor(&mut w);
    let id = w
        .insert(BodyDesc::dynamic(ball(0.3), Vec3A::Y * 2., 1.))
        .unwrap();
    simulate(&mut w, 1200);
    let b = w.body(id).unwrap();
    assert!(
        (b.pose().position.y - 0.3).abs() < 0.01,
        "{}",
        b.pose().position.y
    );
    assert!(b.is_sleeping());
    let p = b.pose().position;
    w.apply_impulse(id, Vec3A::Y * 2., p).unwrap();
    assert!(!w.body(id).unwrap().is_sleeping());
    simulate(&mut w, 12);
    assert!(w.body(id).unwrap().pose().position.y > p.y + 0.1);
}
#[test]
fn elastic_spheres_conserve_linear_momentum() {
    let mut w = World::new(Config {
        gravity: Vec3A::ZERO,
        ..Default::default()
    })
    .unwrap();
    let mut d = BodyDesc::dynamic(ball(0.5), Vec3A::new(-1., 0., 0.), 1.);
    d.velocity = Vec3A::X * 2.;
    d.restitution = 1.;
    d.friction = 0.;
    d.can_sleep = false;
    let a = w.insert(d).unwrap();
    d.position = Vec3A::new(1., 0., 0.);
    d.velocity = Vec3A::NEG_X * 2.;
    let b = w.insert(d).unwrap();
    simulate(&mut w, 60);
    assert!(w.body(a).unwrap().velocity().x < -1.9);
    assert!(w.body(b).unwrap().velocity().x > 1.9);
    assert!((w.body(a).unwrap().velocity() + w.body(b).unwrap().velocity()).length() < 1e-5);
}
#[test]
fn off_center_impulse_uses_inertia_and_integrates_rotation() {
    let mut w = World::new(Config {
        gravity: Vec3A::ZERO,
        ..Default::default()
    })
    .unwrap();
    let id = w
        .insert(BodyDesc::dynamic(cube(0.5), Vec3A::ZERO, 2.))
        .unwrap();
    w.apply_impulse(id, Vec3A::Y * 2., Vec3A::X * 0.5).unwrap();
    let b = w.body(id).unwrap();
    assert!((b.velocity().y - 1.).abs() < 1e-5);
    assert!((b.angular_velocity().z - 3.).abs() < 1e-5);
    simulate(&mut w, 30);
    let b = w.body(id).unwrap();
    assert!(b.pose().rotation.dot(Quat::IDENTITY).abs() < 0.99);
    assert!((b.pose().rotation.length() - 1.).abs() < 1e-5);
}
#[test]
fn sat_rejects_aabb_false_positive_and_clips_face_contacts() {
    let mut w = World::new(Config::default()).unwrap();
    let mut d = BodyDesc::dynamic(
        Shape::Box {
            half_extents: Vec3A::new(2., 0.1, 0.1),
        },
        Vec3A::ZERO,
        1.,
    );
    d.rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4);
    let a = w.insert(d).unwrap();
    d.position = Vec3A::new(-0.3, 0.3, 0.);
    let b = w.insert(d).unwrap();
    let a = w.body(a).unwrap();
    let b = w.body(b).unwrap();
    assert!(a.bounds().overlaps(b.bounds()));
    assert!(collide(a, b).is_none());
    let mut w = World::new(Config::default()).unwrap();
    let a = w
        .insert(BodyDesc::dynamic(cube(0.5), Vec3A::ZERO, 1.))
        .unwrap();
    let b = w
        .insert(BodyDesc::dynamic(cube(0.5), Vec3A::Y * 0.9, 1.))
        .unwrap();
    let m = collide(w.body(a).unwrap(), w.body(b).unwrap()).unwrap();
    assert_eq!(m.count, 4);
    assert!((m.depth - 0.1).abs() < 1e-5);
    assert!(m.normal.dot(Vec3A::Y) > 0.999);
}
#[test]
fn embedded_sphere_is_expelled_and_coincident_centers_stay_finite() {
    let mut w = World::new(Config {
        gravity: Vec3A::ZERO,
        ..Default::default()
    })
    .unwrap();
    w.insert(BodyDesc::fixed(cube(1.), Vec3A::ZERO)).unwrap();
    let id = w
        .insert(BodyDesc::dynamic(ball(0.2), Vec3A::new(0.9, 0., 0.), 1.))
        .unwrap();
    simulate(&mut w, 30);
    assert!(w.body(id).unwrap().pose().position.x >= 1.19);
    let mut w = World::new(Config {
        gravity: Vec3A::ZERO,
        ..Default::default()
    })
    .unwrap();
    w.insert(BodyDesc::dynamic(ball(0.5), Vec3A::ZERO, 1.))
        .unwrap();
    w.insert(BodyDesc::dynamic(ball(0.5), Vec3A::ZERO, 1.))
        .unwrap();
    simulate(&mut w, 20);
    assert!(w.bodies().all(|(_, b)| b.pose().position.is_finite()));
}
#[test]
fn box_stack_remains_above_ground_and_stable() {
    let mut w = World::new(Config::default()).unwrap();
    floor(&mut w);
    let mut ids = vec![];
    for i in 0..5 {
        let mut d = BodyDesc::dynamic(cube(0.25), Vec3A::Y * (0.26 + i as f32 * 0.51), 1.);
        d.restitution = 0.;
        ids.push(w.insert(d).unwrap());
    }
    simulate(&mut w, 1200);
    for (i, id) in ids.iter().enumerate() {
        let b = w.body(*id).unwrap();
        assert!(
            (b.pose().position.y - (0.25 + i as f32 * 0.5)).abs() < 0.09,
            "layer {i}: {:?}",
            b.pose().position
        );
        assert!(b.pose().position.x.abs() < 0.1 && b.pose().position.z.abs() < 0.1);
    }
}
#[test]
fn friction_reduces_sliding_speed() {
    let mut w = World::new(Config::default()).unwrap();
    floor(&mut w);
    let mut d = BodyDesc::dynamic(cube(0.25), Vec3A::Y * 0.25, 1.);
    d.velocity = Vec3A::X * 3.;
    d.restitution = 0.;
    let id = w.insert(d).unwrap();
    simulate(&mut w, 240);
    assert!(w.body(id).unwrap().velocity().x.abs() < 0.15);
}
#[test]
fn motion_substeps_stop_fast_sphere_at_thin_floor() {
    let mut w = World::new(Config::default()).unwrap();
    floor(&mut w);
    let mut d = BodyDesc::dynamic(ball(0.1), Vec3A::Y * 1., 1.);
    d.velocity = Vec3A::NEG_Y * 100.;
    d.motion_substeps = true;
    d.restitution = 0.;
    let id = w.insert(d).unwrap();
    simulate(&mut w, 12);
    assert!(w.body(id).unwrap().pose().position.y > 0.09);
    assert!(w.stats().motion_substeps > w.stats().steps);
}
#[test]
fn fixed_clock_pause_stalls_and_interpolation() {
    let mut a = World::new(Config::default()).unwrap();
    let mut b = World::new(Config::default()).unwrap();
    let id = a
        .insert(BodyDesc::dynamic(ball(0.2), Vec3A::Y * 100., 1.))
        .unwrap();
    b.insert(BodyDesc::dynamic(ball(0.2), Vec3A::Y * 100., 1.))
        .unwrap();
    for _ in 0..60 {
        a.advance(1. / 60., false).unwrap();
    }
    for _ in 0..120 {
        b.advance(1. / 120., false).unwrap();
    }
    assert_eq!(a.stats.steps, b.stats.steps);
    assert_eq!(
        a.body(id).unwrap().pose().position,
        b.body(id).unwrap().pose().position
    );
    let p = a.body(id).unwrap().pose().position;
    assert_eq!(a.advance(10., true).unwrap(), 0);
    assert_eq!(a.body(id).unwrap().pose().position, p);
    assert_eq!(a.advance(2., false).unwrap(), 8);
    assert!(a.stats.dropped_seconds > 1.9);
    assert!((0. ..1.).contains(&a.interpolation_alpha()));
    let body = a.body(id).unwrap();
    assert_eq!(body.interpolated_pose(0.).position, body.previous.position);
    assert_eq!(body.interpolated_pose(1.).position, body.pose.position);
}
#[test]
fn broad_phase_matches_brute_force_and_has_no_duplicates() {
    let mut w = World::new(Config {
        gravity: Vec3A::ZERO,
        ..Default::default()
    })
    .unwrap();
    for i in 0..80 {
        let mut d = BodyDesc::dynamic(
            if i % 2 == 0 { ball(0.35) } else { cube(0.3) },
            Vec3A::new(
                (i % 7) as f32 * 0.6,
                ((i / 7) % 4) as f32 * 0.5,
                (i / 28) as f32 * 0.6,
            ),
            1.,
        );
        d.rotation = Quat::from_rotation_y(i as f32 * 0.37);
        w.insert(d).unwrap();
    }
    let mut expected = vec![];
    for a in 0..w.bodies.len() {
        for b in a + 1..w.bodies.len() {
            if collide(&w.bodies[a], &w.bodies[b]).is_some() {
                expected.push((a, b));
            }
        }
    }
    w.detect_contacts(w.config.fixed_dt);
    let mut actual: Vec<_> = w
        .contacts
        .iter()
        .map(|c| (c.a.min(c.b), c.a.max(c.b)))
        .collect();
    actual.sort_unstable();
    actual.dedup();
    expected.sort_unstable();
    assert_eq!(actual, expected);
    let mut points = std::collections::HashSet::new();
    for c in &w.contacts {
        assert!(
            points.insert((c.a, c.b, c.ra.to_array().map(f32::to_bits))),
            "Duplicate contact"
        );
    }
}
#[test]
fn invalid_input_is_rejected_without_mutating_world() {
    assert!(
        World::new(Config {
            fixed_dt: f32::NAN,
            ..Default::default()
        })
        .is_err()
    );
    let mut w = World::new(Config::default()).unwrap();
    assert!(
        w.insert(BodyDesc::dynamic(ball(-1.), Vec3A::ZERO, 1.))
            .is_err()
    );
    assert!(
        w.insert(BodyDesc::dynamic(ball(0.2), Vec3A::ZERO, -1.))
            .is_err()
    );
    assert!(w.advance(f32::NAN, false).is_err());
    assert_eq!(w.bodies.len(), 0);
    assert_eq!(w.stats.steps, 0);
}

#[test]
fn impact_wakes_sleeping_body_and_transfers_momentum() {
    let mut w = World::new(Config::default()).unwrap();
    floor(&mut w);
    let lower = w
        .insert(BodyDesc::dynamic(ball(0.3), Vec3A::Y * 0.3, 1.))
        .unwrap();
    simulate(&mut w, 240);
    assert!(w.body(lower).unwrap().is_sleeping());
    let mut d = BodyDesc::dynamic(ball(0.3), Vec3A::new(0.12, 1.2, 0.), 1.);
    d.velocity = Vec3A::NEG_Y * 2.;
    w.insert(d).unwrap();
    let mut woke = false;
    for _ in 0..60 {
        w.step();
        woke |= !w.body(lower).unwrap().is_sleeping();
    }
    assert!(woke);
    assert!(w.body(lower).unwrap().pose().position.x.abs() > 0.02);
}

#[test]
fn rotated_box_falls_and_settles_without_crossing_floor() {
    let mut w = World::new(Config::default()).unwrap();
    floor(&mut w);
    let mut d = BodyDesc::dynamic(cube(0.3), Vec3A::Y * 3., 1.);
    d.rotation = Quat::from_euler(glam::EulerRot::XYZ, 0.3, 0.6, 0.2);
    d.angular_velocity = Vec3A::new(0.2, 0.4, 0.7);
    let id = w.insert(d).unwrap();
    simulate(&mut w, 1800);
    let b = w.body(id).unwrap();
    assert!(b.bounds().min.y > -0.02);
    assert!(b.pose().position.y < 0.6);
    assert!(b.angular_velocity().length() < 0.2);
}

#[test]
fn motion_limit_is_bounded_and_reported() {
    let mut w = World::new(Config {
        gravity: Vec3A::ZERO,
        ..Default::default()
    })
    .unwrap();
    let mut d = BodyDesc::dynamic(ball(0.1), Vec3A::ZERO, 1.);
    d.velocity = Vec3A::X * 1000.;
    d.motion_substeps = true;
    w.insert(d).unwrap();
    w.step();
    assert_eq!(w.stats.motion_substeps, MAX_MOTION_SUBSTEPS as u64);
    assert_eq!(w.stats.motion_limit_hits, 1);
}
