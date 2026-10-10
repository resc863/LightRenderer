param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
& "$PSScriptRoot/bootstrap-sdk.ps1"
if (-not $SkipBuild) {
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Cargo build failed' }
}
$layout = Join-Path $root 'dist\layout'
New-Item -ItemType Directory -Path $layout -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $root 'target\release\light-renderer.exe') -Destination $layout
Copy-Item -LiteralPath (Join-Path $root 'target\release\D3D12') -Destination $layout -Recurse -Force
Copy-Item -LiteralPath (Join-Path $root 'Package.appxmanifest') -Destination $layout
$assetLayout = Join-Path $layout 'Assets'
New-Item -ItemType Directory -Path $assetLayout -Force | Out-Null
Get-ChildItem -LiteralPath (Join-Path $root 'Assets') -File | Copy-Item -Destination $assetLayout -Force
Copy-Item -LiteralPath (Join-Path $root 'LICENSE') -Destination $layout -Force
$licenses = Join-Path $layout 'Licenses'
New-Item -ItemType Directory -Path $licenses -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $root '.sdk/agility/LICENSE.txt') -Destination (Join-Path $licenses 'Agility-SDK.txt') -Force
winapp package $layout --output (Join-Path $root 'dist\LightRenderer.msix') --no-sign --skip-pri
if ($LASTEXITCODE -ne 0) { throw 'winapp package failed' }
Write-Output "Unsigned MSIX: $root\dist\LightRenderer.msix"
