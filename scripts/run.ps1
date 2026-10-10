param([switch]$DebugBuild, [switch]$Software, [switch]$Fluid)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
& "$PSScriptRoot/bootstrap-sdk.ps1"
$configuration = if ($DebugBuild) { 'debug' } else { 'release' }
$buildArgs = @('build', '--locked')
if (-not $DebugBuild) { $buildArgs += '--release' }
& cargo @buildArgs
if ($LASTEXITCODE -ne 0) { throw 'Cargo build failed' }
$exe = Join-Path (Get-Location) "target\$configuration\light-renderer.exe"
$runArgs = @()
if ($Software) { $runArgs += '--software' }
if ($Fluid) { $runArgs += '--fluid' }
& $exe @runArgs
