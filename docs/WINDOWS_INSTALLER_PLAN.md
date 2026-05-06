# Windows Installer Plan

## Goal

Provide a Windows installation path that makes Mercurio usable from PowerShell, Python, VS Code, notebooks, and future desktop tooling without requiring users to manually copy binaries or configure environment variables.

The installer should remain a packaging layer over the repository's normal build artifacts. It should not introduce a second runtime architecture.

## Install Targets

Install Mercurio command-line binaries under the current user's local application directory:

```text
%LOCALAPPDATA%\Mercurio\
  bin\
    mercurio.exe
  wheels\
    mercurio-0.1.0-py3-none-any.whl
```

Add this directory to the user `PATH`:

```text
%LOCALAPPDATA%\Mercurio\bin
```

This should make these commands available in new terminals:

```powershell
mercurio --help
mercurio server --port 0
```

## Bootstrap Script

The first implementation is a PowerShell bootstrap script:

```powershell
.\tools\install-mercurio.ps1 -Build -InstallPython
```

Supported options:

- `-Configuration release|debug`: selects the built executable from `target\<configuration>`.
- `-InstallRoot PATH`: overrides `%LOCALAPPDATA%\Mercurio`.
- `-InstallPython`: installs the Python SDK into the active `py` environment.
- `-PythonMode editable|wheel`: selects editable source install or wheel install.
- `-PythonWheel PATH`: installs a specific Python SDK wheel.
- `-Build`: builds the selected Rust binary before install.
- `-DryRun`: prints the planned actions without mutating the machine.

The script should:

1. Build or locate `mercurio.exe`.
2. Copy it to `%LOCALAPPDATA%\Mercurio\bin`.
3. Add the bin directory to the user `PATH` if missing.
4. Verify `mercurio.exe --help`.
5. Optionally install the Python SDK from source or wheel.
6. Verify `import mercurio`.

## Release Package

Build a release package before formal installer packaging:

```powershell
.\tools\build-release-package.ps1 -Clean
```

This writes:

```text
dist\
  mercurio-windows-x64\
    bin\
      mercurio.exe
    wheels\
      mercurio-0.1.0-py3-none-any.whl
    manifest.json
```

Install from the release package:

```powershell
.\tools\install-mercurio.ps1 -InstallPython -PythonMode wheel
```

Development installs can still use editable mode:

```powershell
.\tools\install-mercurio.ps1 -Configuration debug -InstallPython -PythonMode editable
```

## Python SDK Policy

Do not silently install into every Python environment on the machine. The bootstrap script should install only into the `py` launcher's selected default Python when `-InstallPython` is provided.

Future installers may provide choices:

- install no Python package
- install into the default `py` environment
- install into a Mercurio-managed virtual environment
- install from a bundled wheel

The Python SDK must continue to support manual setup:

```powershell
py -m pip install mercurio
$env:MERCURIO_EXE = "C:\Path\To\mercurio.exe"
```

## WiX Installer

After the bootstrap behavior is stable, add a WiX project:

```text
installer/windows/wix/
  Product.wxs
  Package.wxs
  build.ps1
```

The WiX installer should package the same install layout and expose the same choices. WiX should not define a different installation model.

Recommended WiX behavior:

- per-user install by default
- optional per-machine install later
- install `mercurio.exe`
- add the install bin directory to PATH
- optionally install bundled Python wheel
- include uninstall metadata
- include upgrade metadata

## Verification

Minimum verification after install:

```powershell
mercurio --help
mercurio server --help
py -c "import mercurio; print(mercurio.__version__)"
```

For Python backend integration:

```powershell
$env:MERCURIO_EXE = "mercurio"
py -m unittest discover -s python\tests
```

## Open Questions

- Should the official installer be per-user only or offer per-machine installation?
- Should the Python SDK be bundled as a wheel in the installer or installed from PyPI?
- Should the installer create a Mercurio-managed virtual environment for notebooks?
- Should the installer register VS Code extension recommendations once the extension exists?
- Should PATH changes be optional for managed enterprise deployments?
