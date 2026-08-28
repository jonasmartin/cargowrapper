use std::env;
use std::io::{self, Write};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, exit};
use std::time::{SystemTime, UNIX_EPOCH};

fn executable_name(name: &str) -> String {
    format!("{name}{}", env::consts::EXE_SUFFIX)
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let home = env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = env::var_os("HOME");

    home.map(PathBuf::from)
}

fn real_cargo() -> Option<PathBuf> {
    // Rustup honours CARGO_HOME, so that is where the real cargo lives when it is set.
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".cargo")))?;

    Some(cargo_home.join("bin").join(executable_name("cargo")))
}

fn parse_config_log_file(contents: &str) -> Option<PathBuf> {
    contents.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }

        let (key, value) = line.split_once('=')?;
        if key.trim() != "LOG_FILE" {
            return None;
        }

        let value = value.trim();
        let value = if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            &value[1..value.len() - 1]
        } else {
            value
        };

        (!value.is_empty()).then(|| PathBuf::from(value))
    })
}

fn configured_log_file() -> Option<PathBuf> {
    if let Some(path) = env::var_os("CARGO_WRAPPER_LOG_FILE") {
        return Some(PathBuf::from(path));
    }

    let executable_dir = env::current_exe().ok()?.parent()?.to_path_buf();
    let contents = std::fs::read_to_string(executable_dir.join("wrapper.toml")).ok()?;
    let path = parse_config_log_file(&contents)?;

    Some(if path.is_absolute() {
        path
    } else {
        executable_dir.join(path)
    })
}

fn format_utc_timestamp(seconds: u64, milliseconds: u32) -> String {
    let days = (seconds / 86_400) as i64;
    let seconds_in_day = seconds % 86_400;
    let hour = seconds_in_day / 3_600;
    let minute = (seconds_in_day % 3_600) / 60;
    let second = seconds_in_day % 60;

    // Convert days since the Unix epoch to a Gregorian calendar date.
    let shifted_days = days + 719_468;
    let era = if shifted_days >= 0 {
        shifted_days
    } else {
        shifted_days - 146_096
    } / 146_097;
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{milliseconds:03}Z")
}

fn utc_timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_utc_timestamp(now.as_secs(), now.subsec_millis())
}

#[cfg(unix)]
fn shell_profile(home: &Path) -> PathBuf {
    let shell = env::var_os("SHELL")
        .and_then(|shell| PathBuf::from(shell).file_name().map(|name| name.to_owned()));

    match shell.as_deref().and_then(|name| name.to_str()) {
        Some("zsh") => home.join(".zprofile"),
        Some("bash") => home.join(".bash_profile"),
        _ => home.join(".profile"),
    }
}

#[cfg(unix)]
fn ensure_unix_path(profile: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::fs::OpenOptions;

    const PATH_LINE: &str = r#"export PATH="$HOME/.local/bin:$PATH""#;
    let existing = match std::fs::read_to_string(profile) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };

    if existing.lines().any(|line| line.trim() == PATH_LINE) {
        return Ok(());
    }

    let mut file = OpenOptions::new().create(true).append(true).open(profile)?;
    if !existing.is_empty() && !existing.ends_with('\n') {
        writeln!(file)?;
    }
    writeln!(file, "# Added by cargo-wrapper")?;
    writeln!(file, "{PATH_LINE}")?;
    Ok(())
}

fn install_wrapper() -> Result<(), Box<dyn std::error::Error>> {
    let project_dir = env::current_dir()?;
    let release_executable = project_dir
        .join("target")
        .join("release")
        .join(executable_name("cargowrapper"));
    let home = home_dir().ok_or("cannot determine the user's home directory")?;
    #[cfg(windows)]
    let bin_dir = home.join("bin");
    #[cfg(unix)]
    let bin_dir = home.join(".local").join("bin");
    let installed_executable = bin_dir.join(executable_name("cargo"));
    #[cfg(unix)]
    let profile = shell_profile(&home);

    println!("The installer will:");
    println!("  [ ] Build the wrapper in release mode.");
    println!("  [ ] Create {}.", bin_dir.display());
    println!(
        "  [ ] Copy {} to {}.",
        release_executable.display(),
        installed_executable.display()
    );
    #[cfg(windows)]
    println!("  [ ] Add {} to your user PATH.", bin_dir.display());
    #[cfg(unix)]
    println!(
        "  [ ] Add {} to PATH in {}.",
        bin_dir.display(),
        profile.display()
    );
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

    let cargo = real_cargo()
        .ok_or("cannot locate Cargo: neither CARGO_HOME nor the home directory is defined")?;
    let build_status = Command::new(cargo).args(["build", "--release"]).status()?;
    if !build_status.success() {
        return Err("release build failed".into());
    }
    println!("  [x] Built the wrapper in release mode.");

    std::fs::create_dir_all(&bin_dir)?;
    println!("  [x] Ensured {} exists.", bin_dir.display());

    std::fs::copy(&release_executable, &installed_executable)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&installed_executable)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&installed_executable, permissions)?;
    }
    println!(
        "  [x] Copied the wrapper to {}.",
        installed_executable.display()
    );

    #[cfg(windows)]
    {
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
    }

    #[cfg(unix)]
    {
        ensure_unix_path(&profile)?;
        println!(
            "  [x] Ensured {} is in PATH via {}.",
            bin_dir.display(),
            profile.display()
        );
    }

    println!("Installed {}.", installed_executable.display());
    #[cfg(windows)]
    println!("Open a new terminal and run `cargo wrapper` to verify it.");
    #[cfg(unix)]
    println!(
        "Open a new terminal or run `source {}`, then run `cargo wrapper` to verify it.",
        profile.display()
    );
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
        eprintln!(
            "cargo-wrapper: cannot locate Cargo: neither CARGO_HOME nor the home directory is defined"
        );
        exit(1);
    };

    if let Some(log_file_path) = configured_log_file() {
        use std::fs::OpenOptions;

        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file_path)
        {
            Ok(mut log_file) => {
                let timestamp = utc_timestamp();
                writeln!(log_file, "{timestamp} Executing: cargo {}", args.join(" ")).unwrap();
            }
            Err(error) => {
                eprintln!(
                    "cargo-wrapper: cannot write {}: {error}",
                    log_file_path.display()
                );
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn unix_executable_names_have_no_suffix() {
        assert_eq!(executable_name("cargo"), "cargo");
        assert_eq!(executable_name("cargowrapper"), "cargowrapper");
    }

    #[test]
    fn parses_plain_and_quoted_log_file_settings() {
        assert_eq!(
            parse_config_log_file("OTHER=value\nLOG_FILE=commands.log\n"),
            Some(PathBuf::from("commands.log"))
        );
        assert_eq!(
            parse_config_log_file("LOG_FILE = \"logs/commands.log\""),
            Some(PathBuf::from("logs/commands.log"))
        );
        assert_eq!(parse_config_log_file("# LOG_FILE=ignored.log"), None);
    }

    #[test]
    fn formats_utc_timestamps_without_external_dependencies() {
        assert_eq!(format_utc_timestamp(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            format_utc_timestamp(1_735_689_599, 123),
            "2024-12-31T23:59:59.123Z"
        );
    }

    #[test]
    fn adding_the_path_to_a_profile_is_idempotent() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let profile = env::temp_dir().join(format!(
            "cargo-wrapper-profile-{}-{unique}",
            std::process::id()
        ));

        ensure_unix_path(&profile).unwrap();
        ensure_unix_path(&profile).unwrap();
        let contents = std::fs::read_to_string(&profile).unwrap();
        std::fs::remove_file(profile).unwrap();

        assert_eq!(contents.matches("$HOME/.local/bin").count(), 1);
    }
}
