use glam::Vec3;

pub struct Camera {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub orbit: bool,
    pub walking: bool,
    pub radius: f32,
    pub target: Vec3,
    velocity_y: f32,
}
impl Default for Camera {
    fn default() -> Self {
        let mut c = Self {
            position: Vec3::ZERO,
            yaw: 0.25,
            pitch: -0.13,
            orbit: true,
            walking: false,
            radius: 11.3,
            target: Vec3::new(0.0, 2.15, 1.0),
            velocity_y: 0.0,
        };
        c.update_orbit();
        c
    }
}
impl Camera {
    pub fn forward(&self) -> Vec3 {
        Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.cos() * self.pitch.cos(),
        )
        .normalize()
    }
    pub fn basis(&self, aspect: f32) -> (Vec3, Vec3, Vec3) {
        let f = self.forward();
        let r = Vec3::Y.cross(f).normalize();
        let u = f.cross(r).normalize();
        let fov = (50.0f32.to_radians() * 0.5).tan();
        (r * fov * aspect, u * fov, f)
    }
    pub fn update_orbit(&mut self) {
        if self.orbit {
            self.position = self.target - self.forward() * self.radius;
        }
    }
    pub fn look(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * 0.0025;
        self.pitch = (self.pitch - dy * 0.0025).clamp(-1.45, 1.45);
        self.update_orbit();
    }
    pub fn zoom(&mut self, delta: f32) {
        self.radius = (self.radius - delta * 0.65).clamp(2.8, 35.0);
        self.update_orbit();
    }
    pub fn preset(&mut self, close: bool) {
        *self = Self::default();
        if close {
            self.radius = 4.1;
            self.yaw = 0.05;
            self.pitch = -0.035;
            self.target = Vec3::new(0.0, 1.96, 0.0);
        }
        self.update_orbit();
    }
    pub fn move_keys(&mut self, keys: &[bool; 256], dt: f32, jump: bool) {
        let axis = |positive: usize, negative: usize| {
            (keys[positive] as i32 - keys[negative] as i32) as f32
        };
        let z = axis(b'W' as usize, b'S' as usize);
        let x = axis(b'D' as usize, b'A' as usize);
        let y = axis(b'E' as usize, b'Q' as usize);
        if z != 0.0 || x != 0.0 || y != 0.0 {
            self.orbit = false;
        }
        if self.orbit {
            return;
        }
        let f = self.forward();
        let r = Vec3::Y.cross(f).normalize();
        let f = if self.walking {
            Vec3::new(f.x, 0.0, f.z).normalize()
        } else {
            f
        };
        let delta = f * z
            + r * x
            + if self.walking {
                Vec3::ZERO
            } else {
                Vec3::Y * y
            };
        let speed = if keys[16] { 7.5 } else { 3.1 };
        let candidate = self.position + delta.normalize_or_zero() * speed * dt;
        if self.walking {
            if walkable(candidate.x, self.position.z) {
                self.position.x = candidate.x;
            }
            if walkable(self.position.x, candidate.z) {
                self.position.z = candidate.z;
            }
            let ground = 1.94;
            if jump && self.position.y <= ground + 0.01 {
                self.velocity_y = 4.2;
            }
            self.velocity_y -= 9.8 * dt;
            self.position.y += self.velocity_y * dt;
            if self.position.y < ground {
                self.position.y = ground;
                self.velocity_y = 0.0;
            }
        } else {
            self.position = candidate;
            self.position.y = self.position.y.clamp(0.55, 30.0);
        }
    }
    pub fn toggle_walk(&mut self) {
        self.walking = !self.walking;
        self.orbit = false;
        self.velocity_y = 0.0;
        if self.walking {
            self.position = Vec3::new(0.0, 1.94, -3.7);
        }
    }
}
fn walkable(x: f32, z: f32) -> bool {
    if x.abs() > 6.8 || !(-4.1..=8.3).contains(&z) {
        return false;
    }
    if x.abs() < 0.82 && z.abs() < 0.65 {
        return false;
    }
    for side in [-1.0, 1.0] {
        if (x - side * 2.8).abs() < 0.58 && (z - 4.3).abs() < 0.58 {
            return false;
        }
        for lz in [-2.0, 2.8, 6.5] {
            if (x - side * 2.3).abs() < 0.58 && (z - lz).abs() < 0.58 {
                return false;
            }
        }
        for (tx, tz) in [(4.5, 1.3), (5.0, 6.2)] {
            if (x - side * tx).hypot(z - tz) < 0.6 {
                return false;
            }
        }
    }
    true
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn movement_is_normalized_and_camera_basis_is_orthogonal() {
        let mut c = Camera {
            orbit: false,
            ..Default::default()
        };
        let start = c.position;
        let mut keys = [false; 256];
        keys[87] = true;
        keys[68] = true;
        c.move_keys(&keys, 1.0, false);
        assert!(((c.position - start).length() - 3.1).abs() < 1e-4);
        let (r, u, f) = c.basis(1.6);
        assert!(r.dot(u).abs() < 1e-5 && r.dot(f).abs() < 1e-5 && u.dot(f).abs() < 1e-5);
    }
    #[test]
    fn walking_collides_with_props_and_lands_after_jump() {
        assert!(!walkable(2.8, 4.3));
        assert!(walkable(0., 4.3));
        let mut c = Camera::default();
        c.toggle_walk();
        let keys = [false; 256];
        c.move_keys(&keys, 0.016, true);
        assert!(c.position.y > 1.94);
        for _ in 0..180 {
            c.move_keys(&keys, 0.016, false);
        }
        assert!((c.position.y - 1.94).abs() < 1e-5);
    }
}
