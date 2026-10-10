use bytemuck::{Pod, Zeroable};
use glam::{Quat, Vec3};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Object {
    pub center_kind: [f32; 4],
    pub size: [f32; 4],
    pub color: [f32; 4],
    pub rotation: [f32; 4],
}

impl Object {
    pub fn new(center: Vec3, size: Vec3, color: Vec3, kind: f32, reflection: f32, q: Quat) -> Self {
        Self {
            center_kind: [center.x, center.y, center.z, kind],
            size: [size.x, size.y, size.z, 0.0],
            color: [color.x, color.y, color.z, reflection],
            rotation: q.to_array(),
        }
    }
    pub fn bounds(&self) -> [f32; 6] {
        let q = Quat::from_array(self.rotation);
        let s = Vec3::from_slice(&self.size);
        let e = (q * Vec3::X).abs() * s.x + (q * Vec3::Y).abs() * s.y + (q * Vec3::Z).abs() * s.z;
        let p = Vec3::from_slice(&self.center_kind);
        let lo = p - e - Vec3::splat(0.002);
        let hi = p + e + Vec3::splat(0.002);
        [lo.x, lo.y, lo.z, hi.x, hi.y, hi.z]
    }
}

const WHITE: Vec3 = Vec3::new(0.92, 0.93, 0.88);
const INK: Vec3 = Vec3::new(0.045, 0.055, 0.13);
const TEAL: Vec3 = Vec3::new(0.07, 0.61, 0.63);
const RED: Vec3 = Vec3::new(0.86, 0.16, 0.16);

pub fn garden(time: f32) -> Vec<Object> {
    garden_with_environment(time).0
}

/// Static collision proxies for the environment, excluding mascot, grass and petals.
/// Boxes are exact OBBs. Tree trunks use conservative box proxies.
pub fn environment() -> Vec<Object> {
    let (s, count) = garden_with_environment(0.);
    s[..count]
        .iter()
        .filter(|o| o.center_kind[3] > 0.5 || (o.size[1] > 1.5 && o.size[0] < 0.3))
        .copied()
        .collect()
}

fn garden_with_environment(time: f32) -> (Vec<Object>, usize) {
    let mut s = Vec::with_capacity(220);
    let mut add =
        |p: Vec3, r: Vec3, c: Vec3, kind, refl, q| s.push(Object::new(p, r, c, kind, refl, q));
    // An analytic ray-traced water plane and floating stone garden.
    add(
        Vec3::new(0.0, -0.45, 0.0),
        Vec3::new(90.0, 0.12, 90.0),
        Vec3::new(0.10, 0.51, 0.60),
        1.0,
        0.52,
        Quat::IDENTITY,
    );
    add(
        Vec3::new(0.0, -0.10, 2.0),
        Vec3::new(7.4, 0.35, 7.0),
        Vec3::new(0.28, 0.40, 0.42),
        1.0,
        0.04,
        Quat::IDENTITY,
    );
    add(
        Vec3::new(0.0, 0.23, 2.0),
        Vec3::new(7.2, 0.10, 6.8),
        Vec3::new(0.75, 0.80, 0.73),
        1.0,
        0.05,
        Quat::IDENTITY,
    );
    for i in 0..9 {
        add(
            Vec3::new(0.0, 0.36, -4.5 + i as f32 * 1.2),
            Vec3::new(1.05, 0.04, 0.46),
            WHITE,
            1.0,
            0.08,
            Quat::IDENTITY,
        );
    }
    for side in [-1.0, 1.0] {
        add(
            Vec3::new(side * 4.7, 0.38, 2.0),
            Vec3::new(1.8, 0.08, 5.0),
            Vec3::new(0.23, 0.53, 0.40),
            1.0,
            0.0,
            Quat::IDENTITY,
        );
        // Torii, with black caps and paired beams.
        add(
            Vec3::new(side * 2.8, 2.8, 4.3),
            Vec3::new(0.24, 2.5, 0.24),
            RED,
            1.0,
            0.06,
            Quat::from_rotation_z(side * -0.035),
        );
        add(
            Vec3::new(side * 2.8, 0.54, 4.3),
            Vec3::new(0.4, 0.24, 0.4),
            INK,
            1.0,
            0.06,
            Quat::IDENTITY,
        );
        add(
            Vec3::new(side * 3.58, 5.62, 4.3),
            Vec3::new(0.87, 0.13, 0.35),
            INK,
            1.0,
            0.02,
            Quat::from_rotation_z(side * 0.10),
        );
        for z in [-2.0, 2.8, 6.5] {
            let x = side * 2.3;
            add(
                Vec3::new(x, 0.60, z),
                Vec3::splat(0.28),
                Vec3::new(0.40, 0.46, 0.46),
                1.0,
                0.0,
                Quat::IDENTITY,
            );
            add(
                Vec3::new(x, 1.0, z),
                Vec3::new(0.12, 0.35, 0.12),
                INK,
                1.0,
                0.0,
                Quat::IDENTITY,
            );
            add(
                Vec3::new(x, 1.46, z),
                Vec3::new(0.23, 0.22, 0.23),
                Vec3::new(1.0, 0.72, 0.30),
                1.0,
                -1.0,
                Quat::IDENTITY,
            );
            add(
                Vec3::new(x, 1.74, z),
                Vec3::new(0.38, 0.07, 0.38),
                INK,
                1.0,
                0.0,
                Quat::from_rotation_y(0.3),
            );
        }
        // Sakura trees: individually intersected blossoms and branches.
        for (index, z) in [1.3, 6.2].into_iter().enumerate() {
            let x = side * (4.5 + index as f32 * 0.5);
            let base = Vec3::new(x, 0.5, z);
            add(
                base + Vec3::Y * 1.6,
                Vec3::new(0.22, 1.6, 0.22),
                Vec3::new(0.27, 0.15, 0.23),
                0.0,
                0.0,
                Quat::from_rotation_z(side * 0.15),
            );
            for j in 0..7 {
                let a = j as f32 * 2.399 + index as f32;
                let radius = if j == 0 { 0.0 } else { 1.1 };
                let p = base
                    + Vec3::new(
                        a.cos() * radius,
                        3.2 + (a * 1.7).sin() * 0.45,
                        a.sin() * radius,
                    );
                let q = Quat::from_rotation_z((time * 0.8 + a).sin() * 0.018);
                add(
                    p,
                    Vec3::new(1.1, 0.70, 1.0),
                    Vec3::new(1.0, 0.48 + j as f32 * 0.022, 0.66 + j as f32 * 0.015),
                    0.0,
                    0.0,
                    q,
                );
                let midpoint = (p + base + Vec3::Y * 1.7) * 0.5;
                let d = p - (base + Vec3::Y * 1.7);
                add(
                    midpoint,
                    Vec3::new(0.075, d.length() * 0.5, 0.075),
                    Vec3::new(0.27, 0.15, 0.23),
                    0.0,
                    0.0,
                    Quat::from_rotation_arc(Vec3::Y, d.normalize()),
                );
            }
        }
        for j in 0..12 {
            let x = side * (3.2 + (j as f32 * 1.1).sin().abs() * 2.5);
            let z = -2.5 + j as f32 * 0.75;
            add(
                Vec3::new(x, 0.7, z),
                Vec3::new(0.045, 0.28, 0.13),
                Vec3::new(0.17, 0.45 + (j % 3) as f32 * 0.05, 0.31),
                0.0,
                0.0,
                Quat::from_rotation_z((time * 1.8 + j as f32).sin() * 0.17),
            );
        }
    }
    add(
        Vec3::new(0.0, 4.6, 4.3),
        Vec3::new(3.45, 0.16, 0.25),
        RED,
        1.0,
        0.03,
        Quat::IDENTITY,
    );
    add(
        Vec3::new(0.0, 5.35, 4.3),
        Vec3::new(3.8, 0.20, 0.32),
        RED,
        1.0,
        0.03,
        Quat::IDENTITY,
    );
    add(
        Vec3::new(0.0, 5.58, 4.3),
        Vec3::new(3.0, 0.13, 0.36),
        INK,
        1.0,
        0.02,
        Quat::IDENTITY,
    );
    add(
        Vec3::new(0.0, 4.95, 4.02),
        Vec3::new(0.28, 0.48, 0.05),
        Vec3::new(0.91, 0.68, 0.25),
        1.0,
        0.1,
        Quat::IDENTITY,
    );
    let environment_count = s.len();
    let mut add = |p, r, c, kind, refl, q| s.push(Object::new(p, r, c, kind, refl, q));
    // Original procedural anime mascot: teal hair, sailor outfit, ribbons and boots.
    let lift = 0.035 * (time * 2.2).sin();
    let root = Vec3::new(0.0, 0.38 + lift, 0.0);
    let skin = Vec3::new(1.0, 0.79, 0.67);
    for side in [-1.0, 1.0] {
        add(
            root + Vec3::new(side * 0.20, 0.44, 0.0),
            Vec3::new(0.115, 0.39, 0.13),
            INK,
            0.0,
            0.08,
            Quat::from_rotation_z(side * 0.05),
        );
        add(
            root + Vec3::new(side * 0.21, 0.09, -0.10),
            Vec3::new(0.15, 0.12, 0.25),
            INK,
            0.0,
            0.2,
            Quat::IDENTITY,
        );
        add(
            root + Vec3::new(side * 0.21, 0.76, -0.02),
            Vec3::new(0.12, 0.18, 0.12),
            skin,
            0.0,
            0.0,
            Quat::IDENTITY,
        );
        let wave = if side < 0.0 {
            (time * 2.2).sin() * 0.12
        } else {
            0.0
        };
        add(
            root + Vec3::new(side * 0.53, 1.51, 0.0),
            Vec3::new(0.16, 0.30, 0.16),
            WHITE,
            0.0,
            0.0,
            Quat::from_rotation_z(side * 0.48 + wave),
        );
        add(
            root + Vec3::new(side * 0.65, 1.12, -0.08),
            Vec3::new(0.095, 0.24, 0.10),
            skin,
            0.0,
            0.0,
            Quat::from_rotation_z(side * 0.15 + wave),
        );
        add(
            root + Vec3::new(side * 0.70, 0.89, -0.08),
            Vec3::new(0.12, 0.13, 0.10),
            skin,
            0.0,
            0.0,
            Quat::IDENTITY,
        );
        let sway = (time * 1.7 + side * 0.5).sin() * 0.13;
        add(
            root + Vec3::new(side * 0.64, 1.81, 0.22),
            Vec3::new(0.23, 0.70, 0.22),
            TEAL,
            0.0,
            0.07,
            Quat::from_rotation_z(side * -0.14 + sway),
        );
        add(
            root + Vec3::new(side * 0.50, 2.37, 0.10),
            Vec3::new(0.17, 0.11, 0.16),
            Vec3::new(0.97, 0.26, 0.43),
            1.0,
            0.0,
            Quat::from_rotation_z(side * 0.25),
        );
    }
    add(
        root + Vec3::new(0.0, 1.08, 0.0),
        Vec3::new(0.49, 0.31, 0.34),
        Vec3::new(0.12, 0.19, 0.34),
        0.0,
        0.02,
        Quat::IDENTITY,
    );
    for j in 0..9 {
        let a = j as f32 * std::f32::consts::TAU / 9.0;
        add(
            root + Vec3::new(a.cos() * 0.34, 1.01, a.sin() * 0.23),
            Vec3::new(0.115, 0.22, 0.13),
            Vec3::new(0.13, 0.23, 0.41),
            0.0,
            0.01,
            Quat::from_rotation_y(-a),
        );
    }
    add(
        root + Vec3::new(0.0, 1.56, 0.0),
        Vec3::new(0.34, 0.39, 0.23),
        WHITE,
        0.0,
        0.0,
        Quat::IDENTITY,
    );
    add(
        root + Vec3::new(0.0, 1.88, 0.0),
        Vec3::new(0.13, 0.15, 0.13),
        skin,
        0.0,
        0.0,
        Quat::IDENTITY,
    );
    add(
        root + Vec3::new(0.0, 1.63, -0.235),
        Vec3::new(0.25, 0.15, 0.028),
        TEAL,
        1.0,
        0.0,
        Quat::IDENTITY,
    );
    add(
        root + Vec3::new(0.0, 1.49, -0.27),
        Vec3::new(0.09, 0.19, 0.036),
        Vec3::new(0.96, 0.29, 0.45),
        0.0,
        0.0,
        Quat::IDENTITY,
    );
    add(
        root + Vec3::new(0.0, 2.22, 0.03),
        Vec3::new(0.46, 0.49, 0.40),
        skin,
        0.0,
        0.0,
        Quat::IDENTITY,
    );
    add(
        root + Vec3::new(0.0, 2.40, 0.16),
        Vec3::new(0.49, 0.38, 0.39),
        TEAL,
        0.0,
        0.07,
        Quat::IDENTITY,
    );
    for j in 0..5 {
        let x = (j as f32 - 2.0) * 0.16;
        add(
            root + Vec3::new(x, 2.46 - (j % 2) as f32 * 0.055, -0.24),
            Vec3::new(0.13, 0.23, 0.14),
            TEAL,
            0.0,
            0.03,
            Quat::from_rotation_z(x * 0.4),
        );
    }
    for side in [-1.0, 1.0] {
        add(
            root + Vec3::new(side * 0.195, 2.22, -0.345),
            Vec3::new(0.112, 0.145, 0.035),
            INK,
            0.0,
            0.0,
            Quat::IDENTITY,
        );
        add(
            root + Vec3::new(side * 0.195, 2.21, -0.371),
            Vec3::new(0.09, 0.116, 0.018),
            WHITE,
            0.0,
            0.0,
            Quat::IDENTITY,
        );
        add(
            root + Vec3::new(side * 0.185, 2.205, -0.389),
            Vec3::new(0.056, 0.084, 0.012),
            Vec3::new(0.09, 0.51, 0.71),
            0.0,
            0.12,
            Quat::IDENTITY,
        );
        add(
            root + Vec3::new(side * 0.182, 2.224, -0.401),
            Vec3::new(0.024, 0.048, 0.006),
            INK,
            0.0,
            0.0,
            Quat::IDENTITY,
        );
        add(
            root + Vec3::new(side * 0.185 - 0.018, 2.25, -0.408),
            Vec3::new(0.019, 0.025, 0.008),
            WHITE,
            0.0,
            -1.0,
            Quat::IDENTITY,
        );
        add(
            root + Vec3::new(side * 0.28, 2.04, -0.32),
            Vec3::new(0.058, 0.025, 0.018),
            Vec3::new(1.0, 0.48, 0.52),
            0.0,
            0.0,
            Quat::IDENTITY,
        );
    }
    add(
        root + Vec3::new(0.0, 1.993, -0.341),
        Vec3::new(0.068, 0.012, 0.013),
        Vec3::new(0.55, 0.22, 0.30),
        0.0,
        0.0,
        Quat::IDENTITY,
    );
    // Airborne petals are real ray-traced ellipsoids, with independent animation.
    for i in 0..24 {
        let f = i as f32;
        let p = Vec3::new(
            (f * 2.4 + time * 0.18).sin() * 6.5,
            0.8 + (f * 0.71 - time * 0.4).rem_euclid(4.4),
            (f * 1.91 + time * 0.2).cos() * 5.5 + 1.5,
        );
        add(
            p,
            Vec3::new(0.06, 0.014, 0.10),
            Vec3::new(1.0, 0.59, 0.72),
            0.0,
            0.0,
            Quat::from_euler(glam::EulerRot::XYZ, f + time, f, time * 0.7),
        );
    }
    (s, environment_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotated_bounds_contain_every_box_corner() {
        for o in garden(1.23) {
            let b = o.bounds();
            let q = Quat::from_array(o.rotation);
            let p = Vec3::from_slice(&o.center_kind);
            let r = Vec3::from_slice(&o.size);
            for x in [-1.0, 1.0] {
                for y in [-1.0, 1.0] {
                    for z in [-1.0, 1.0] {
                        let v = p + q * (r * Vec3::new(x, y, z));
                        assert!(
                            v.x >= b[0]
                                && v.x <= b[3]
                                && v.y >= b[1]
                                && v.y <= b[4]
                                && v.z >= b[2]
                                && v.z <= b[5]
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn animation_preserves_primitive_order_and_valid_geometry() {
        let first = garden(0.0);
        for t in [0.1, 9.0, 100.0] {
            let frame = garden(t);
            assert_eq!(frame.len(), first.len());
            assert!(
                frame
                    .iter()
                    .all(|o| o.size[..3].iter().all(|s| s.is_finite() && *s > 0.0))
            );
            assert!(
                frame
                    .iter()
                    .zip(&first)
                    .any(|(a, b)| a.center_kind != b.center_kind)
            );
        }
    }
}
