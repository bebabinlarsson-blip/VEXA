use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use vexa_boot::CORE_EXECUTABLE_NAME;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "vexa-boot-{label}-{}-{counter}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap_or_else(|error| {
            panic!(
                "failed to create test directory {}: {error}",
                path.display()
            )
        });
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn compile_core_helper(root: &Path, version: &str) -> PathBuf {
    let version_dir = root.join("versions").join(version);
    fs::create_dir_all(&version_dir)
        .unwrap_or_else(|error| panic!("failed to create version directory: {error}"));
    let source = root.join("core-helper.rs");
    fs::write(
        &source,
        r#"
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("helper requires capture path and exit code");
        process::exit(120);
    }
    let capture = PathBuf::from(&args[0]);
    let code: i32 = match args[1].parse() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("invalid helper exit code: {error}");
            process::exit(121);
        }
    };
    if let Err(error) = fs::write(&capture, args[2..].join("\n")) {
        eprintln!("failed to write capture file: {error}");
        process::exit(122);
    }
    println!("helper-stdout");
    eprintln!("helper-stderr");
    process::exit(code);
}
"#,
    )
    .unwrap_or_else(|error| panic!("failed to write helper source: {error}"));

    let output_path = version_dir.join(CORE_EXECUTABLE_NAME);
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(rustc)
        .arg(&source)
        .arg("-o")
        .arg(&output_path)
        .output()
        .unwrap_or_else(|error| panic!("failed to invoke rustc for helper: {error}"));
    assert!(
        output.status.success(),
        "helper compilation failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output_path
}

#[test]
fn binary_uses_install_root_override_forwards_args_stdio_and_exit_code() {
    let root = TestDir::new("launcher-e2e");
    fs::write(
        root.path().join("current.json"),
        r#"{"schema_version":1,"version":"1.2.3"}"#,
    )
    .unwrap_or_else(|error| panic!("failed to write current manifest: {error}"));
    let _core = compile_core_helper(root.path(), "1.2.3");
    let capture = root.path().join("forwarded-args.txt");

    let output = Command::new(env!("CARGO_BIN_EXE_vexa-boot"))
        .env("VEXA_INSTALL_ROOT", root.path())
        .arg(&capture)
        .arg("37")
        .arg("alpha")
        .arg("two words")
        .arg("--flag=value")
        .output()
        .unwrap_or_else(|error| panic!("failed to run vexa-boot: {error}"));

    assert_eq!(output.status.code(), Some(37));
    assert_eq!(
        fs::read_to_string(&capture)
            .unwrap_or_else(|error| panic!("failed to read captured args: {error}")),
        "alpha\ntwo words\n--flag=value"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("helper-stdout"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("helper-stderr"));
}

#[test]
fn binary_rejects_traversal_manifest_before_starting_any_child() {
    let root = TestDir::new("launcher-traversal");
    fs::write(
        root.path().join("current.json"),
        r#"{"schema_version":1,"version":"../escape"}"#,
    )
    .unwrap_or_else(|error| panic!("failed to write current manifest: {error}"));

    let output = Command::new(env!("CARGO_BIN_EXE_vexa-boot"))
        .env("VEXA_INSTALL_ROOT", root.path())
        .output()
        .unwrap_or_else(|error| panic!("failed to run vexa-boot: {error}"));
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid installed version"));
    assert!(stderr.contains("path separators are forbidden"));
}
