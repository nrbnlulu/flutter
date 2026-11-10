# Install: irm https://raw.githubusercontent.com/nrbnlulu/flutter/rust/flutter-rs-shell/engine/src/flutter/shell/platform/rust/install.ps1 | iex
# Overrides: RLUTTER_VERSION, RLUTTER_REPO, RLUTTER_INSTALL_DIR.
# Uses GitHub CLI authentication when available, with HTTPS fallback.
[CmdletBinding()]
param([switch]$NoPath)

$ErrorActionPreference = 'Stop'
if ([Environment]::OSVersion.Platform -ne 'Win32NT') { throw 'rlutter-install: Windows required.' }
$architecture = $env:PROCESSOR_ARCHITEW6432
if (-not $architecture) { $architecture = $env:PROCESSOR_ARCHITECTURE }
if ($architecture -ne 'AMD64') { throw "rlutter-install: unsupported architecture $architecture (available: Windows x64)." }
$repo = if ($env:RLUTTER_REPO) { $env:RLUTTER_REPO } else { 'nrbnlulu/flutter' }
$version = if ($env:RLUTTER_VERSION) { $env:RLUTTER_VERSION } else { 'rlutter' }
$installDir = if ($env:RLUTTER_INSTALL_DIR) { $env:RLUTTER_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'rlutter\bin' }
$installDir = [IO.Path]::GetFullPath($installDir)
$asset = 'rlutter-windows-x64.zip'
$tempDir = Join-Path ([IO.Path]::GetTempPath()) ('rlutter-install-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tempDir | Out-Null
try {
    Write-Host "rlutter-install: downloading $asset ($version) from $repo"
    $downloaded = $false
    if (Get-Command gh -ErrorAction SilentlyContinue) {
        & gh release download $version --repo $repo --pattern $asset --pattern "$asset.sha256" --dir $tempDir --clobber
        $downloaded = $LASTEXITCODE -eq 0
        if (-not $downloaded) { Write-Host 'rlutter-install: gh download failed; trying HTTPS.' }
    }
    if (-not $downloaded) {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        $base = "https://github.com/$repo/releases/download/$version"
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile (Join-Path $tempDir $asset)
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset.sha256" -OutFile (Join-Path $tempDir "$asset.sha256")
    }
    $expected = ((Get-Content -LiteralPath (Join-Path $tempDir "$asset.sha256") -Raw).Trim() -split '\s+')[0]
    $actual = (Get-FileHash -LiteralPath (Join-Path $tempDir $asset) -Algorithm SHA256).Hash
    if ($expected -notmatch '^[a-fA-F0-9]{64}$' -or $expected -ne $actual) { throw "rlutter-install: checksum mismatch (expected $expected, got $actual)." }
    Expand-Archive -LiteralPath (Join-Path $tempDir $asset) -DestinationPath (Join-Path $tempDir 'extracted')
    $binary = Join-Path $tempDir 'extracted\rlutter.exe'
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw 'rlutter-install: archive has no rlutter.exe.' }
    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    Copy-Item -LiteralPath $binary -Destination (Join-Path $installDir 'rlutter.exe') -Force
    if (-not $NoPath) {
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        if ($installDir -notin ($userPath -split ';')) {
            $newPath = (("$userPath").TrimEnd(';') + ';' + $installDir).TrimStart(';')
            [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        }
        if ($installDir -notin ($env:Path -split ';')) { $env:Path += ";$installDir" }
    }
    Write-Host "rlutter-install: installed $(Join-Path $installDir 'rlutter.exe')"
    Write-Host 'rlutter-install: the first command downloads its Flutter SDK and engine; no Flutter or FVM install is needed.'
    Write-Host 'rlutter-install: building apps requires Rust/Cargo and Visual Studio C++ build tools. Open a new terminal to use the updated PATH.'
} finally {
    Remove-Item -LiteralPath $tempDir -Recurse -Force
}
