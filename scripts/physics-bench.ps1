param(
    [int]$Steps = 180,
    [int]$Repeats = 3,
    [switch]$CompareScalar,
    # Explicit opt-in performance gate, in CPU milliseconds per 120 Hz tick.
    [double]$MaxP95Ms = 0
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
if ($Steps -lt 30 -or $Repeats -lt 1 -or $MaxP95Ms -lt 0) { throw 'Invalid benchmark parameters' }
& "$PSScriptRoot/bootstrap-sdk.ps1"
$modes = if ($CompareScalar) { @('simd', 'scalar') } else { @('simd') }
$results = @{}
foreach ($mode in $modes) {
    $cargoArgs = @('bench', '--locked', '--bench', 'physics')
    if ($mode -eq 'scalar') { $cargoArgs += @('--features', 'physics-scalar') }
    $cargoArgs += @('--', '--steps', $Steps, '--repeats', $Repeats, '--report', "artifacts/physics-bench-$mode.json")
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "Physics benchmark failed: $mode" }
    $result = Get-Content "artifacts/physics-bench-$mode.json" -Raw | ConvertFrom-Json
    $results[$mode] = $result
    if ($MaxP95Ms -gt 0) {
        foreach ($case in $result.cases) {
            if ($case.p95_ms -gt $MaxP95Ms) { throw "$mode $($case.case) $($case.bodies): p95 exceeds $MaxP95Ms ms" }
        }
    }
}
if ($CompareScalar) {
    $comparison = for ($i = 0; $i -lt $results.simd.cases.Count; $i++) {
        $simd = $results.simd.cases[$i]
        $scalar = $results.scalar.cases[$i]
        [pscustomobject]@{
            case = $simd.case; bodies = $simd.bodies
            simd_mean_ms = $simd.mean_ms; scalar_mean_ms = $scalar.mean_ms
            scalar_over_simd_mean = $scalar.mean_ms / $simd.mean_ms
            simd_p95_ms = $simd.p95_ms; scalar_p95_ms = $scalar.p95_ms
        }
    }
    [pscustomobject]@{
        cpu = $env:PROCESSOR_IDENTIFIER
        rustc = (& rustc --version)
        steps_per_repeat = $Steps; repeats = $Repeats
        note = 'Separate sequential builds; timing noise and backend-dependent floating-point trajectories affect ratios.'
        cases = @($comparison)
    } | ConvertTo-Json -Depth 8 | Set-Content 'artifacts/physics-bench-comparison.json' -Encoding utf8
    $comparison | Format-Table -AutoSize
}
