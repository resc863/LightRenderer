use crate::{camera::Camera, scene::Object};
use bytemuck::{Pod, Zeroable};
use glam::Vec3;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Patch {
    pub object: u32,
    pub face: u32,
    pub x: u32,
    pub y: u32,
    pub columns: u32,
    pub rows: u32,
    pub width: u32,
    pub height: u32,
}
pub struct DetailPlan {
    pub patches: Vec<Patch>,
    pub triangles: u64,
    pub visible_objects: u32,
}
pub fn plan(objects: &[Object], camera: &Camera, height: u32, error: f32) -> DetailPlan {
    let mut out = DetailPlan {
        patches: vec![],
        triangles: 0,
        visible_objects: 0,
    };
    let f = camera.forward();
    let aspect_fov = (25.0f32.to_radians()).tan();
    for (id, o) in objects.iter().enumerate() {
        let center = Vec3::from_slice(&o.center_kind);
        let size = Vec3::from_slice(&o.size);
        let radius = size.length();
        let d = center - camera.position;
        if d.dot(f) < -radius || d.length() - radius > 100. {
            continue;
        }
        // Analytic patches are generated on demand. There are no prebuilt LOD meshes.
        let projected_radius =
            radius * height as f32 / (2. * aspect_fov * (d.length() - radius * 0.5).max(0.2));
        let n = ((std::f32::consts::PI * (projected_radius / (2. * error.max(0.1))).sqrt()).ceil()
            as u32)
            .clamp(6, 96);
        out.visible_objects += 1;
        if o.center_kind[3] > 0.5 {
            for face in 0..6 {
                out.patches.push(Patch {
                    object: id as u32,
                    face,
                    x: 0,
                    y: 0,
                    columns: 1,
                    rows: 1,
                    width: 1,
                    height: 1,
                });
            }
            out.triangles += 12;
        } else {
            let cols = n * 2;
            let rows = n;
            for y in (0..rows).step_by(7) {
                for x in (0..cols).step_by(7) {
                    out.patches.push(Patch {
                        object: id as u32,
                        face: 6,
                        x,
                        y,
                        columns: cols,
                        rows,
                        width: (cols - x).min(7),
                        height: (rows - y).min(7),
                    });
                }
            }
            out.triangles += (cols * rows * 2) as u64;
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn continuous_error_budget_changes_density_and_respects_mesh_limits() {
        let objects = crate::scene::garden(0.);
        let c = Camera::default();
        let fine = plan(&objects, &c, 900, 0.3);
        let coarse = plan(&objects, &c, 900, 2.);
        assert!(fine.triangles > coarse.triangles * 2);
        for p in fine.patches {
            assert!((p.width + 1) * (p.height + 1) <= 64 && p.width * p.height * 2 <= 98);
        }
    }
    #[test]
    fn approaching_the_subject_increases_streamed_geometry() {
        let objects = crate::scene::garden(0.);
        let far = Camera::default();
        let mut near = Camera::default();
        near.preset(true);
        assert!(
            plan(&objects, &near, 900, 0.7).triangles > plan(&objects, &far, 900, 0.7).triangles
        );
    }
}
