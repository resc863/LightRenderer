param([switch]$Force)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$sdk = Join-Path $root '.sdk'
New-Item -ItemType Directory -Path $sdk -Force | Out-Null
# Stable shipping SDK/compiler, pinned and verified before extraction. No machine-wide install.
$packages = @(
    @{ Name='agility'; Id='microsoft.direct3d.d3d12'; Version='1.619.6'; Hash='08F0489281401AA430FC37322D6C3FC98A8025175AACD714C10D562F4963F1E9'; Probe='build/native/bin/x64/D3D12Core.dll' },
    @{ Name='dxc'; Id='microsoft.direct3d.dxc'; Version='1.9.2609.5'; Hash='73CFA082245918C2CC1EC6740250EB7377E83A757B3B3A3E16E0D71535BFBFC3'; Probe='build/native/bin/x64/dxc.exe' }
)
foreach ($p in $packages) {
    $destination = Join-Path $sdk $p.Name
    if (-not $Force -and (Test-Path (Join-Path $destination $p.Probe))) { continue }
    $archive = Join-Path $sdk "$($p.Name).zip"
    if (-not (Test-Path $archive) -or (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $p.Hash) {
        $url = "https://api.nuget.org/v3-flatcontainer/$($p.Id)/$($p.Version)/$($p.Id).$($p.Version).nupkg"
        Invoke-WebRequest -Uri $url -OutFile $archive
    }
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $p.Hash) { throw "SDK checksum mismatch: $($p.Id)" }
    Expand-Archive -LiteralPath $archive -DestinationPath $destination -Force
    Write-Output "$($p.Id) $($p.Version) ready"
}
