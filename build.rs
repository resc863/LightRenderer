use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rustc-link-arg-bin=light-renderer=/EXPORT:D3D12SDKVersion");
    println!("cargo:rustc-link-arg-bin=light-renderer=/EXPORT:D3D12SDKPath");
    println!("cargo:rerun-if-changed=shaders/garden.hlsl");
    println!("cargo:rerun-if-changed=shaders/ultimate.hlsl");
    println!("cargo:rerun-if-env-changed=DXC_PATH");
    let dxc = env::var_os("DXC_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            let p = PathBuf::from(".sdk/dxc/build/native/bin/x64/dxc.exe");
            p.exists().then_some(p)
        })
        .or_else(|| {
            let root = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
            let mut versions: Vec<_> = std::fs::read_dir(root)
                .ok()?
                .flatten()
                .map(|v| v.path().join("x64/dxc.exe"))
                .filter(|p| p.exists())
                .collect();
            versions.sort();
            versions.pop()
        })
        .unwrap_or_else(|| PathBuf::from("dxc.exe"));
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let profile = out.ancestors().nth(3).unwrap();
    let dlls = profile.join("D3D12");
    std::fs::create_dir_all(&dlls).unwrap();
    for name in ["D3D12Core.dll", "d3d12SDKLayers.dll"] {
        let source = PathBuf::from(".sdk/agility/build/native/bin/x64").join(name);
        println!("cargo:rerun-if-changed={}", source.display());
        std::fs::copy(&source, dlls.join(name))
            .expect("Run scripts/bootstrap-sdk.ps1 before building");
    }
    for (name, entry, hw) in [
        ("trace_dxr", "Trace", true),
        ("trace_soft", "Trace", false),
        ("compose", "Compose", false),
    ] {
        let mut cmd = Command::new(&dxc);
        cmd.args([
            "-T",
            if hw { "cs_6_5" } else { "cs_6_0" },
            "-E",
            entry,
            "-O3",
            "-Ges",
            "-WX",
            "-Fo",
        ])
        .arg(out.join(format!("{name}.dxil")))
        .arg("shaders/garden.hlsl");
        if hw {
            cmd.args(["-D", "HARDWARE_DXR=1"]);
        }
        let result = cmd
            .output()
            .expect("DXC is required: install Windows SDK or set DXC_PATH to dxc.exe");
        assert!(
            result.status.success(),
            "Shader {name}: {}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    for model in ["6_5", "6_9"] {
        for (name, entry, stage) in [("mesh", "Mesh", "ms"), ("shade", "Shade", "ps")] {
            let output = Command::new(&dxc)
                .args([
                    "-T",
                    &format!("{stage}_{model}"),
                    "-E",
                    entry,
                    "-O3",
                    "-Ges",
                    "-WX",
                    "-Fo",
                ])
                .arg(out.join(format!("{name}_{model}.dxil")))
                .arg("shaders/ultimate.hlsl")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    let output = Command::new(&dxc)
        .args(["-T", "cs_6_0", "-E", "Background", "-O3", "-Fo"])
        .arg(out.join("background.dxil"))
        .arg("shaders/garden.hlsl")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
