use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use vexa_boot::{
    BootError, CORE_EXECUTABLE_NAME, CURRENT_MANIFEST_SCHEMA_VERSION, CurrentManifest,
    activate_version, load_current_manifest, resolve_current_core, resolve_install_root,
    rollback_version, validate_manifest, validate_version,
};

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

fn write_manifest(root: &Path, json: &str) {
    fs::write(root.join("current.json"), json)
        .unwrap_or_else(|error| panic!("failed to write manifest fixture: {error}"));
}

#[test]
fn manifest_schema_one_and_canonical_semver_are_accepted() {
    let root = TestDir::new("manifest-valid");
    write_manifest(
        root.path(),
        r#"{"schema_version":1,"version":"1.2.3-rc.1+build.7"}"#,
    );
    let manifest = load_current_manifest(root.path())
        .unwrap_or_else(|error| panic!("valid manifest rejected: {error}"));
    assert_eq!(manifest.schema_version, CURRENT_MANIFEST_SCHEMA_VERSION);
    assert_eq!(manifest.version, "1.2.3-rc.1+build.7");
}

#[test]
fn manifest_rejects_unknown_fields_and_schema_versions() {
    let root = TestDir::new("manifest-schema");
    write_manifest(
        root.path(),
        r#"{"schema_version":1,"version":"1.2.3","extra":true}"#,
    );
    assert!(matches!(
        load_current_manifest(root.path()),
        Err(BootError::ManifestJson { .. })
    ));

    write_manifest(root.path(), r#"{"schema_version":2,"version":"1.2.3"}"#);
    assert!(matches!(
        load_current_manifest(root.path()),
        Err(BootError::UnsupportedManifestSchema {
            expected: 1,
            actual: 2
        })
    ));
}

#[test]
fn version_validation_rejects_traversal_and_noncanonical_values() {
    for version in [
        "",
        ".",
        "..",
        "../1.2.3",
        "1.2.3/../../evil",
        r"1.2.3\..\evil",
        "/absolute",
        r"C:\absolute",
        "v1.2.3",
        "01.2.3",
        "1.2",
        "1.2.3 ",
        "1.2.3/",
        "１.２.３",
    ] {
        assert!(
            matches!(
                validate_version(version),
                Err(BootError::InvalidVersion { .. })
            ),
            "version unexpectedly accepted: {version:?}"
        );
    }

    for version in ["0.1.0", "1.2.3", "1.2.3-alpha.1", "1.2.3+build.9"] {
        validate_version(version)
            .unwrap_or_else(|error| panic!("valid version {version:?} rejected: {error}"));
    }
}

#[test]
fn direct_manifest_validation_checks_schema_before_version() {
    let manifest = CurrentManifest {
        schema_version: 99,
        version: "../evil".to_owned(),
    };
    assert!(matches!(
        validate_manifest(&manifest),
        Err(BootError::UnsupportedManifestSchema { actual: 99, .. })
    ));
}

#[test]
fn install_root_override_wins_and_empty_override_is_rejected() {
    let boot = Path::new("install").join("vexa-boot");
    assert_eq!(
        resolve_install_root(Some(OsStr::new("fixture-root")), &boot)
            .unwrap_or_else(|error| panic!("override rejected: {error}")),
        PathBuf::from("fixture-root")
    );
    assert_eq!(
        resolve_install_root(None, &boot)
            .unwrap_or_else(|error| panic!("default root rejected: {error}")),
        PathBuf::from("install")
    );
    assert!(matches!(
        resolve_install_root(Some(OsStr::new("")), &boot),
        Err(BootError::EmptyInstallRootOverride)
    ));
}

#[test]
fn manifest_size_limit_is_enforced() {
    let root = TestDir::new("manifest-large");
    let large = format!(
        r#"{{"schema_version":1,"version":"1.2.3","padding":"{}"}}"#,
        "x".repeat(5000)
    );
    write_manifest(root.path(), &large);
    assert!(matches!(
        load_current_manifest(root.path()),
        Err(BootError::ManifestTooLarge { .. })
    ));
}

#[test]
fn core_resolution_is_exactly_versions_version_core_name() {
    let root = TestDir::new("core-path");
    write_manifest(root.path(), r#"{"schema_version":1,"version":"1.2.3"}"#);
    let expected = root
        .path()
        .join("versions")
        .join("1.2.3")
        .join(CORE_EXECUTABLE_NAME);
    fs::create_dir_all(
        expected
            .parent()
            .unwrap_or_else(|| panic!("fixture core must have parent")),
    )
    .unwrap_or_else(|error| panic!("failed to create fixture version directory: {error}"));
    fs::write(&expected, b"fixture")
        .unwrap_or_else(|error| panic!("failed to create fixture core: {error}"));

    let (_, resolved) = resolve_current_core(root.path())
        .unwrap_or_else(|error| panic!("core resolution failed: {error}"));
    assert_eq!(resolved, expected);
}

#[test]
fn missing_core_is_reported_without_fallback() {
    let root = TestDir::new("core-missing");
    write_manifest(root.path(), r#"{"schema_version":1,"version":"9.8.7"}"#);
    let error = resolve_current_core(root.path())
        .err()
        .unwrap_or_else(|| panic!("missing core should fail"));
    match error {
        BootError::CoreNotFound(path) => assert_eq!(
            path,
            root.path()
                .join("versions")
                .join("9.8.7")
                .join(CORE_EXECUTABLE_NAME)
        ),
        other => panic!("unexpected missing-core error: {other}"),
    }
}

#[test]
fn activation_switches_current_atomically_and_rollback_swaps_versions() {
    let root = TestDir::new("activation-rollback");
    for version in ["1.0.0", "2.0.0"] {
        fs::create_dir_all(root.path().join("versions").join(version))
            .unwrap_or_else(|error| panic!("failed to create version fixture: {error}"));
    }
    fs::write(
        root.path().join("current.json"),
        r#"{"schema_version":1,"version":"1.0.0"}"#,
    )
    .unwrap_or_else(|error| panic!("failed to seed current manifest: {error}"));
    activate_version(root.path(), "2.0.0")
        .unwrap_or_else(|error| panic!("activation failed: {error}"));
    assert_eq!(
        load_current_manifest(root.path())
            .unwrap_or_else(|error| panic!("current manifest load failed: {error}"))
            .version,
        "2.0.0"
    );
    rollback_version(root.path()).unwrap_or_else(|error| panic!("rollback failed: {error}"));
    assert_eq!(
        load_current_manifest(root.path())
            .unwrap_or_else(|error| panic!("current manifest load failed: {error}"))
            .version,
        "1.0.0"
    );
    let previous = fs::read_to_string(root.path().join("previous.json"))
        .unwrap_or_else(|error| panic!("failed to read rollback manifest: {error}"));
    assert!(previous.contains("2.0.0"));
}

#[test]
fn failed_activation_does_not_replace_current_manifest() {
    let root = TestDir::new("activation-missing");
    fs::create_dir_all(root.path().join("versions").join("1.0.0"))
        .unwrap_or_else(|error| panic!("failed to create version fixture: {error}"));
    write_manifest(root.path(), r#"{"schema_version":1,"version":"1.0.0"}"#);
    assert!(matches!(
        activate_version(root.path(), "2.0.0"),
        Err(BootError::VersionNotInstalled(_))
    ));
    assert_eq!(
        load_current_manifest(root.path())
            .unwrap_or_else(|error| panic!("current manifest load failed: {error}"))
            .version,
        "1.0.0"
    );
}
