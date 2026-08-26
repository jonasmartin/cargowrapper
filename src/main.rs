use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, exit};

fn real_cargo() -> PathBuf {
    PathBuf::from(env::var("USERPROFILE").expect("USERPROFILE is not defined"))
        .join(".cargo")
        .join("bin")
        .join("cargo.exe")
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

    let build_status = Command::new(real_cargo())
        .args(["build", "--release"])
        .status()?;
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

    if let Some(command) = args.first_mut() {
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
        "build", "test", "run", "check", "bench", "clippy", "doc", "rustc", "rustdoc",
    ];

    if let Some(command) = args.first() {
        if locked_commands.contains(&command.as_str()) && !args.iter().any(|arg| arg == "--locked")
        {
            args.insert(1, "--locked".to_string());
        }
    }

    let cargo = real_cargo();

    if let Ok(log_file_path) = env::var("CARGO_WRAPPER_LOG_FILE") {
        use std::fs::OpenOptions;

        let mut log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file_path)
            .unwrap();
        writeln!(log_file, "Executing: cargo {}", args.join(" ")).unwrap();
    }

    let status = Command::new(cargo)
        .args(&args)
        .status()
        .expect("failed to execute cargo");

    exit(status.code().unwrap_or(1));
}
