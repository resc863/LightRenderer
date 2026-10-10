//! Fluid-specific release benchmark with physical state validation.
use glam::Vec3A;
use light_renderer::physics::fluid::{Fluid, FluidConfig, ParticleDesc};
use serde_json::json;
use std::{hint::black_box, path::PathBuf, time::Instant};

fn make_fluid(case: &str, count: usize) -> Fluid {
    let n = (count as f32).cbrt().ceil() as usize;
    let mut f = Fluid::new(FluidConfig {
        bounds_min: Vec3A::new(-1., 0., -1.),
        bounds_max: Vec3A::new(12., 6., 12.),
        gravity: if case == "sparse" {
            Vec3A::ZERO
        } else {
            Vec3A::new(0., -9.81, 0.)
        },
        viscosity: if case == "viscous" { 20. } else { 2. },
        ..Default::default()
    })
    .unwrap();
    for i in 0..count {
        let p = if case == "sparse" {
            Vec3A::new((i % 32) as f32 * 0.35, 2., (i / 32) as f32 * 0.35)
        } else {
            Vec3A::new(
                (i % n) as f32 * 0.1,
                0.5 + ((i / n) % n) as f32 * 0.1,
                (i / (n * n)) as f32 * 0.1,
            )
        };
        let mut d = ParticleDesc::new(p);
        if case == "viscous" {
            d.velocity = Vec3A::X * if i % 2 == 0 { 0.2 } else { -0.2 };
        }
        f.insert(d).unwrap();
    }
    f
}
fn percentile(samples: &[f64], p: f64) -> f64 {
    samples[((samples.len() as f64 * p).ceil() as usize)
        .saturating_sub(1)
        .min(samples.len() - 1)]
}
fn main() {
    let mut steps = 120usize;
    let mut repeats = 3usize;
    let mut report = PathBuf::from("artifacts/fluid-bench.json");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--steps" => steps = args.next().unwrap().parse().unwrap(),
            "--repeats" => repeats = args.next().unwrap().parse().unwrap(),
            "--report" => report = args.next().unwrap().into(),
            "--bench" => {}
            _ => panic!("Unknown fluid benchmark option {arg}"),
        }
    }
    assert!(steps >= 30 && repeats >= 1);
    let mut cases = vec![];
    let backend = if cfg!(feature = "physics-scalar") {
        "scalar"
    } else {
        "Vec3A SIMD"
    };
    for (case, count) in [
        ("sparse", 512),
        ("dam-break", 216),
        ("dam-break", 512),
        ("dam-break", 1024),
        ("viscous", 512),
    ] {
        let mut times = vec![];
        let mut pairs = 0.;
        let mut candidates = 0.;
        let mut compression = 0f32;
        let mut substeps = 0.;
        let mut speed_limit_hits = 0u64;
        let mut motion_limit_hits = 0u64;
        let mut correction_limit_hits = 0u64;
        for _ in 0..repeats {
            let mut f = make_fluid(case, count);
            let mass = f.total_mass();
            for _ in 0..30 {
                f.step(1. / 120.).unwrap();
            }
            let initial_substeps = f.stats().substeps;
            for _ in 0..steps {
                let start = Instant::now();
                f.step(1. / 120.).unwrap();
                black_box(f.stats());
                times.push(start.elapsed().as_secs_f64() * 1000.);
                pairs += f.stats().neighbor_pairs as f64;
                candidates += f.stats().grid_candidates as f64;
                compression = compression.max(f.stats().max_compression_after);
            }
            substeps += (f.stats().substeps - initial_substeps) as f64;
            speed_limit_hits += f.stats().speed_limit_hits;
            motion_limit_hits += f.stats().motion_limit_hits;
            correction_limit_hits += f.stats().correction_limit_hits;
            assert_eq!(f.particles().len(), count);
            assert_eq!(mass, f.total_mass());
            assert!(f.particles().iter().all(|p| {
                p.position().is_finite()
                    && p.velocity().is_finite()
                    && p.density().is_finite()
                    && p.position()
                        .cmpge(f.config().bounds_min + Vec3A::splat(f.config().particle_radius))
                        .all()
                    && p.position()
                        .cmple(f.config().bounds_max - Vec3A::splat(f.config().particle_radius))
                        .all()
            }));
            assert_eq!(
                f.stats().speed_limit_hits,
                0,
                "Pathological velocity during benchmark"
            );
        }
        times.sort_by(f64::total_cmp);
        let samples = times.len() as f64;
        let mean = times.iter().sum::<f64>() / samples;
        let p95 = percentile(&times, 0.95);
        println!(
            "{backend:12} {case:10} {count:5} particles: mean {mean:.3} ms, p95 {p95:.3} ms, pairs {:.0}",
            pairs / samples
        );
        cases.push(json!({"case":case,"particles":count,"samples":times.len(),"warmup_steps":30,
            "mean_ms":mean,"p50_ms":percentile(&times,0.5),"p95_ms":p95,"p99_ms":percentile(&times,0.99),
            "max_ms":times[times.len()-1],"mean_neighbor_pairs":pairs/samples,"mean_grid_candidates":candidates/samples,
            "mean_substeps":substeps/samples,"max_compression_after":compression,"speed_limit_hits":speed_limit_hits,
            "motion_limit_hits":motion_limit_hits,"correction_limit_hits":correction_limit_hits,
            "within_120hz_cpu_budget_p95":p95<1000./120.}));
    }
    let result = json!({"method":"PBF","math_backend":backend,"fixed_hz":120,"solver_iterations":4,
        "steps_per_repeat":steps,"repeats":repeats,"architecture":std::env::consts::ARCH,
        "scope":"CPU fluid only; construction, warmup, rigid collisions and GPU rendering excluded",
        "cases":cases});
    if let Some(parent) = report.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&report, serde_json::to_string_pretty(&result).unwrap()).unwrap();
    println!("Report: {}", report.display());
}
