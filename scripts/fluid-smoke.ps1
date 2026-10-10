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
foreach ($backend in @('ultimate','raytracing','software')) {
    $args = @('--fluid','--frames','180','--scripted-input','--debug','--width','960','--height','640',
        '--capture',"artifacts/fluid-$backend.bmp",'--report',"artifacts/fluid-$backend-report.json")
    if ($backend -eq 'software') { $args += '--software' }
    else { $args += @('--renderer',$backend) }
    $process = Start-Process -FilePath $exe -ArgumentList $args -PassThru -WindowStyle Hidden
    if (-not $process.WaitForExit(60000)) { Stop-Process -Id $process.Id; throw "$backend fluid smoke timed out" }
    if ($process.ExitCode -ne 0) { throw (Get-Content 'artifacts/last-error.txt' -Raw) }
    $r = Get-Content "artifacts/fluid-$backend-report.json" -Raw | ConvertFrom-Json
    $f = $r.physics.fluid
    if ($r.frames -ne 180 -or $r.primitives -ne 446 -or $r.gpu_warnings_and_errors.Count -ne 0 -or
        -not $r.resize_exercised -or -not $r.physics_pause_exercised -or -not $r.physics_kick_exercised -or
        $f.steps -ne 350 -or $f.particles -ne 216 -or $f.boundary_contacts -le 0 -or
        [Math]::Abs($f.total_mass_kg - 110.592) -gt 0.001 -or $f.speed_limit_hits -ne 0 -or $f.motion_limit_hits -ne 0) {
        throw "$backend fluid integration verification failed"
    }
    foreach ($p in $f.positions) {
        if ($p.Count -ne 3 -or $p[0] -lt -4.16001 -or $p[0] -gt -2.63999 -or $p[1] -lt 0.49999 -or $p[1] -gt 1.96001 -or
            $p[2] -lt -3.46001 -or $p[2] -gt -1.93999) { throw "$backend fluid particle escaped its container" }
    }
    Write-Output "$backend PASS: $($f.particles) particles, $($f.steps) ticks, mass $($f.total_mass_kg) kg"
}
Add-Type -AssemblyName System.Drawing
foreach ($backend in @('ultimate','raytracing','software')) {
    $image = [System.Drawing.Bitmap]::new((Join-Path (Get-Location) "artifacts/fluid-$backend.bmp"))
    try { $image.Save((Join-Path (Get-Location) "artifacts/fluid-$backend.png"), [System.Drawing.Imaging.ImageFormat]::Png) }
    finally { $image.Dispose() }
}
