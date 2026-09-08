# Cargo Wrapper

A small Linux, macOS, and Windows wrapper for Cargo that enforces locked dependency resolution for common commands and prevents accidental lockfile updates.

The wrapper forwards commands to the real Cargo executable in `$CARGO_HOME/bin`, or to the standard Rustup installation when `CARGO_HOME` is unset:

- Linux: `$HOME/.cargo/bin/cargo`
- macOS: `$HOME/.cargo/bin/cargo`
- Windows: `%USERPROFILE%\.cargo\bin\cargo.exe`

## Behavior

- Adds `--locked` to these commands when it is not already present:
  - `build`
  - `test`
  - `run`
  - `check`
  - `bench`
  - `clippy`
  - `doc`
  - `rustc`
  - `rustdoc`
- Rejects `cargo update`.
- Provides `cargo forceupdate` as an explicit way to run `cargo update`.
- Provides `cargo wrapper` to confirm that the wrapper is active.
- Runs enabled executable plugins as middleware around ordinary Cargo commands.
- Provides `cargo wrapper-install` to interactively install the wrapper.
- Optionally logs forwarded Cargo commands.

All other commands and arguments are passed through unchanged.

## Automatic installation

From the project directory, run:

```sh
cargo run -- wrapper-install
```

The command displays its planned actions and asks `Continue? [Y/n]`. Press Enter or enter `Y` to:

1. Build the release executable.
2. Create the wrapper bin directory.
3. Copy and rename the release executable to `cargo` (or `cargo.exe`).
4. Prepend the wrapper bin directory to your persistent `PATH`.

On macOS, the wrapper is installed as `$HOME/.local/bin/cargo`. The installer updates `.zprofile` for zsh, `.bash_profile` for bash, or `.profile` for other shells. Open a new terminal, or source the profile printed by the installer, and verify:

```sh
which cargo
cargo wrapper
```

`which cargo` should report `$HOME/.local/bin/cargo`.

On Windows, the wrapper is installed as `%USERPROFILE%\bin\cargo.exe`. Open a new terminal and verify with:

```powershell
Get-Command cargo
cargo wrapper
```

Enter `n` at the prompt to cancel without making installation changes.

## Manual installation

### macOS

```sh
cargo build --release
mkdir -p "$HOME/.local/bin"
cp target/release/cargowrapper "$HOME/.local/bin/cargo"
chmod 755 "$HOME/.local/bin/cargo"
printf '\n# Added by cargo-wrapper\nexport PATH="$HOME/.local/bin:$PATH"\n' >> "$HOME/.zprofile"
source "$HOME/.zprofile"
```

Do not install the wrapper in `$HOME/.cargo/bin`; it needs the real Cargo executable there for forwarding. Verify with `which cargo` and `cargo wrapper`.

### Windows

#### 1. Build the executable

```powershell
cargo build --release
```

#### 2. Install it in a separate directory

Create a directory for wrapper executables, then copy and rename the compiled executable to `cargo.exe`:

```powershell
New-Item -ItemType Directory -Force C:\bin
Copy-Item target\release\cargowrapper.exe C:\bin\cargo.exe
```

Do not place the wrapper in `%USERPROFILE%\.cargo\bin` or replace the real Cargo executable there. The wrapper needs that executable to forward commands.

#### 3. Add the directory to `PATH`

Prepend `C:\bin` to your persistent user `PATH` with PowerShell:

```powershell
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
$newPath = if ($userPath) { "C:\bin;$userPath" } else { "C:\bin" }
[Environment]::SetEnvironmentVariable("Path", $newPath, "User")
```

Open a new terminal for the change to take effect. To use it in the current PowerShell session immediately, run:

```powershell
$env:Path = "C:\bin;$env:Path"
```

Alternatively, use **System Properties → Environment Variables**, edit the user `Path`, add `C:\bin`, and move it above `%USERPROFILE%\.cargo\bin`.

The order is important: Windows must find `C:\bin\cargo.exe` before the real Cargo executable.

#### 4. Verify the installation

Check which executable Windows finds:

```powershell
Get-Command cargo
```

Its `Source` should be `C:\bin\cargo.exe`. Then check that the wrapper responds:

```powershell
cargo wrapper
```

Expected output:

```text
Cargo wrapper is active.
```

## Plugins

Cargo Wrapper can run locally installed native executables around ordinary Cargo calls. The core does not download plugins: obtain and verify a precompiled plugin binary yourself, then provide its local path. Installation copies the file into a private user directory and leaves it disabled.

```sh
cargo wrapper plugin install <plugin-name> <local-plugin-binary>
cargo wrapper plugin list
cargo wrapper plugin enable <plugin-name>
cargo wrapper plugin disable <plugin-name>
cargo wrapper plugin uninstall <plugin-name>
```

Replace `<plugin-name>` with the plugin's documented registration name and `<local-plugin-binary>` with the path to the verified executable you downloaded. Concrete installation commands belong in each plugin's own documentation.

Enabling a plugin means that future Cargo calls execute that native binary with your user privileges. Only enable binaries whose source and release provenance you trust. Install and enable are deliberately separate operations, and uninstall refuses active plugins; disable them first. Plugin-owned configuration and data are retained when its executable is uninstalled.

Before installation, pin a plugin release, read its release notes, download the correct precompiled artifact from the plugin's release page, and verify its published checksum and GitHub artifact attestation when available. The core never downloads, builds, updates, inspects, or automatically enables plugin code.

Multiple active plugins execute in ascending name order, which is also the order shown by `list`. A missing or malformed active plugin stops the Cargo operation instead of being silently skipped. Plugin-management commands bypass the plugin chain, so `list`, `disable`, and `uninstall` remain available to repair a broken installation.

The list statuses mean:

- `active`: the executable and enabled marker are valid, so ordinary Cargo calls run it.
- `inactive`: the executable is valid but disabled.
- `broken`: the executable, marker, or directory layout is missing or malformed.

To recover from a broken active entry, inspect it with `cargo wrapper plugin list`, disable it with `cargo wrapper plugin disable <name>`, and then repair or uninstall it. A broken inactive entry does not block ordinary Cargo calls.

The default user-wide plugin locations are:

- Linux and macOS: `$XDG_DATA_HOME/cargowrapper/plugins`, or `$HOME/.local/share/cargowrapper/plugins` when `XDG_DATA_HOME` is unset.
- Windows: `%LOCALAPPDATA%\CargoWrapper\plugins`.

`CARGO_WRAPPER_PLUGIN_DIR` overrides this location for tests and managed deployments. Plugin authors should follow the versioned middleware contract in [PLUGIN-PROTOCOL.md](PLUGIN-PROTOCOL.md).

## Command logging

Logging is disabled by default. Set `CARGO_WRAPPER_LOG_FILE` to enable it and specify the destination file. This environment variable takes precedence over file configuration:

```sh
# macOS
export CARGO_WRAPPER_LOG_FILE="$HOME/cargo-wrapper.log"
cargo check
```

```powershell
# Windows
$env:CARGO_WRAPPER_LOG_FILE = "C:\logs\cargo-wrapper.log"
cargo check
```

If `CARGO_WRAPPER_LOG_FILE` is not set, the wrapper looks for `wrapper.toml` in the same directory as the wrapper executable. Set `LOG_FILE` using either an unquoted or quoted value:

```toml
LOG_FILE=logs/cargo-wrapper.log
```

Relative paths are resolved from the wrapper executable's directory. Absolute paths can also be used. Each forwarded command is appended to the selected file with an RFC 3339 UTC timestamp:

```text
2026-08-27T16:42:15.123456Z Executing: cargo check --locked
```

The destination directory must already exist.

## Examples

```powershell
# Runs: cargo build --locked
cargo build

# Rejected with a warning
cargo update

# Explicitly runs: cargo update
cargo forceupdate
```

## License

This project is licensed under the [MIT License](LICENSE).
