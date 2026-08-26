# Cargo Wrapper

A small Windows wrapper for Cargo that enforces locked dependency resolution for common commands and prevents accidental lockfile updates.

The wrapper forwards commands to the real Cargo executable at:

```text
%USERPROFILE%\.cargo\bin\cargo.exe
```

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

```powershell
cargo run -- wrapper-install
```

The command first displays every action with an unchecked box and asks `Continue? [Y/n]`. Press Enter or enter `Y` to run the steps; each completed action is then printed with `[x]`:

1. Build the release executable.
2. Create `%USERPROFILE%\bin`.
3. Copy the release executable to `%USERPROFILE%\bin\cargo.exe`.
4. Prepend `%USERPROFILE%\bin` to your persistent user `PATH` if it is not already present.

Enter `n` to cancel without making these installation changes. After installation, open a new terminal and verify it:

```powershell
Get-Command cargo
cargo wrapper
```

`Get-Command cargo` should report `%USERPROFILE%\bin\cargo.exe`, and the second command should print `Cargo wrapper is active.`

## Manual installation

### 1. Build the executable

```powershell
cargo build --release
```

### 2. Install it in a separate directory

Create a directory for wrapper executables, then copy and rename the compiled executable to `cargo.exe`:

```powershell
New-Item -ItemType Directory -Force C:\bin
Copy-Item target\release\cargowrapper.exe C:\bin\cargo.exe
```

Do not place the wrapper in `%USERPROFILE%\.cargo\bin` or replace the real Cargo executable there. The wrapper needs that executable to forward commands.

### 3. Add the directory to `PATH`

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

### 4. Verify the installation

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

```powershell
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
