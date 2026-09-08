//! Manages user-installed executable plugins and their middleware process chain.
//! Plugin state is filesystem-only so the core wrapper keeps zero runtime dependencies.

use std::env;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub const PROTOCOL_VERSION: &str = "1";

pub const ENV_PROTOCOL: &str = "CARGO_WRAPPER_PLUGIN_PROTOCOL";
pub const ENV_PLUGIN_NAME: &str = "CARGO_WRAPPER_PLUGIN_NAME";
pub const ENV_REAL_CARGO: &str = "CARGO_WRAPPER_REAL_CARGO";
pub const ENV_NEXT: &str = "CARGO_WRAPPER_NEXT";
pub const ENV_ORIGINAL_SUBCOMMAND: &str = "CARGO_WRAPPER_ORIGINAL_SUBCOMMAND";
pub const ENV_CHAIN: &str = "CARGO_WRAPPER_PLUGIN_CHAIN";
pub const ENV_INDEX: &str = "CARGO_WRAPPER_PLUGIN_INDEX";
pub const ENV_ROOT: &str = "CARGO_WRAPPER_PLUGIN_ROOT";
pub const ENV_SESSION: &str = "CARGO_WRAPPER_PLUGIN_SESSION";

const RESERVED_ENVIRONMENT: &[&str] = &[
    ENV_PROTOCOL,
    ENV_PLUGIN_NAME,
    ENV_REAL_CARGO,
    ENV_NEXT,
    ENV_ORIGINAL_SUBCOMMAND,
    ENV_CHAIN,
    ENV_INDEX,
    ENV_ROOT,
    ENV_SESSION,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PluginStatus {
    Active,
    Inactive,
    Broken,
}

impl PluginStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Broken => "broken",
        }
    }
}

pub fn usage() -> &'static str {
    "Usage:\n  cargo wrapper plugin list\n  cargo wrapper plugin install <name> <local-binary>\n  cargo wrapper plugin enable <name>\n  cargo wrapper plugin disable <name>\n  cargo wrapper plugin uninstall <name>"
}

pub fn manage(arguments: &[String]) -> Result<i32, String> {
    let host = PluginHost::new(PluginHost::default_root()?)?;
    match arguments {
        [command] if command == "list" => {
            let installed = host.list()?;
            if installed.is_empty() {
                println!("No Cargo Wrapper plugins are installed.");
            } else {
                println!("NAME\tSTATUS\tEXECUTABLE");
                for plugin in installed {
                    println!(
                        "{}\t{}\t{}",
                        plugin.name,
                        plugin.status.label(),
                        plugin.executable.display()
                    );
                }
            }
        }
        [command, name, source] if command == "install" => {
            let installed = host.install(name, &PathBuf::from(source))?;
            println!(
                "Installed inactive plugin `{name}` at {}.",
                installed.display()
            );
            #[cfg(unix)]
            println!(
                "The copied binary was made executable only inside the private plugin directory."
            );
            println!("Review it, then enable it with: cargo wrapper plugin enable {name}");
        }
        [command, name] if command == "enable" => {
            println!(
                "Plugin `{name}` {}.",
                if host.enable(name)? {
                    "enabled"
                } else {
                    "was already enabled"
                }
            );
            println!(
                "Future Cargo calls will execute this native binary with your user privileges."
            );
        }
        [command, name] if command == "disable" => println!(
            "Plugin `{name}` {}.",
            if host.disable(name)? {
                "disabled"
            } else {
                "was already disabled"
            }
        ),
        [command, name] if command == "uninstall" => {
            host.uninstall(name)?;
            println!("Uninstalled plugin `{name}`.");
            println!("Plugin-owned configuration and data were intentionally retained.");
        }
        _ => return Err(usage().to_owned()),
    }
    Ok(0)
}

pub fn continue_if_requested<F>(args: &[String], run_real_cargo: F) -> Result<Option<i32>, String>
where
    F: FnOnce(&Path, &[String]) -> Result<i32, String>,
{
    let Some(context) = ChainContext::from_environment()? else {
        return Ok(None);
    };
    let host = PluginHost::from_context(&context);
    host.dispatch(&context, args, run_real_cargo).map(Some)
}

pub fn dispatch<F>(
    real_cargo: &Path,
    original_subcommand: Option<&str>,
    args: &[String],
    run_real_cargo: F,
) -> Result<i32, String>
where
    F: FnOnce(&Path, &[String]) -> Result<i32, String>,
{
    let host = PluginHost::new(PluginHost::default_root()?)?;
    let context = host.begin_chain(real_cargo, original_subcommand)?;
    host.dispatch(&context, args, run_real_cargo)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginInfo {
    pub name: String,
    pub status: PluginStatus,
    pub executable: PathBuf,
    marker_requires_fail_closed: bool,
}

#[derive(Clone, Debug)]
pub struct ChainContext {
    chain: Vec<String>,
    index: usize,
    root: PathBuf,
    real_cargo: PathBuf,
    original_subcommand: Option<String>,
    session: String,
}

impl ChainContext {
    pub fn from_environment() -> Result<Option<Self>, String> {
        let Some(protocol) = env::var_os(ENV_PROTOCOL) else {
            return Ok(None);
        };
        if protocol != OsStr::new(PROTOCOL_VERSION) {
            return Err(format!(
                "cargo-wrapper: unsupported plugin protocol {}; expected {PROTOCOL_VERSION}",
                protocol.to_string_lossy()
            ));
        }

        let chain = required_unicode_environment(ENV_CHAIN)?;
        let chain = parse_chain(&chain)?;
        let index = required_unicode_environment(ENV_INDEX)?
            .parse::<usize>()
            .map_err(|_| "cargo-wrapper: malformed plugin continuation index".to_owned())?;
        if index > chain.len() {
            return Err("cargo-wrapper: plugin continuation index exceeds chain length".to_owned());
        }
        let root = required_absolute_path(ENV_ROOT)?;
        let real_cargo = required_absolute_path(ENV_REAL_CARGO)?;
        let session = required_unicode_environment(ENV_SESSION)?;
        if session.is_empty() || session.len() > 256 {
            return Err("cargo-wrapper: malformed plugin continuation session".to_owned());
        }
        let original_subcommand = env::var_os(ENV_ORIGINAL_SUBCOMMAND)
            .map(|value| {
                value.into_string().map_err(|_| {
                    "cargo-wrapper: plugin original subcommand is not valid Unicode".to_owned()
                })
            })
            .transpose()?;

        Ok(Some(Self {
            chain,
            index,
            root,
            real_cargo,
            original_subcommand,
            session,
        }))
    }
}

#[derive(Clone, Debug)]
pub struct PluginHost {
    root: PathBuf,
}

impl PluginHost {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        Ok(Self {
            root: absolute_path(root)?,
        })
    }

    pub fn default_root() -> Result<PathBuf, String> {
        if let Some(path) = env::var_os("CARGO_WRAPPER_PLUGIN_DIR") {
            return absolute_path(PathBuf::from(path));
        }

        #[cfg(windows)]
        {
            return windows_default_root(env::var_os("LOCALAPPDATA").map(PathBuf::from));
        }

        #[cfg(not(windows))]
        {
            unix_default_root(
                env::var_os("XDG_DATA_HOME").map(PathBuf::from),
                env::var_os("HOME").map(PathBuf::from),
            )
        }
    }

    pub fn install(&self, name: &str, source: &Path) -> Result<PathBuf, String> {
        validate_name(name)?;
        let source = source.canonicalize().map_err(|error| {
            format!(
                "cargo-wrapper: cannot resolve plugin binary {}: {error}",
                source.display()
            )
        })?;
        if !source
            .metadata()
            .map_err(|error| {
                format!(
                    "cargo-wrapper: cannot inspect plugin binary {}: {error}",
                    source.display()
                )
            })?
            .is_file()
        {
            return Err(format!(
                "cargo-wrapper: plugin source is not a regular file: {}",
                source.display()
            ));
        }

        ensure_private_root(&self.root)?;
        let directory = self.plugin_directory(name);
        create_private_directory(&directory).map_err(|error| {
            format!(
                "cargo-wrapper: cannot create plugin directory {}: {error}",
                directory.display()
            )
        })?;
        let executable = directory.join(plugin_executable_name());
        let temporary = directory.join(format!(
            "plugin.tmp-{}-{}",
            std::process::id(),
            unique_nonce()
        ));

        let copy_result = (|| -> Result<(), String> {
            let mut input = fs::File::open(&source).map_err(|error| {
                format!(
                    "cargo-wrapper: cannot open plugin binary {}: {error}",
                    source.display()
                )
            })?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o700);
            }
            let mut output = options.open(&temporary).map_err(|error| {
                format!(
                    "cargo-wrapper: cannot create temporary plugin binary {}: {error}",
                    temporary.display()
                )
            })?;
            std::io::copy(&mut input, &mut output).map_err(|error| {
                format!(
                    "cargo-wrapper: cannot copy plugin binary to {}: {error}",
                    temporary.display()
                )
            })?;
            output.sync_all().map_err(|error| {
                format!(
                    "cargo-wrapper: cannot flush plugin binary {}: {error}",
                    temporary.display()
                )
            })?;
            #[cfg(unix)]
            set_mode(&temporary, 0o700)?;
            fs::rename(&temporary, &executable).map_err(|error| {
                format!(
                    "cargo-wrapper: cannot activate installed plugin binary {}: {error}",
                    executable.display()
                )
            })?;
            Ok(())
        })();

        if let Err(error) = copy_result {
            let _ = fs::remove_file(&temporary);
            let _ = fs::remove_dir(&directory);
            return Err(error);
        }
        Ok(executable)
    }

    pub fn enable(&self, name: &str) -> Result<bool, String> {
        validate_name(name)?;
        let executable = self.plugin_executable(name);
        validate_executable(&executable)?;
        let marker = self.enabled_marker(name);
        match fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.file_type().is_file() => return Ok(false),
            Ok(_) => {
                return Err(format!(
                    "cargo-wrapper: invalid enabled marker for plugin {name}: {}",
                    marker.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "cargo-wrapper: cannot inspect enabled marker {}: {error}",
                    marker.display()
                ));
            }
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let marker_file = options.open(&marker).map_err(|error| {
            format!(
                "cargo-wrapper: cannot enable plugin {name} at {}: {error}",
                marker.display()
            )
        })?;
        marker_file.sync_all().map_err(|error| {
            format!(
                "cargo-wrapper: cannot flush enabled marker {}: {error}",
                marker.display()
            )
        })?;
        Ok(true)
    }

    pub fn disable(&self, name: &str) -> Result<bool, String> {
        validate_name(name)?;
        let marker = self.enabled_marker(name);
        match fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.file_type().is_file() => {
                fs::remove_file(&marker).map_err(|error| {
                    format!(
                        "cargo-wrapper: cannot disable plugin {name} at {}: {error}",
                        marker.display()
                    )
                })?;
                Ok(true)
            }
            Ok(_) => Err(format!(
                "cargo-wrapper: refusing invalid enabled marker for plugin {name}: {}",
                marker.display()
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(format!(
                "cargo-wrapper: cannot inspect enabled marker {}: {error}",
                marker.display()
            )),
        }
    }

    pub fn uninstall(&self, name: &str) -> Result<(), String> {
        validate_name(name)?;
        let directory = self.plugin_directory(name);
        let directory_metadata = fs::symlink_metadata(&directory).map_err(|error| {
            format!(
                "cargo-wrapper: cannot inspect installed plugin {name} at {}: {error}",
                directory.display()
            )
        })?;
        if !directory_metadata.file_type().is_dir() {
            return Err(format!(
                "cargo-wrapper: plugin path is not a directory: {}",
                directory.display()
            ));
        }
        match fs::symlink_metadata(self.enabled_marker(name)) {
            Ok(_) => {
                return Err(format!(
                    "cargo-wrapper: plugin {name} is active or has an invalid enabled marker; disable or repair it before uninstalling"
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "cargo-wrapper: cannot inspect enabled marker for plugin {name}: {error}"
                ));
            }
        }

        let expected_executable = OsStr::new(plugin_executable_name());
        let mut removable = Vec::new();
        for entry in fs::read_dir(&directory).map_err(|error| {
            format!(
                "cargo-wrapper: cannot read plugin directory {}: {error}",
                directory.display()
            )
        })? {
            let entry = entry.map_err(|error| {
                format!(
                    "cargo-wrapper: cannot inspect plugin directory {}: {error}",
                    directory.display()
                )
            })?;
            let file_name = entry.file_name();
            let known_temporary = file_name
                .to_str()
                .is_some_and(|name| name.starts_with("plugin.tmp-"));
            if file_name != expected_executable && !known_temporary {
                return Err(format!(
                    "cargo-wrapper: refusing to uninstall plugin {name}; unexpected entry {}",
                    entry.path().display()
                ));
            }
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                format!(
                    "cargo-wrapper: cannot inspect plugin entry {}: {error}",
                    entry.path().display()
                )
            })?;
            if metadata.file_type().is_dir() {
                return Err(format!(
                    "cargo-wrapper: refusing to uninstall plugin {name}; expected a file at {}",
                    entry.path().display()
                ));
            }
            removable.push(entry.path());
        }
        for path in removable {
            fs::remove_file(&path).map_err(|error| {
                format!(
                    "cargo-wrapper: cannot remove plugin file {}: {error}",
                    path.display()
                )
            })?;
        }
        fs::remove_dir(&directory).map_err(|error| {
            format!(
                "cargo-wrapper: cannot remove plugin directory {}: {error}",
                directory.display()
            )
        })?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<PluginInfo>, String> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(format!(
                    "cargo-wrapper: cannot read plugin directory {}: {error}",
                    self.root.display()
                ));
            }
        };
        let mut plugins = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "cargo-wrapper: cannot inspect plugin directory {}: {error}",
                    self.root.display()
                )
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let executable = entry.path().join(plugin_executable_name());
            let valid_directory = fs::symlink_metadata(entry.path())
                .is_ok_and(|metadata| metadata.file_type().is_dir());
            let valid_name = validate_name(&name).is_ok();
            let executable_valid = valid_directory && validate_executable(&executable).is_ok();
            let marker = entry.path().join("enabled");
            let (marker_state, marker_requires_fail_closed) = match fs::symlink_metadata(marker) {
                Ok(metadata) if metadata.file_type().is_file() => (Some(true), true),
                Ok(_) => (Some(false), true),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ) =>
                {
                    (None, false)
                }
                Err(_) => (Some(false), true),
            };
            let status = match (valid_name, executable_valid, marker_state) {
                (true, true, None) => PluginStatus::Inactive,
                (true, true, Some(true)) => PluginStatus::Active,
                _ => PluginStatus::Broken,
            };
            plugins.push(PluginInfo {
                name,
                status,
                executable,
                marker_requires_fail_closed,
            });
        }
        plugins.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(plugins)
    }

    pub fn begin_chain(
        &self,
        real_cargo: &Path,
        original_subcommand: Option<&str>,
    ) -> Result<ChainContext, String> {
        let chain = self.enabled_names()?;
        let root = if self.root.exists() {
            self.root.canonicalize().map_err(|error| {
                format!(
                    "cargo-wrapper: cannot resolve plugin root {}: {error}",
                    self.root.display()
                )
            })?
        } else {
            self.root.clone()
        };
        let real_cargo = absolute_path(real_cargo.to_path_buf())?;
        validate_real_cargo(&real_cargo)?;
        Ok(ChainContext {
            chain,
            index: 0,
            root,
            // Preserve the `cargo` filename: rustup is a multicall executable and
            // chooses Cargo behavior from argv[0]. Canonicalizing a Cargo symlink
            // to `rustup` would change the program it runs.
            real_cargo,
            original_subcommand: original_subcommand.map(ToOwned::to_owned),
            session: format!("{}-{}", std::process::id(), unique_nonce()),
        })
    }

    pub fn from_context(context: &ChainContext) -> Self {
        Self {
            root: context.root.clone(),
        }
    }

    pub fn dispatch<F>(
        &self,
        context: &ChainContext,
        args: &[String],
        run_real_cargo: F,
    ) -> Result<i32, String>
    where
        F: FnOnce(&Path, &[String]) -> Result<i32, String>,
    {
        if context.index == context.chain.len() {
            return run_real_cargo(&context.real_cargo, args);
        }
        let name = context.chain.get(context.index).ok_or_else(|| {
            "cargo-wrapper: plugin continuation index exceeds chain length".to_owned()
        })?;
        let executable = self.resolve_active(name)?;
        let wrapper = env::current_exe().map_err(|error| {
            format!("cargo-wrapper: cannot resolve its executable for plugin chaining: {error}")
        })?;
        let mut command = Command::new(&executable);
        command
            .args(args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .env(ENV_PROTOCOL, PROTOCOL_VERSION)
            .env(ENV_PLUGIN_NAME, name)
            .env(ENV_REAL_CARGO, &context.real_cargo)
            .env(ENV_NEXT, &wrapper)
            .env(ENV_CHAIN, context.chain.join(","))
            .env(ENV_INDEX, (context.index + 1).to_string())
            .env(ENV_ROOT, &context.root)
            .env(ENV_SESSION, &context.session);
        if let Some(original) = &context.original_subcommand {
            command.env(ENV_ORIGINAL_SUBCOMMAND, original);
        } else {
            command.env_remove(ENV_ORIGINAL_SUBCOMMAND);
        }
        let status = command.status().map_err(|error| {
            format!(
                "cargo-wrapper: cannot launch active plugin {name} at {}: {error}",
                executable.display()
            )
        })?;
        Ok(status.code().unwrap_or(1))
    }

    fn enabled_names(&self) -> Result<Vec<String>, String> {
        let plugins = self.list()?;
        let mut enabled = Vec::new();
        for plugin in plugins {
            match plugin.status {
                PluginStatus::Active => enabled.push(plugin.name),
                PluginStatus::Broken => {
                    if plugin.marker_requires_fail_closed {
                        return Err(format!(
                            "cargo-wrapper: enabled plugin {} is broken; run `cargo wrapper plugin list`, then disable or repair it",
                            plugin.name
                        ));
                    }
                }
                PluginStatus::Inactive => {}
            }
        }
        Ok(enabled)
    }

    fn resolve_active(&self, name: &str) -> Result<PathBuf, String> {
        validate_name(name)?;
        let directory = self.plugin_directory(name);
        let directory_metadata = fs::symlink_metadata(&directory).map_err(|error| {
            format!("cargo-wrapper: plugin {name} became unavailable during dispatch: {error}")
        })?;
        if !directory_metadata.file_type().is_dir() {
            return Err(format!(
                "cargo-wrapper: plugin {name} directory became invalid during dispatch"
            ));
        }
        let marker = self.enabled_marker(name);
        let marker_metadata = fs::symlink_metadata(&marker).map_err(|error| {
            format!("cargo-wrapper: plugin {name} became unavailable during dispatch: {error}")
        })?;
        if !marker_metadata.file_type().is_file() {
            return Err(format!(
                "cargo-wrapper: plugin {name} has an invalid enabled marker"
            ));
        }
        let executable = self.plugin_executable(name);
        validate_executable(&executable)?;
        Ok(executable)
    }

    fn plugin_directory(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn plugin_executable(&self, name: &str) -> PathBuf {
        self.plugin_directory(name).join(plugin_executable_name())
    }

    fn enabled_marker(&self, name: &str) -> PathBuf {
        self.plugin_directory(name).join("enabled")
    }
}

#[cfg(windows)]
fn windows_default_root(local_app_data: Option<PathBuf>) -> Result<PathBuf, String> {
    let base = local_app_data.ok_or_else(|| {
        "cargo-wrapper: LOCALAPPDATA is undefined; set CARGO_WRAPPER_PLUGIN_DIR".to_owned()
    })?;
    Ok(base.join("CargoWrapper").join("plugins"))
}

#[cfg(not(windows))]
fn unix_default_root(
    xdg_data_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Result<PathBuf, String> {
    if let Some(path) = xdg_data_home {
        return Ok(path.join("cargowrapper").join("plugins"));
    }
    let home = home.ok_or_else(|| {
        "cargo-wrapper: HOME is undefined; set CARGO_WRAPPER_PLUGIN_DIR".to_owned()
    })?;
    Ok(home
        .join(".local")
        .join("share")
        .join("cargowrapper")
        .join("plugins"))
}

pub fn clear_protocol_environment(command: &mut Command) {
    for variable in RESERVED_ENVIRONMENT {
        command.env_remove(variable);
    }
}

pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("cargo-wrapper: plugin name must contain 1 to 64 ASCII characters".to_owned());
    }
    if matches!(name, "plugin" | "wrapper") {
        return Err(format!("cargo-wrapper: plugin name `{name}` is reserved"));
    }
    let mut characters = name.bytes();
    let first = characters.next().expect("non-empty checked above");
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(format!(
            "cargo-wrapper: invalid plugin name `{name}`; use lowercase ASCII letters, digits, and internal hyphens"
        ));
    }
    if !characters.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-') {
        return Err(format!(
            "cargo-wrapper: invalid plugin name `{name}`; use lowercase ASCII letters, digits, and internal hyphens"
        ));
    }
    Ok(())
}

fn parse_chain(value: &str) -> Result<Vec<String>, String> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for name in value.split(',') {
        validate_name(name)?;
        if names.iter().any(|known| known == name) {
            return Err(format!(
                "cargo-wrapper: duplicate plugin `{name}` in continuation chain"
            ));
        }
        names.push(name.to_owned());
    }
    if !names.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err("cargo-wrapper: plugin continuation chain is not sorted".to_owned());
    }
    Ok(names)
}

fn required_unicode_environment(name: &str) -> Result<String, String> {
    env::var_os(name)
        .ok_or_else(|| format!("cargo-wrapper: missing plugin continuation variable {name}"))?
        .into_string()
        .map_err(|_| format!("cargo-wrapper: plugin continuation variable {name} is not Unicode"))
}

fn required_absolute_path(name: &str) -> Result<PathBuf, String> {
    let path =
        PathBuf::from(env::var_os(name).ok_or_else(|| {
            format!("cargo-wrapper: missing plugin continuation variable {name}")
        })?);
    if !path.is_absolute() {
        return Err(format!(
            "cargo-wrapper: plugin continuation variable {name} must be an absolute path"
        ));
    }
    Ok(path)
}

fn absolute_path(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path)
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|error| format!("cargo-wrapper: cannot resolve current directory: {error}"))
    }
}

fn plugin_executable_name() -> &'static str {
    if cfg!(windows) {
        "plugin.exe"
    } else {
        "plugin"
    }
}

fn validate_executable(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "cargo-wrapper: plugin executable is unavailable at {}: {error}",
            path.display()
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "cargo-wrapper: plugin executable is not a regular file: {}",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!(
                "cargo-wrapper: plugin executable lacks execute permission: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn validate_real_cargo(path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| {
        format!(
            "cargo-wrapper: real Cargo is unavailable at {}: {error}",
            path.display()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "cargo-wrapper: real Cargo is not a regular file: {}",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!(
                "cargo-wrapper: real Cargo lacks execute permission: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn ensure_private_root(root: &Path) -> Result<(), String> {
    if root.exists() {
        let metadata = fs::symlink_metadata(root).map_err(|error| {
            format!(
                "cargo-wrapper: cannot inspect plugin root {}: {error}",
                root.display()
            )
        })?;
        if !metadata.file_type().is_dir() {
            return Err(format!(
                "cargo-wrapper: plugin root is not a directory: {}",
                root.display()
            ));
        }
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(root).map_err(|error| {
        format!(
            "cargo-wrapper: cannot create plugin root {}: {error}",
            root.display()
        )
    })
}

fn create_private_directory(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|error| {
        format!(
            "cargo-wrapper: cannot set permissions on {}: {error}",
            path.display()
        )
    })
}

fn unique_nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let path = env::temp_dir().join(format!(
                "cargowrapper-plugin-{label}-{}-{}",
                std::process::id(),
                unique_nonce()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn executable_fixture(directory: &Path) -> PathBuf {
        let path = directory.join(if cfg!(windows) {
            "fixture.exe"
        } else {
            "fixture"
        });
        fs::write(&path, b"fixture").unwrap();
        #[cfg(unix)]
        set_mode(&path, 0o700).unwrap();
        path
    }

    #[test]
    fn plugin_names_are_strict_and_path_safe() {
        for valid in ["a", "dependency-audit", "plugin2"] {
            assert!(validate_name(valid).is_ok(), "{valid}");
        }
        assert!(validate_name(&"a".repeat(64)).is_ok());
        for invalid in [
            "",
            "-starts-with-hyphen",
            "UPPER",
            "has space",
            "../escape",
            "with/slash",
            "with\\slash",
            "comma,name",
            "plugin",
            "wrapper",
            "é",
        ] {
            assert!(validate_name(invalid).is_err(), "{invalid}");
        }
        assert!(validate_name(&"a".repeat(65)).is_err());
    }

    #[cfg(not(windows))]
    #[test]
    fn resolves_xdg_and_unix_home_plugin_roots() {
        assert_eq!(
            unix_default_root(
                Some(PathBuf::from("/data")),
                Some(PathBuf::from("/home/me"))
            )
            .unwrap(),
            PathBuf::from("/data/cargowrapper/plugins")
        );
        assert_eq!(
            unix_default_root(None, Some(PathBuf::from("/home/me"))).unwrap(),
            PathBuf::from("/home/me/.local/share/cargowrapper/plugins")
        );
        assert!(unix_default_root(None, None).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn resolves_windows_plugin_root() {
        assert_eq!(
            windows_default_root(Some(PathBuf::from(r"C:\Users\me\AppData\Local"))).unwrap(),
            PathBuf::from(r"C:\Users\me\AppData\Local\CargoWrapper\plugins")
        );
        assert!(windows_default_root(None).is_err());
    }

    #[test]
    fn install_is_inactive_and_lifecycle_is_idempotent() {
        let directory = TestDirectory::new("lifecycle");
        let source = executable_fixture(&directory.0);
        let host = PluginHost::new(directory.0.join("root")).unwrap();
        let installed = host.install("dependency-audit", &source).unwrap();
        assert!(installed.is_file());
        assert_eq!(
            host.list().unwrap(),
            vec![PluginInfo {
                name: "dependency-audit".into(),
                status: PluginStatus::Inactive,
                executable: installed.clone(),
                marker_requires_fail_closed: false,
            }]
        );
        assert!(host.install("dependency-audit", &source).is_err());
        assert!(host.enable("dependency-audit").unwrap());
        assert!(!host.enable("dependency-audit").unwrap());
        assert_eq!(host.list().unwrap()[0].status, PluginStatus::Active);
        assert!(host.uninstall("dependency-audit").is_err());
        assert!(host.disable("dependency-audit").unwrap());
        assert!(!host.disable("dependency-audit").unwrap());
        host.uninstall("dependency-audit").unwrap();
        assert!(host.list().unwrap().is_empty());
    }

    #[test]
    fn broken_active_plugins_fail_closed_but_inactive_entries_do_not() {
        let directory = TestDirectory::new("broken");
        let root = directory.0.join("root");
        let host = PluginHost::new(root.clone()).unwrap();
        fs::create_dir_all(root.join("broken")).unwrap();
        fs::write(root.join("broken/enabled"), b"").unwrap();
        assert_eq!(host.list().unwrap()[0].status, PluginStatus::Broken);
        assert!(host.enabled_names().is_err());
        fs::remove_file(root.join("broken/enabled")).unwrap();
        assert!(host.enabled_names().unwrap().is_empty());
    }

    #[test]
    fn uninstall_refuses_unexpected_entries() {
        let directory = TestDirectory::new("uninstall");
        let source = executable_fixture(&directory.0);
        let host = PluginHost::new(directory.0.join("root")).unwrap();
        host.install("demo", &source).unwrap();
        fs::write(host.plugin_directory("demo").join("user-data"), b"keep").unwrap();
        assert!(host.uninstall("demo").is_err());
        assert!(host.plugin_executable("demo").exists());
    }

    #[test]
    fn list_and_enabled_chain_are_sorted_and_report_every_status() {
        let directory = TestDirectory::new("list");
        let source = executable_fixture(&directory.0);
        let root = directory.0.join("root");
        let host = PluginHost::new(root.clone()).unwrap();
        host.install("zeta", &source).unwrap();
        host.install("alpha", &source).unwrap();
        host.enable("zeta").unwrap();
        fs::create_dir(root.join("middle")).unwrap();
        fs::write(root.join("middle/enabled"), b"").unwrap();

        let listed = host.list().unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|plugin| (plugin.name.as_str(), plugin.status))
                .collect::<Vec<_>>(),
            [
                ("alpha", PluginStatus::Inactive),
                ("middle", PluginStatus::Broken),
                ("zeta", PluginStatus::Active),
            ]
        );
        assert!(host.enabled_names().is_err());

        fs::remove_file(root.join("middle/enabled")).unwrap();
        assert_eq!(host.enabled_names().unwrap(), ["zeta"]);
    }

    #[test]
    fn chain_parser_requires_unique_sorted_safe_names() {
        assert_eq!(
            parse_chain("alpha,beta").unwrap(),
            vec!["alpha".to_owned(), "beta".to_owned()]
        );
        assert!(parse_chain("beta,alpha").is_err());
        assert!(parse_chain("alpha,alpha").is_err());
        assert!(parse_chain("alpha,../escape").is_err());
    }
}
