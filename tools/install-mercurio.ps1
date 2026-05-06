param(
    [ValidateSet("release", "debug")]
    [string]$Configuration = "release",

    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA "Mercurio"),

    [switch]$InstallPython,

    [ValidateSet("editable", "wheel")]
    [string]$PythonMode = "editable",

    [string]$PythonWheel,

    [switch]$Build,

    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

function Write-Step {
    param([string]$Message)
    Write-Host "==> $Message"
}

function Invoke-InstallCommand {
    param(
        [string]$Description,
        [scriptblock]$Command
    )

    Write-Step $Description
    if ($DryRun) {
        return
    }
    & $Command
}

function Add-UserPathEntry {
    param([string]$PathEntry)

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @()
    if (-not [string]::IsNullOrWhiteSpace($userPath)) {
        $entries = $userPath -split ";" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    }

    $alreadyPresent = $entries | Where-Object {
        $_.TrimEnd("\") -ieq $PathEntry.TrimEnd("\")
    }

    if ($alreadyPresent) {
        Write-Step "User PATH already contains $PathEntry"
        return
    }

    $newPath = (@($entries) + $PathEntry) -join ";"
    Invoke-InstallCommand "Adding $PathEntry to user PATH" {
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    }
}

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$binDir = Join-Path $InstallRoot "bin"
$sourceExe = Join-Path $repoRoot "target\$Configuration\mercurio.exe"
$targetExe = Join-Path $binDir "mercurio.exe"
$pythonProject = Join-Path $repoRoot "python"
$wheelInstallDir = Join-Path $InstallRoot "wheels"

if ($Build) {
    Invoke-InstallCommand "Building Mercurio $Configuration binary" {
        if ($Configuration -eq "release") {
            cargo build --bin mercurio --release
        } else {
            cargo build --bin mercurio
        }
    }
}

if (-not (Test-Path $sourceExe)) {
    throw "Mercurio executable not found: $sourceExe. Run cargo build --bin mercurio, cargo build --bin mercurio --release, or pass -Build."
}

Invoke-InstallCommand "Creating install directory $binDir" {
    New-Item -ItemType Directory -Force -Path $binDir | Out-Null
}

Invoke-InstallCommand "Installing $sourceExe to $targetExe" {
    Copy-Item -LiteralPath $sourceExe -Destination $targetExe -Force
}

Add-UserPathEntry -PathEntry $binDir

Invoke-InstallCommand "Verifying Mercurio executable" {
    & $targetExe --help | Out-Null
}

if ($InstallPython) {
    if ($PythonMode -eq "wheel") {
        if ([string]::IsNullOrWhiteSpace($PythonWheel)) {
            $packageWheelDir = Join-Path $repoRoot "dist\mercurio-windows-x64\wheels"
            $wheels = @()
            if (Test-Path $packageWheelDir) {
                $wheels = Get-ChildItem -Path $packageWheelDir -Filter "mercurio-*.whl" | Sort-Object LastWriteTime -Descending
            }
            if ($wheels.Count -eq 0) {
                throw "No Mercurio wheel found in $packageWheelDir. Pass -PythonWheel or run tools\build-release-package.ps1."
            }
            $PythonWheel = $wheels[0].FullName
        }

        if (-not (Test-Path $PythonWheel)) {
            throw "Python wheel not found: $PythonWheel"
        }

        $targetWheel = Join-Path $wheelInstallDir (Split-Path -Leaf $PythonWheel)
        Invoke-InstallCommand "Creating wheel directory $wheelInstallDir" {
            New-Item -ItemType Directory -Force -Path $wheelInstallDir | Out-Null
        }

        Invoke-InstallCommand "Copying Python wheel to $targetWheel" {
            Copy-Item -LiteralPath $PythonWheel -Destination $targetWheel -Force
        }

        Invoke-InstallCommand "Installing Mercurio Python SDK from wheel" {
            py -m pip install --upgrade $targetWheel
        }
    } else {
        if (-not (Test-Path $pythonProject)) {
            throw "Python project not found: $pythonProject"
        }

        Invoke-InstallCommand "Installing Mercurio Python SDK in editable mode" {
            py -m pip install -e $pythonProject
        }
    }

    Invoke-InstallCommand "Verifying Mercurio Python SDK import" {
        py -c "import mercurio; print(mercurio.__version__)"
    }
}

Write-Host ""
Write-Host "Mercurio installed to $InstallRoot"
Write-Host "Open a new terminal for PATH changes to take effect."
