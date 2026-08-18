# bbg — native Windows installer.
# Usage: powershell -ExecutionPolicy Bypass -File .\install.ps1
$ErrorActionPreference = 'Stop'

$repo = 'forge18/brief-bright-gone'
$version = if ($env:BBG_VERSION) { $env:BBG_VERSION } else { 'latest' }
$arch = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture) {
    'X64' { 'x86_64' }
    'Arm64' { 'aarch64' }
    default { throw "bbg: unsupported Windows architecture" }
}

if ($version -eq 'latest') {
    $release = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest"
    $version = $release.tag_name -replace '^v', ''
}
$base = "https://github.com/$repo/releases/download/v$version"
$asset = "bbg-$arch-pc-windows-msvc.tar.gz"
$root = Join-Path ([System.IO.Path]::GetTempPath()) ("bbg-install-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $root | Out-Null
try {
    $archive = Join-Path $root $asset
    $checksums = Join-Path $root 'SHA256SUMS'
    Invoke-WebRequest "$base/$asset" -OutFile $archive
    Invoke-WebRequest "$base/SHA256SUMS" -OutFile $checksums

    $expected = ((Get-Content $checksums) | Where-Object { $_ -match "^([0-9a-fA-F]{64})\s+\*?$([regex]::Escape($asset))$" } | Select-Object -First 1)
    if (-not $expected -or $expected -notmatch '^([0-9a-fA-F]{64})') {
        throw "bbg: checksum missing for $asset"
    }
    $actual = (Get-FileHash $archive -Algorithm SHA256).Hash
    if ($actual -ine $Matches[1]) {
        throw 'bbg: archive checksum mismatch'
    }

    $entries = @(tar -tzf $archive)
    if (($entries -join "`n") -ne "bbg.exe`nbbg-proxy.exe") {
        throw 'bbg: archive must contain exactly bbg.exe and bbg-proxy.exe'
    }
    tar -xzf $archive -C $root

    $installDir = if ($env:BBG_INSTALL_DIR) { $env:BBG_INSTALL_DIR } else { Join-Path $HOME '.local\bin' }
    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    Copy-Item (Join-Path $root 'bbg.exe') (Join-Path $installDir 'bbg.exe') -Force
    Copy-Item (Join-Path $root 'bbg-proxy.exe') (Join-Path $installDir 'bbg-proxy.exe') -Force
    Write-Host "bbg: installed to $installDir (v$version)"
} finally {
    Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue
}
