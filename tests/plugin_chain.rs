//! Exercises the public plugin commands and middleware protocol as real subprocesses.
//! A tiny Rust fixture acts as both native plugins and Cargo without running the real tool.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "cargo wrapper plugin chain-{}-{nonce}",
            std::process::id()
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

fn executable_name(name: &str) -> String {
    format!("{name}{}", env::consts::EXE_SUFFIX)
}

fn run_wrapper(wrapper: &Path, plugin_root: &Path, cargo_home: &Path, args: &[&str]) -> Output {
    Command::new(wrapper)
        .args(args)
        .env("CARGO_WRAPPER_PLUGIN_DIR", plugin_root)
        .env("CARGO_HOME", cargo_home)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn compile_fixture(directory: &Path) -> PathBuf {
    let source = directory.join("fixture.rs");
    let executable = directory.join(executable_name("fixture"));
    fs::write(
        &source,
        r#"
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::{Command, exit};

const PROTOCOL_ENV: &[&str] = &[
    "CARGO_WRAPPER_PLUGIN_PROTOCOL",
    "CARGO_WRAPPER_PLUGIN_NAME",
    "CARGO_WRAPPER_REAL_CARGO",
    "CARGO_WRAPPER_NEXT",
    "CARGO_WRAPPER_ORIGINAL_SUBCOMMAND",
    "CARGO_WRAPPER_PLUGIN_CHAIN",
    "CARGO_WRAPPER_PLUGIN_INDEX",
    "CARGO_WRAPPER_PLUGIN_ROOT",
    "CARGO_WRAPPER_PLUGIN_SESSION",
];

fn record(line: &str) {
    let trace = env::var_os("FIXTURE_TRACE").expect("FIXTURE_TRACE");
    let mut file = OpenOptions::new().create(true).append(true).open(trace).unwrap();
    writeln!(file, "{line}").unwrap();
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let executable = env::current_exe().unwrap();
    let stem = executable.file_stem().and_then(|value| value.to_str()).unwrap_or("");
    if stem == "cargo" {
        let leaked = PROTOCOL_ENV.iter().any(|name| env::var_os(name).is_some());
        record(&format!("cargo|{}|protocol-leaked={leaked}", args.join(" ")));
        return;
    }

    assert_eq!(env::var("CARGO_WRAPPER_PLUGIN_PROTOCOL").unwrap(), "1");
    let name = env::var("CARGO_WRAPPER_PLUGIN_NAME").unwrap();
    let original = env::var("CARGO_WRAPPER_ORIGINAL_SUBCOMMAND").unwrap_or_default();
    record(&format!("plugin:{name}|{}|original={original}", args.join(" ")));
    if env::var("FIXTURE_REJECT_PLUGIN").ok().as_deref() == Some(&name) {
        exit(42);
    }

    let next = env::var_os("CARGO_WRAPPER_NEXT").unwrap();
    let mut next_args = args;
    if name == "alpha" && env::var_os("FIXTURE_REWRITE_TO_BUILD").is_some() && !next_args.is_empty() {
        next_args[0] = "build".to_owned();
    }
    next_args.push(format!("--seen-by-{name}"));
    let status = Command::new(Path::new(&next)).args(next_args).status().unwrap();
    exit(status.code().unwrap_or(1));
}
"#,
    )
    .unwrap();

    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(rustc)
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert_success(&output);
    executable
}

#[test]
fn plugins_wrap_cargo_in_order_and_management_bypasses_the_chain() {
    let directory = TestDirectory::new();
    let plugin_root = directory.0.join("plugins");
    let cargo_home = directory.0.join("cargo-home");
    let cargo_bin = cargo_home.join("bin");
    fs::create_dir_all(&cargo_bin).unwrap();
    let fixture = compile_fixture(&directory.0);
    fs::copy(&fixture, cargo_bin.join(executable_name("cargo"))).unwrap();

    let wrapper = PathBuf::from(env!("CARGO_BIN_EXE_cargowrapper"));

    let trace = directory.0.join("trace");
    let no_plugins = Command::new(&wrapper)
        .args(["build", "--", "--locked"])
        .env("CARGO_WRAPPER_PLUGIN_DIR", &plugin_root)
        .env("CARGO_HOME", &cargo_home)
        .env("FIXTURE_TRACE", &trace)
        .output()
        .unwrap();
    assert_success(&no_plugins);
    assert_eq!(
        fs::read_to_string(&trace).unwrap(),
        "cargo|build --locked -- --locked|protocol-leaked=false\n"
    );

    for name in ["beta", "alpha"] {
        let output = run_wrapper(
            &wrapper,
            &plugin_root,
            &cargo_home,
            &[
                "wrapper",
                "plugin",
                "install",
                name,
                fixture.to_str().unwrap(),
            ],
        );
        assert_success(&output);
        assert!(String::from_utf8_lossy(&output.stdout).contains("inactive"));
    }

    let list = run_wrapper(
        &wrapper,
        &plugin_root,
        &cargo_home,
        &["wrapper", "plugin", "list"],
    );
    assert_success(&list);
    let list = String::from_utf8_lossy(&list.stdout);
    assert!(list.find("alpha\tinactive").unwrap() < list.find("beta\tinactive").unwrap());

    fs::write(&trace, "").unwrap();
    let disabled = Command::new(&wrapper)
        .arg("metadata")
        .env("CARGO_WRAPPER_PLUGIN_DIR", &plugin_root)
        .env("CARGO_HOME", &cargo_home)
        .env("FIXTURE_TRACE", &trace)
        .output()
        .unwrap();
    assert_success(&disabled);
    assert_eq!(
        fs::read_to_string(&trace).unwrap(),
        "cargo|metadata|protocol-leaked=false\n"
    );

    for name in ["beta", "alpha"] {
        assert_success(&run_wrapper(
            &wrapper,
            &plugin_root,
            &cargo_home,
            &["wrapper", "plugin", "enable", name],
        ));
    }

    fs::write(&trace, "").unwrap();
    let output = Command::new(&wrapper)
        .args(["forceupdate", "demo"])
        .env("CARGO_WRAPPER_PLUGIN_DIR", &plugin_root)
        .env("CARGO_HOME", &cargo_home)
        .env("FIXTURE_TRACE", &trace)
        .output()
        .unwrap();
    assert_success(&output);
    let lines: Vec<_> = fs::read_to_string(&trace)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        lines,
        [
            "plugin:alpha|update demo|original=forceupdate",
            "plugin:beta|update demo --seen-by-alpha|original=forceupdate",
            "cargo|update demo --seen-by-alpha --seen-by-beta|protocol-leaked=false",
        ]
    );

    fs::write(&trace, "").unwrap();
    let toolchain = Command::new(&wrapper)
        .args(["+nightly", "build"])
        .env("CARGO_WRAPPER_PLUGIN_DIR", &plugin_root)
        .env("CARGO_HOME", &cargo_home)
        .env("FIXTURE_TRACE", &trace)
        .output()
        .unwrap();
    assert_success(&toolchain);
    let toolchain_trace = fs::read_to_string(&trace).unwrap();
    assert!(toolchain_trace.contains("plugin:alpha|+nightly build --locked|original=build"));
    assert!(toolchain_trace.contains(
        "cargo|+nightly build --locked --seen-by-alpha --seen-by-beta|protocol-leaked=false"
    ));

    fs::write(&trace, "").unwrap();
    let rewritten = Command::new(&wrapper)
        .arg("metadata")
        .env("CARGO_WRAPPER_PLUGIN_DIR", &plugin_root)
        .env("CARGO_HOME", &cargo_home)
        .env("FIXTURE_TRACE", &trace)
        .env("FIXTURE_REWRITE_TO_BUILD", "1")
        .output()
        .unwrap();
    assert_success(&rewritten);
    let rewritten_trace = fs::read_to_string(&trace).unwrap();
    assert!(rewritten_trace.contains("plugin:beta|build --seen-by-alpha|original=metadata"));
    assert!(
        rewritten_trace
            .contains("cargo|build --seen-by-alpha --seen-by-beta|protocol-leaked=false")
    );
    assert!(!rewritten_trace.contains("build --locked"));

    fs::write(&trace, "").unwrap();
    let rejected = Command::new(&wrapper)
        .args(["forceupdate", "demo"])
        .env("CARGO_WRAPPER_PLUGIN_DIR", &plugin_root)
        .env("CARGO_HOME", &cargo_home)
        .env("FIXTURE_TRACE", &trace)
        .env("FIXTURE_REJECT_PLUGIN", "alpha")
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(42));
    assert_eq!(
        fs::read_to_string(&trace).unwrap(),
        "plugin:alpha|update demo|original=forceupdate\n"
    );

    fs::remove_file(plugin_root.join("alpha").join(executable_name("plugin"))).unwrap();
    let ordinary = run_wrapper(&wrapper, &plugin_root, &cargo_home, &["metadata"]);
    assert!(!ordinary.status.success());
    assert!(String::from_utf8_lossy(&ordinary.stderr).contains("enabled plugin alpha is broken"));

    let broken_list = run_wrapper(
        &wrapper,
        &plugin_root,
        &cargo_home,
        &["wrapper", "plugin", "list"],
    );
    assert_success(&broken_list);
    assert!(String::from_utf8_lossy(&broken_list.stdout).contains("alpha\tbroken"));
    assert_success(&run_wrapper(
        &wrapper,
        &plugin_root,
        &cargo_home,
        &["wrapper", "plugin", "disable", "alpha"],
    ));
    assert_success(&run_wrapper(
        &wrapper,
        &plugin_root,
        &cargo_home,
        &["wrapper", "plugin", "uninstall", "alpha"],
    ));
}
