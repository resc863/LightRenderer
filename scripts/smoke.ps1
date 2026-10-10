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
$exe = Join-Path (Get-Location) 'target\release\light-renderer.exe'
foreach ($backend in @('ultimate','dxr', 'software','ultimate-disabled')) {
    $args = @('--frames','75','--scripted-input','--debug','--width','960','--height','640',
        '--capture',"artifacts/$backend.bmp",'--report',"artifacts/$backend-report.json")
    if ($backend -eq 'software') { $args += '--software' }
    if ($backend -eq 'dxr') { $args += @('--renderer','raytracing') }
    if ($backend -eq 'ultimate') { $args += @('--renderer','ultimate','--texture-budget-mb','4') }
    if ($backend -eq 'ultimate-disabled') { $args += @('--renderer','ultimate','--no-vrs','--no-streaming') }
    $process = Start-Process -FilePath $exe -ArgumentList $args -PassThru
    if (-not $process.WaitForExit(60000)) { Stop-Process -Id $process.Id; throw "$backend smoke timed out" }
    if ($process.ExitCode -ne 0) { throw (Get-Content 'artifacts/last-error.txt' -Raw) }
    $report = Get-Content "artifacts/$backend-report.json" -Raw | ConvertFrom-Json
    if ($report.frames -ne 75 -or -not $report.resize_exercised -or $report.gpu_warnings_and_errors.Count -ne 0) { throw "$backend smoke verification failed" }
    if ($backend -eq 'ultimate') {
        $u=$report.ultimate
        $s=$u.texture_streaming
        if (-not $report.capabilities.dx12_ultimate -or $u.mesh_shader_frames -ne 70 -or $u.vrs_frames -ne 67 -or $u.max_triangles -le $u.min_triangles -or $s.hardware_feedback_requests -le 0 -or $s.uploaded_pages -le 5 -or $s.resident_bytes -gt $s.physical_pool_bytes) {throw 'Ultimate features/streaming verification failed'}
    }
    if ($backend -eq 'ultimate-disabled' -and ($report.ultimate.vrs_frames -ne 0 -or $report.ultimate.texture_streaming.hardware_feedback_requests -ne 0)) {throw 'Feature switches verification failed'}
    Write-Output "$backend PASS: $($report.backend), $($report.adapter), $($report.frames) frames"
}
Add-Type -AssemblyName System.Drawing
foreach ($name in @('ultimate','dxr','software','ultimate-disabled')) {
    $image = [System.Drawing.Bitmap]::new((Join-Path (Get-Location) "artifacts/$name.bmp"))
    try { $image.Save((Join-Path (Get-Location) "artifacts/$name.png"), [System.Drawing.Imaging.ImageFormat]::Png) } finally { $image.Dispose() }
}
