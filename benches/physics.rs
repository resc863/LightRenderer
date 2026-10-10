//! Reproducible CPU-only workload; no Criterion or external physics engine.
use glam::Vec3A;
use light_renderer::physics::{BodyDesc, Config, Shape, World};
use serde_json::json;
use std::{hint::black_box, path::PathBuf, time::Instant};

fn world(case: &str, count: usize) -> World {
    let mut w = World::new(Config {
        gravity: if case == "sparse" {
            Vec3A::ZERO
        } else {
            Vec3A::new(0., -9.81, 0.)
        },
        ..Default::default()
    })
    .unwrap();
    if case != "sparse" {
        w.insert(BodyDesc::fixed(
            Shape::Box {
                half_extents: Vec3A::new(100., 0.1, 100.),
            },
            Vec3A::new(0., -0.1, 0.),
        ))
        .unwrap();
    }
    let columns = ((count as f32 / 8.).sqrt().ceil() as usize).max(1);
    for i in 0..count {
        let (shape, position) = if case == "sparse" {
            (
                Shape::Sphere { radius: 0.22 },
                Vec3A::new((i % 64) as f32 * 2., 10., (i / 64) as f32 * 2.),
            )
        } else if case == "sleeping" {
            (
                Shape::Sphere { radius: 0.22 },
                Vec3A::new((i % 32) as f32 * 0.65, 1., (i / 32) as f32 * 0.65),
            )
        } else if case == "motion" {
            (
                Shape::Sphere { radius: 0.12 },
                Vec3A::new((i % 16) as f32, 200., (i / 16) as f32),
            )
        } else {
            let layer_size = columns * columns;
            let column = i % layer_size;
            (
                Shape::Box {
                    half_extents: Vec3A::splat(0.22),
                },
                Vec3A::new(
                    (column % columns) as f32 * 0.65,
                    0.23 + (i / layer_size) as f32 * 0.45,
                    (column / columns) as f32 * 0.65,
                ),
            )
        };
        let mut d = BodyDesc::dynamic(shape, position, 1.);
        d.can_sleep = case == "sleeping";
        d.restitution = 0.;
        if case == "sparse" {
            d.velocity = Vec3A::X * 0.1;
        }
        if case == "motion" {
            d.velocity = Vec3A::NEG_Y * 60.;
            d.motion_substeps = true;
        }
        w.insert(d).unwrap();
    }
    w
}
fn percentile(samples: &[f64], p: f64) -> f64 {
    samples[((samples.len() as f64 * p).ceil() as usize)
        .saturating_sub(1)
        .min(samples.len() - 1)]
}
fn main() {
    let mut steps = 180usize;
    let mut repeats = 3usize;
    let mut report = PathBuf::from("artifacts/physics-bench.json");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--steps" => steps = args.next().expect("--steps value").parse().unwrap(),
            "--repeats" => repeats = args.next().expect("--repeats value").parse().unwrap(),
            "--report" => report = args.next().expect("--report path").into(),
            "--bench" => {}
            _ => panic!("Unknown benchmark option {arg}"),
        }
    }
    assert!(steps >= 30 && repeats > 0);
    let backend = if cfg!(feature = "physics-scalar") {
        "scalar"
    } else {
        "Vec3A SIMD"
    };
    let cases = [
        ("sparse", 256),
        ("sparse", 1024),
        ("sparse", 4096),
        ("dense", 128),
        ("dense", 512),
        ("dense", 2048),
        ("sleeping", 512),
        ("motion", 128),
    ];
    let mut results = vec![];
    for (case, count) in cases {
        let warmup = if case == "sleeping" {
            1200
        } else if case == "motion" {
            0
        } else {
            60
        };
        let mut samples = Vec::with_capacity(steps * repeats);
        let mut active = 0.;
        let mut contacts = 0.;
        let mut candidates = 0.;
        let mut substeps = 0.;
        let mut limit_hits = 0;
        for _ in 0..repeats {
            let mut w = world(case, count);
            for _ in 0..warmup {
                w.step();
            }
            let initial_substeps = w.stats().motion_substeps;
            let initial_limits = w.stats().motion_limit_hits;
            for _ in 0..steps {
                let start = Instant::now();
                w.step();
                black_box(w.stats());
                samples.push(start.elapsed().as_secs_f64() * 1000.);
                active += w.stats().active_bodies as f64;
                contacts += w.stats().contacts as f64;
                candidates += w.stats().candidate_pairs as f64;
            }
            substeps += (w.stats().motion_substeps - initial_substeps) as f64;
            limit_hits += w.stats().motion_limit_hits - initial_limits;
            assert!(w.bodies().all(|(_, b)| b.pose().position.is_finite()
                && b.pose().rotation.is_finite()
                && b.velocity().is_finite()));
            if case == "dense" || case == "sleeping" {
                assert!(
                    w.bodies()
                        .filter(|(_, b)| b.is_dynamic())
                        .all(|(_, b)| b.pose().position.y > 0.1),
                    "A stacked body penetrated the floor"
                );
            }
            if case == "sleeping" {
                assert_eq!(
                    w.stats().sleeping_bodies,
                    count,
                    "Sleep workload did not settle"
                );
            }
        }
        samples.sort_by(f64::total_cmp);
        let n = samples.len() as f64;
        let mean = samples.iter().sum::<f64>() / n;
        let p95 = percentile(&samples, 0.95);
        println!(
            "{backend:12} {case:8} {count:5} bodies: mean {mean:8.3} ms, p95 {p95:8.3} ms, active {:.0}, contacts {:.0}",
            active / n,
            contacts / n
        );
        results.push(json!({"case":case, "bodies":count, "warmup_steps_per_repeat":warmup,
            "samples": samples.len(), "mean_ms":mean, "p50_ms":percentile(&samples,0.5),
            "p95_ms":p95, "p99_ms":percentile(&samples,0.99), "max_ms":samples[samples.len()-1],
            "mean_active_bodies":active/n, "mean_contacts":contacts/n, "mean_candidate_pairs":candidates/n,
            "mean_motion_substeps":substeps/n, "motion_limit_hits":limit_hits,
            "within_120hz_cpu_budget_p95":p95 < 1000./120.}));
    }
    let result = json!({"engine":"native", "math_backend":backend,
        "architecture":std::env::consts::ARCH, "os":std::env::consts::OS,
        "debug_assertions":cfg!(debug_assertions), "steps_per_repeat":steps, "repeats":repeats,
        "fixed_hz":120, "solver_iterations":10,
        "scope":"CPU physics only; renderer, GPU, world construction and warmup excluded",
        "cases":results});
    if let Some(parent) = report.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&report, serde_json::to_string_pretty(&result).unwrap()).unwrap();
    println!("Report: {}", report.display());
}
