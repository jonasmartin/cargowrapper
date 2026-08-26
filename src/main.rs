use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, exit};

fn real_cargo() -> Option<PathBuf> {
    // Rustup honours CARGO_HOME, so that is where the real cargo lives when it is set.
    if let Some(cargo_home) = env::var_os("CARGO_HOME") {
        return Some(PathBuf::from(cargo_home).join("bin").join("cargo.exe"));
    }

    // Otherwise, the real cargo may be in the user's profile directory.
    env::var_os("USERPROFILE").map(|user_profile| {
        PathBuf::from(user_profile)
            .join(".cargo")
            .join("bin")
            .join("cargo.exe")
    })
}

fn install_wrapper() -> Result<(), Box<dyn std::error::Error>> {
    let project_dir = env::current_dir()?;
    let release_executable = project_dir
        .join("target")
        .join("release")
        .join("cargowrapper.exe");
    let bin_dir = PathBuf::from(env::var("USERPROFILE")?).join("bin");
    let installed_executable = bin_dir.join("cargo.exe");

    println!("The installer will:");
    println!("  [ ] Build the wrapper in release mode.");
    println!("  [ ] Create {}.", bin_dir.display());
    println!(
        "  [ ] Copy {} to {}.",
        release_executable.display(),
        installed_executable.display()
    );
    println!("  [ ] Add {} to your user PATH.", bin_dir.display());
    print!("Continue? [Y/n] ");
    io::stdout().flush()?;

    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if !matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    ) {
        println!("Installation cancelled.");
        return Ok(());
    }

    let cargo = real_cargo().ok_or("neither CARGO_HOME nor USERPROFILE is defined")?;
    let build_status = Command::new(cargo).args(["build", "--release"]).status()?;
    if !build_status.success() {
        return Err("release build failed".into());
    }
    println!("  [x] Built the wrapper in release mode.");

    std::fs::create_dir_all(&bin_dir)?;
    println!("  [x] Ensured {} exists.", bin_dir.display());

    std::fs::copy(&release_executable, &installed_executable)?;
    println!(
        "  [x] Copied the wrapper to {}.",
        installed_executable.display()
    );

    let path_script = r#"
$bin = $env:CARGO_WRAPPER_INSTALL_DIR
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$entries = @($userPath -split ';' | Where-Object { $_ })
if (-not ($entries | Where-Object { $_.TrimEnd('\\') -ieq $bin.TrimEnd('\\') })) {
    $newPath = if ($userPath) { "$bin;$userPath" } else { $bin }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
}
"#;
    let path_status = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", path_script])
        .env("CARGO_WRAPPER_INSTALL_DIR", &bin_dir)
        .status()?;
    if !path_status.success() {
        return Err("failed to update the user PATH".into());
    }
    println!("  [x] Ensured {} is in your user PATH.", bin_dir.display());

    println!("Installed {}.", installed_executable.display());
    println!("Open a new terminal and run `cargo wrapper` to verify it.");
    Ok(())
}

fn main() {
    let mut args: Vec<String> = env::args().skip(1).collect();

    // A leading +toolchain is consumed by rustup, so the subcommand is the argument after it.
    let sub = if args.first().is_some_and(|arg| arg.starts_with('+')) {
        1
    } else {
        0
    };

    if let Some(command) = args.get_mut(sub) {
        match command.as_str() {
            "wrapper" => {
                println!("Cargo wrapper is active.");
                exit(0);
            }
            "wrapper-install" => {
                if let Err(error) = install_wrapper() {
                    eprintln!("Installation failed: {error}");
                    exit(1);
                }
                exit(0);
            }
            "update" => {
                eprintln!(
                    "Warning: `cargo update` cannot be run with `--locked`. Ensure you are in a safe environment with cooldown set before calling it. To proceed, use `cargo forceupdate`."
                );
                exit(1);
            }
            "forceupdate" => *command = "update".to_string(),
            _ => {}
        }
    }

    let locked_commands = [
        "build", "test", "run", "check", "bench", "clippy", "doc", "rustc", "rustdoc", "install",
    ];

    // Arguments after a bare -- belong to the program cargo launches.
    let cargo_args_end = args
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(args.len());
    let already_locked = args[..cargo_args_end].iter().any(|arg| arg == "--locked");

    let needs_locked = args
        .get(sub)
        .is_some_and(|command| locked_commands.contains(&command.as_str()));

    if needs_locked && !already_locked {
        args.insert(sub + 1, "--locked".to_string());

        if env::var_os("CARGO_WRAPPER_VERBOSE").is_some() {
            eprintln!("cargo-wrapper: added --locked");
        }
    }

    let Some(cargo) = real_cargo() else {
        eprintln!("cargo-wrapper: neither CARGO_HOME nor USERPROFILE is defined");
        exit(1);
    };

    if let Ok(log_file_path) = env::var("CARGO_WRAPPER_LOG_FILE") {
        use std::fs::OpenOptions;

        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file_path)
        {
            Ok(mut log_file) => {
                writeln!(log_file, "Executing: cargo {}", args.join(" ")).unwrap();
            }
            Err(error) => {
                eprintln!("cargo-wrapper: cannot write {log_file_path}: {error}");
            }
        }
    }

    let status = Command::new(&cargo)
        .args(&args)
        .status()
        .unwrap_or_else(|error| {
            eprintln!("cargo-wrapper: cannot run {}: {error}", cargo.display());
            exit(1);
        });

    exit(status.code().unwrap_or(1));
}
