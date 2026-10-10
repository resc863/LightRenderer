param([int]$Steps = 120, [int]$Repeats = 3, [switch]$CompareScalar, [double]$MaxP95Ms = 0)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
if ($Steps -lt 30 -or $Repeats -lt 1 -or $MaxP95Ms -lt 0) { throw 'Invalid benchmark parameters' }
& "$PSScriptRoot/bootstrap-sdk.ps1"
$modes = if ($CompareScalar) { @('simd','scalar') } else { @('simd') }
$results = @{}
foreach ($mode in $modes) {
    $cargoArgs = @('bench','--locked','--bench','fluid')
    if ($mode -eq 'scalar') { $cargoArgs += @('--features','physics-scalar') }
    $cargoArgs += @('--','--steps',$Steps,'--repeats',$Repeats,'--report',"artifacts/fluid-bench-$mode.json")
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "Fluid benchmark failed: $mode" }
    $r = Get-Content "artifacts/fluid-bench-$mode.json" -Raw | ConvertFrom-Json
    $results[$mode] = $r
    if ($MaxP95Ms -gt 0) {
        foreach ($case in $r.cases) {
            if ($case.p95_ms -gt $MaxP95Ms) { throw "$mode $($case.case) $($case.particles): exceeds $MaxP95Ms ms" }
        }
    }
}
if ($CompareScalar) {
    $comparison = for ($i=0; $i -lt $results.simd.cases.Count; $i++) {
        $s = $results.simd.cases[$i]; $c = $results.scalar.cases[$i]
        [pscustomobject]@{
            case=$s.case; particles=$s.particles; simd_mean_ms=$s.mean_ms; simd_p95_ms=$s.p95_ms
            scalar_mean_ms=$c.mean_ms; scalar_p95_ms=$c.p95_ms; scalar_over_simd_mean=$c.mean_ms/$s.mean_ms
        }
    }
    [pscustomobject]@{
        cpu=$env:PROCESSOR_IDENTIFIER; rustc=(& rustc --version)
        steps_per_repeat=$Steps; repeats=$Repeats
        note='CPU-only sequential builds; floating-point trajectories and timing noise may differ.'
        cases=@($comparison)
    } | ConvertTo-Json -Depth 8 | Set-Content 'artifacts/fluid-bench-comparison.json' -Encoding utf8
    $comparison | Format-Table -AutoSize
}
