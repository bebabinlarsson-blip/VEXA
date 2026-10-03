//! Stable launcher for an installed VEXA release.
//!
//! BOOT intentionally has no update or network behavior. It reads `current.json` from the install
//! root, validates the selected version, and starts that version's `vexa-core` with inherited
//! standard streams and the launcher's arguments.

use semver::Version;
use serde::{Deserialize, Serialize};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::Ordering;
use thiserror::Error;

pub const CURRENT_MANIFEST_FILE: &str = "current.json";
pub const CURRENT_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const INSTALL_ROOT_ENV: &str = "VEXA_INSTALL_ROOT";
pub const MAX_MANIFEST_BYTES: u64 = 4 * 1024;
pub const MAX_VERSION_BYTES: usize = 128;

#[cfg(windows)]
pub const CORE_EXECUTABLE_NAME: &str = "vexa-core.exe";
#[cfg(not(windows))]
pub const CORE_EXECUTABLE_NAME: &str = "vexa-core";

#[derive(Debug, Error)]
pub enum BootError {
    #[error("{INSTALL_ROOT_ENV} is set to an empty path")]
    EmptyInstallRootOverride,
    #[error("cannot determine the directory containing the BOOT executable: {0}")]
    ExecutableHasNoParent(PathBuf),
    #[error("failed to determine the BOOT executable path: {0}")]
    CurrentExecutable(#[source] io::Error),
    #[error("failed to inspect current manifest {path}: {source}")]
    ManifestMetadata {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("current manifest {path} is too large ({size} bytes; maximum {max} bytes)")]
    ManifestTooLarge { path: PathBuf, size: u64, max: u64 },
    #[error("current or previous manifest is not a regular file: {0}")]
    ManifestNotFile(PathBuf),
    #[error("failed to read current manifest {path}: {source}")]
    ManifestRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid current manifest {path}: {source}")]
    ManifestJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "unsupported current manifest schema version {actual}; this BOOT supports schema version {expected}"
    )]
    UnsupportedManifestSchema { expected: u32, actual: u32 },
    #[error("invalid installed version {version:?}: {reason}")]
    InvalidVersion { version: String, reason: String },
    #[error("selected VEXA core does not exist: {0}")]
    CoreNotFound(PathBuf),
    #[error("selected VEXA core is not a regular file: {0}")]
    CoreNotFile(PathBuf),
    #[error("failed to inspect selected VEXA core {path}: {source}")]
    CoreMetadata {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to start selected VEXA core {path}: {source}")]
    SpawnCore {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("installed version directory does not exist: {0}")]
    VersionNotInstalled(PathBuf),
    #[error("there is no valid previous version to roll back to")]
    NoPreviousVersion,
    #[error("failed to atomically write version manifest {path}: {source}")]
    ManifestWrite {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub type Result<T> = std::result::Result<T, BootError>;

/// The complete `current.json` schema supported by this BOOT generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentManifest {
    pub schema_version: u32,
    pub version: String,
}

/// Resolve the install root from an optional override and the BOOT executable path.
///
/// The override is intentionally allowed to be relative so development launchers can point at a
/// local fixture tree. In normal installs the root is the directory containing `vexa-boot`.
///
/// # Errors
///
/// Returns an error for an empty override or an executable path without a parent directory.
pub fn resolve_install_root(
    override_root: Option<&OsStr>,
    boot_executable: &Path,
) -> Result<PathBuf> {
    if let Some(override_root) = override_root {
        if override_root.is_empty() {
            return Err(BootError::EmptyInstallRootOverride);
        }
        return Ok(PathBuf::from(override_root));
    }

    boot_executable
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .ok_or_else(|| BootError::ExecutableHasNoParent(boot_executable.to_path_buf()))
}

/// Resolve the install root using `VEXA_INSTALL_ROOT` or the current BOOT executable location.
///
/// # Errors
///
/// Returns an error if the current executable path cannot be determined or the resulting root is
/// invalid.
pub fn install_root_from_environment() -> Result<PathBuf> {
    let boot_executable = env::current_exe().map_err(BootError::CurrentExecutable)?;
    resolve_install_root(env::var_os(INSTALL_ROOT_ENV).as_deref(), &boot_executable)
}

/// Read and validate `install_root/current.json`.
///
/// # Errors
///
/// Returns an error for missing/unreadable manifests, manifests over 4 KiB, malformed JSON,
/// unsupported schema versions, unknown fields, or invalid/noncanonical semantic versions.
pub fn load_current_manifest(install_root: &Path) -> Result<CurrentManifest> {
    let path = install_root.join(CURRENT_MANIFEST_FILE);
    let metadata = fs::symlink_metadata(&path).map_err(|source| BootError::ManifestMetadata {
        path: path.clone(),
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(BootError::ManifestNotFile(path));
    }
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(BootError::ManifestTooLarge {
            path,
            size: metadata.len(),
            max: MAX_MANIFEST_BYTES,
        });
    }

    let bytes = fs::read(&path).map_err(|source| BootError::ManifestRead {
        path: path.clone(),
        source,
    })?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(BootError::ManifestTooLarge {
            path,
            size: bytes.len() as u64,
            max: MAX_MANIFEST_BYTES,
        });
    }

    let manifest: CurrentManifest =
        serde_json::from_slice(&bytes).map_err(|source| BootError::ManifestJson {
            path: path.clone(),
            source,
        })?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

/// Validate a parsed current manifest.
///
/// # Errors
///
/// Returns an error when the schema version is unsupported or the release version is unsafe or
/// not canonical semantic-version syntax.
pub fn validate_manifest(manifest: &CurrentManifest) -> Result<()> {
    if manifest.schema_version != CURRENT_MANIFEST_SCHEMA_VERSION {
        return Err(BootError::UnsupportedManifestSchema {
            expected: CURRENT_MANIFEST_SCHEMA_VERSION,
            actual: manifest.schema_version,
        });
    }
    validate_version(&manifest.version)
}

/// Atomically select an installed version and retain the formerly selected version for rollback.
/// The version directory must already exist. This function does not execute the selected core;
/// callers must run their health check before invoking it.
///
/// # Errors
///
/// Returns an error for invalid versions, a missing version directory, an invalid existing
/// manifest, or filesystem failures while writing the manifests.
pub fn activate_version(install_root: &Path, version: &str) -> Result<CurrentManifest> {
    validate_version(version)?;
    let version_dir = install_root.join("versions").join(version);
    if !fs::symlink_metadata(&version_dir).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        return Err(BootError::VersionNotInstalled(version_dir));
    }
    let current_path = install_root.join(CURRENT_MANIFEST_FILE);
    let previous_path = install_root.join("previous.json");
    let next = CurrentManifest {
        schema_version: CURRENT_MANIFEST_SCHEMA_VERSION,
        version: version.to_owned(),
    };
    let old = match fs::symlink_metadata(&current_path) {
        Ok(_) => {
            let manifest = load_current_manifest(install_root)?;
            let old_dir = install_root.join("versions").join(&manifest.version);
            if !fs::symlink_metadata(&old_dir).is_ok_and(|metadata| metadata.file_type().is_dir()) {
                return Err(BootError::VersionNotInstalled(old_dir));
            }
            Some(manifest)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(BootError::ManifestMetadata {
                path: current_path.clone(),
                source,
            });
        }
    };
    let old_rollback = load_previous_manifest(install_root).ok();

    if let Some(old) = old.as_ref().filter(|old| old.version != version) {
        write_manifest_atomic(&previous_path, old)?;
    }
    if let Err(error) = write_manifest_atomic(&current_path, &next) {
        if let Some(old_rollback) = old_rollback {
            let _ = write_manifest_atomic(&previous_path, &old_rollback);
        } else {
            let _ = fs::remove_file(&previous_path);
        }
        return Err(error);
    }
    Ok(next)
}

/// Atomically switch to the retained previous version, keeping the former current version as
/// the next rollback target.
///
/// # Errors
///
/// Returns an error when the current or previous manifest is invalid, the rollback target is
/// unavailable, or manifest replacement fails.
pub fn rollback_version(install_root: &Path) -> Result<CurrentManifest> {
    let current = load_current_manifest(install_root)?;
    let previous_path = install_root.join("previous.json");
    let previous = load_previous_manifest(install_root)?;
    write_manifest_atomic(&previous_path, &current)?;
    if let Err(error) = write_manifest_atomic(&install_root.join(CURRENT_MANIFEST_FILE), &previous)
    {
        let _ = write_manifest_atomic(&previous_path, &previous);
        return Err(error);
    }
    Ok(previous)
}

/// Read and validate the retained rollback target without changing either manifest.
///
/// # Errors
///
/// Returns [`BootError::NoPreviousVersion`] when the rollback manifest or version directory is
/// missing or invalid.
pub fn load_previous_manifest(install_root: &Path) -> Result<CurrentManifest> {
    let previous_path = install_root.join("previous.json");
    let metadata =
        fs::symlink_metadata(&previous_path).map_err(|_| BootError::NoPreviousVersion)?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_MANIFEST_BYTES {
        return Err(BootError::NoPreviousVersion);
    }
    let bytes = fs::read(&previous_path).map_err(|_| BootError::NoPreviousVersion)?;
    let previous: CurrentManifest =
        serde_json::from_slice(&bytes).map_err(|_| BootError::NoPreviousVersion)?;
    validate_manifest(&previous).map_err(|_| BootError::NoPreviousVersion)?;
    let previous_dir = install_root.join("versions").join(&previous.version);
    if !fs::symlink_metadata(&previous_dir).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        return Err(BootError::NoPreviousVersion);
    }
    Ok(previous)
}

fn write_manifest_atomic(path: &Path, manifest: &CurrentManifest) -> Result<()> {
    let parent = path.parent().ok_or_else(|| BootError::ManifestWrite {
        path: path.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidInput, "manifest path has no parent"),
    })?;
    fs::create_dir_all(parent).map_err(|source| BootError::ManifestWrite {
        path: path.to_path_buf(),
        source,
    })?;
    let temp = parent.join(format!(
        ".current-{}-{}.tmp",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        let mut writer = io::BufWriter::new(file);
        serde_json::to_writer(&mut writer, manifest)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|source| BootError::ManifestWrite {
        path: path.to_path_buf(),
        source,
    })
}

static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Validate an installed version directory name.
///
/// Valid versions are canonical `SemVer` strings such as `1.2.3`, `1.2.3-rc.1`, or
/// `1.2.3+build.7`. Prefixes such as `v1.2.3`, path separators, traversal components, whitespace,
/// and noncanonical forms are rejected.
///
/// # Errors
///
/// Returns [`BootError::InvalidVersion`] for any unsafe or invalid value.
pub fn validate_version(version: &str) -> Result<()> {
    if version.is_empty() {
        return invalid_version(version, "version must not be empty");
    }
    if version.len() > MAX_VERSION_BYTES {
        return invalid_version(version, "version exceeds 128 bytes");
    }
    if !version.is_ascii() {
        return invalid_version(version, "version must contain ASCII characters only");
    }
    if version.contains('/') || version.contains('\\') {
        return invalid_version(version, "path separators are forbidden");
    }
    if version == "." || version == ".." {
        return invalid_version(version, "path traversal components are forbidden");
    }

    let parsed = Version::parse(version).map_err(|error| BootError::InvalidVersion {
        version: version.to_owned(),
        reason: error.to_string(),
    })?;
    if parsed.to_string() != version {
        return invalid_version(
            version,
            "version must use canonical semantic-version spelling",
        );
    }
    Ok(())
}

fn invalid_version<T>(version: &str, reason: &str) -> Result<T> {
    Err(BootError::InvalidVersion {
        version: version.to_owned(),
        reason: reason.to_owned(),
    })
}

/// Resolve the selected `vexa-core` path without executing it.
///
/// # Errors
///
/// Returns an error if the manifest is invalid or the selected core is missing or not a file.
pub fn resolve_current_core(install_root: &Path) -> Result<(CurrentManifest, PathBuf)> {
    let manifest = load_current_manifest(install_root)?;
    let path = install_root
        .join("versions")
        .join(&manifest.version)
        .join(CORE_EXECUTABLE_NAME);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Err(BootError::CoreNotFound(path));
        }
        Err(source) => {
            return Err(BootError::CoreMetadata {
                path: path.clone(),
                source,
            });
        }
    };
    if !metadata.file_type().is_file() {
        return Err(BootError::CoreNotFile(path));
    }
    Ok((manifest, path))
}

/// Start the currently selected core with the supplied arguments and inherited standard streams.
///
/// # Errors
///
/// Returns an error if the selected release cannot be resolved or the child process cannot start.
pub fn launch_current<I, S>(install_root: &Path, args: I) -> Result<i32>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let (_, core_path) = resolve_current_core(install_root)?;
    let status = Command::new(&core_path)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|source| BootError::SpawnCore {
            path: core_path,
            source,
        })?;
    Ok(exit_status_code(status))
}

/// Resolve the install root and launch the selected core with the launcher's arguments.
///
/// # Errors
///
/// Returns an error if root resolution, manifest validation, core resolution, or process startup
/// fails.
pub fn run_from_environment<I, S>(args: I) -> Result<i32>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let install_root = install_root_from_environment()?;
    launch_current(&install_root, args)
}

fn exit_status_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128_i32.saturating_add(signal);
        }
    }

    1
}

/// Return process arguments excluding BOOT's own executable name.
pub fn forwarded_args() -> impl Iterator<Item = OsString> {
    env::args_os().skip(1)
}
