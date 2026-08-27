# Cargo Wrapper

A small macOS and Windows wrapper for Cargo that enforces locked dependency resolution for common commands and prevents accidental lockfile updates.

The wrapper forwards commands to the real Cargo executable in `$CARGO_HOME/bin`, or to the standard Rustup installation when `CARGO_HOME` is unset:

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

## Command logging

Logging is disabled by default. Set `CARGO_WRAPPER_LOG_FILE` to enable it and specify the destination file:

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

Each forwarded command is appended to the file:

```text
Executing: cargo check --locked
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
