# Offline integration checks; run with Windows PowerShell 5.1 or PowerShell 7.
$ErrorActionPreference = 'Stop'
$installer = Join-Path $PSScriptRoot '..\install.ps1'
$root = Join-Path ([IO.Path]::GetTempPath()) ('rlutter-installer-test-' + [guid]::NewGuid())
$oldInstall = $env:RLUTTER_INSTALL_DIR
$oldRepo = $env:RLUTTER_REPO
$oldVersion = $env:RLUTTER_VERSION
$oldExit = $global:LASTEXITCODE
New-Item -ItemType Directory -Path $root | Out-Null
try {
    $fixture = Join-Path $root 'fixture'
    New-Item -ItemType Directory -Path $fixture | Out-Null
    Set-Content (Join-Path $fixture 'rlutter.exe') 'fixture binary'
    $archive = Join-Path $fixture 'rlutter-windows-x64.zip'
    Compress-Archive -LiteralPath (Join-Path $fixture 'rlutter.exe') -DestinationPath $archive
    $hash = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  rlutter-windows-x64.zip" | Set-Content "$archive.sha256" -Encoding ascii
    $env:RLUTTER_INSTALL_DIR = Join-Path $root 'install with spaces'
    $env:RLUTTER_REPO = 'test/repo'
    $env:RLUTTER_VERSION = 'test-tag'
    $testState = @{ useFallback = $false; webCalls = 0; downloads = @() }
    function gh {
        $testState.downloads += ,$args
        if ($testState.useFallback) { $global:LASTEXITCODE = 1; return }
        $destination = $args[[Array]::IndexOf($args, '--dir') + 1]
        Copy-Item -LiteralPath $archive,"$archive.sha256" -Destination $destination
        $global:LASTEXITCODE = 0
    }
    function Invoke-WebRequest {
        param([switch]$UseBasicParsing, $Uri, $OutFile)
        if ($Uri -notlike 'https://github.com/test/repo/releases/download/test-tag/*') { throw "Unexpected URL: $Uri" }
        $testState.webCalls++
        Copy-Item -LiteralPath (Join-Path $fixture ($Uri.Split('/')[-1])) -Destination $OutFile
    }
    & $installer -NoPath
    $installed = Join-Path $env:RLUTTER_INSTALL_DIR 'rlutter.exe'
    if ((Get-Content $installed -Raw).Trim() -ne 'fixture binary') { throw 'gh installation failed' }
    if ($testState.webCalls -ne 0 -or $testState.downloads[0][2] -ne 'test-tag' -or $testState.downloads[0][4] -ne 'test/repo') { throw 'gh arguments incorrect' }
    Set-Content $installed 'old version'
    $testState.useFallback = $true
    & $installer -NoPath
    if ($testState.webCalls -ne 2 -or (Get-Content $installed -Raw).Trim() -ne 'fixture binary') { throw 'HTTPS fallback or upgrade failed' }
    Set-Content "$archive.sha256" ('0' * 64)
    $rejected = $false
    try { & $installer -NoPath } catch {
        if ($_.Exception.Message -notlike '*checksum mismatch*') { throw }
        $rejected = $true
    }
    if (-not $rejected -or (Get-Content $installed -Raw).Trim() -ne 'fixture binary') { throw 'Checksum rejection did not preserve installed binary' }
    foreach ($download in $testState.downloads) {
        $directory = $download[[Array]::IndexOf($download, '--dir') + 1]
        if (Test-Path -LiteralPath $directory) { throw "Leaked download directory: $directory" }
    }
    Write-Host 'PASS: gh download, overrides, HTTPS fallback, upgrade, checksum rejection, cleanup.'
} finally {
    $env:RLUTTER_INSTALL_DIR = $oldInstall
    $env:RLUTTER_REPO = $oldRepo
    $env:RLUTTER_VERSION = $oldVersion
    $global:LASTEXITCODE = $oldExit
    Remove-Item -LiteralPath $root -Recurse -Force
}
