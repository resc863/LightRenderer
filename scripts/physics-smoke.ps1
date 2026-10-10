param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
& "$PSScriptRoot/bootstrap-sdk.ps1"
if (-not $SkipBuild) {
    cargo test --locked
    if ($LASTEXITCODE -ne 0) { throw 'Tests failed' }
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
}
$exe = Join-Path (Get-Location) 'target/release/light-renderer.exe'
foreach ($backend in @('ultimate', 'raytracing', 'software', 'disabled')) {
    $args = @('--frames','180','--scripted-input','--debug','--width','960','--height','640',
        '--capture',"artifacts/physics-$backend.bmp",'--report',"artifacts/physics-$backend-report.json")
    if ($backend -eq 'software') { $args += '--software' }
    if ($backend -eq 'disabled') { $args += '--no-physics' }
    if ($backend -in @('ultimate', 'raytracing')) { $args += @('--renderer',$backend) }
    $process = Start-Process -FilePath $exe -ArgumentList $args -PassThru -WindowStyle Hidden
    if (-not $process.WaitForExit(60000)) { Stop-Process -Id $process.Id; throw "$backend smoke timed out" }
    if ($process.ExitCode -ne 0) { throw (Get-Content 'artifacts/last-error.txt' -Raw) }
    $r = Get-Content "artifacts/physics-$backend-report.json" -Raw | ConvertFrom-Json
    if ($r.frames -ne 180 -or -not $r.resize_exercised -or $r.gpu_warnings_and_errors.Count -ne 0) { throw "$backend renderer failure" }
    if ($backend -eq 'disabled') {
        if ($null -ne $r.physics -or $r.primitives -ne 206) { throw 'Physics disable failed' }
    } else {
        if ($r.physics.steps -ne 350 -or $r.physics.dynamic_bodies -ne 24 -or $r.primitives -ne 230 -or
            $r.physics.total_contacts -le 0 -or -not $r.physics_pause_exercised -or -not $r.physics_kick_exercised) {
            throw "$backend physics integration failure"
        }
        if ($r.physics.dropped_seconds -gt 0.001 -or $r.physics.motion_limit_hits -ne 0) { throw 'Unexpected time/motion limit' }
    }
    Write-Output "$backend PASS: $($r.frames) frames, physics steps $($r.physics.steps), contacts $($r.physics.total_contacts)"
}
Add-Type -AssemblyName System.Drawing
foreach ($backend in @('ultimate', 'raytracing', 'software', 'disabled')) {
    $image = [System.Drawing.Bitmap]::new((Join-Path (Get-Location) "artifacts/physics-$backend.bmp"))
    try { $image.Save((Join-Path (Get-Location) "artifacts/physics-$backend.png"), [System.Drawing.Imaging.ImageFormat]::Png) }
    finally { $image.Dispose() }
}
