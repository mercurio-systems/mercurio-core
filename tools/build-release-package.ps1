param(
    [ValidateSet("windows-x64")]
    [string]$Target = "windows-x64",

    [ValidateSet("release", "debug")]
    [string]$Configuration = "release",

    [string]$OutputRoot,

    [switch]$SkipRustBuild,

    [switch]$SkipPythonWheel,

    [switch]$Clean
)

$ErrorActionPreference = "Stop"

function Write-Step {
    param([string]$Message)
    Write-Host "==> $Message"
}

function Read-PythonPackageVersion {
    param([string]$PyprojectPath)

    $versionLine = Get-Content $PyprojectPath | Where-Object { $_ -match '^\s*version\s*=' } | Select-Object -First 1
    if (-not $versionLine) {
        throw "Could not find Python package version in $PyprojectPath"
    }
    return ($versionLine -replace '^\s*version\s*=\s*"', '') -replace '"\s*$', ''
}

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
if ([string]::IsNullOrWhiteSpace($OutputRoot)) {
    $OutputRoot = Join-Path $repoRoot "dist"
}

$packageRoot = Join-Path $OutputRoot "mercurio-$Target"
$packageBin = Join-Path $packageRoot "bin"
$packageWheels = Join-Path $packageRoot "wheels"
$sourceExe = Join-Path $repoRoot "target\$Configuration\mercurio.exe"
$targetExe = Join-Path $packageBin "mercurio.exe"
$pythonRoot = Join-Path $repoRoot "python"
$pythonDist = Join-Path $pythonRoot "dist"
$pythonVersion = Read-PythonPackageVersion (Join-Path $pythonRoot "pyproject.toml")
$wheelName = "mercurio-$pythonVersion-py3-none-any.whl"
$wheelPath = Join-Path $pythonDist $wheelName

if ($Clean -and (Test-Path $packageRoot)) {
    Write-Step "Removing existing package directory $packageRoot"
    Remove-Item -LiteralPath $packageRoot -Recurse -Force
}

if (-not $SkipRustBuild) {
    Write-Step "Building Mercurio $Configuration binary"
    if ($Configuration -eq "release") {
        cargo build --bin mercurio --release
    } else {
        cargo build --bin mercurio
    }
}

if (-not (Test-Path $sourceExe)) {
    throw "Mercurio executable not found: $sourceExe"
}

Write-Step "Creating package directories"
New-Item -ItemType Directory -Force -Path $packageBin | Out-Null
New-Item -ItemType Directory -Force -Path $packageWheels | Out-Null

Write-Step "Copying Mercurio executable"
Copy-Item -LiteralPath $sourceExe -Destination $targetExe -Force

if (-not $SkipPythonWheel) {
    Write-Step "Building Mercurio Python wheel"
    foreach ($path in @(
        (Join-Path $pythonRoot "build"),
        (Join-Path $pythonRoot "dist"),
        (Join-Path $pythonRoot "mercurio.egg-info")
    )) {
        if (Test-Path $path) {
            Remove-Item -LiteralPath $path -Recurse -Force
        }
    }

    Push-Location $pythonRoot
    try {
        py -m build --wheel
    } catch {
        Write-Step "py -m build unavailable; falling back to setuptools bdist_wheel"
        py setup.py bdist_wheel
    } finally {
        Pop-Location
    }

    if (-not (Test-Path $wheelPath)) {
        throw "Expected Python wheel was not created: $wheelPath"
    }

    Write-Step "Copying Python wheel"
    Copy-Item -LiteralPath $wheelPath -Destination (Join-Path $packageWheels $wheelName) -Force
}

Write-Step "Writing package manifest"
$manifest = [ordered]@{
    target = $Target
    configuration = $Configuration
    packageRoot = $packageRoot
    executable = "bin/mercurio.exe"
    pythonWheel = if ($SkipPythonWheel) { $null } else { "wheels/$wheelName" }
}
$manifest | ConvertTo-Json -Depth 4 | Set-Content -Path (Join-Path $packageRoot "manifest.json") -Encoding UTF8

Write-Host ""
Write-Host "Release package written to $packageRoot"
