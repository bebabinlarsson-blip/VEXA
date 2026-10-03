//! Safe release discovery and staging for official VEXA binaries.
//!
//! Packages are verified before extraction and health checks. Activation selects a versioned core
//! without replacing the running executable; retained hashes protect explicit rollback.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const OFFICIAL_REPOSITORY: &str = "bebabinlarsson-blip/VEXA";
pub const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/bebabinlarsson-blip/VEXA/releases/latest";
pub const STATE_SCHEMA_VERSION: u32 = 1;

const CHECKSUMS_MAX_BYTES: u64 = 4 * 1024 * 1024;
const METADATA_MAX_BYTES: u64 = 4 * 1024 * 1024;
const ARTIFACT_MAX_BYTES: u64 = 512 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(60);
const USER_AGENT: &str = concat!("vexa-update/", env!("CARGO_PKG_VERSION"));
const LIFECYCLE_LOCK_FILE: &str = "lifecycle.lock";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static PROCESS_LIFECYCLE_LOCKS: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("boot lifecycle operation failed: {0}")]
    Boot(#[from] vexa_boot::BootError),
    #[error("HTTP request failed: {0}")]
    Http(#[from] ureq::Error),
    #[error("I/O operation failed: {0}")]
    Io(#[from] io::Error),
    #[error("JSON data is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported release target: {0}")]
    UnsupportedTarget(String),
    #[error("release version is not valid semantic version data: {0}")]
    InvalidVersion(String),
    #[error("GitHub latest-release response points at a draft release")]
    DraftRelease,
    #[error("release does not contain an artifact for {0}")]
    NoMatchingArtifact(String),
    #[error("release contains duplicate compatible artifact: {0}")]
    DuplicateArtifact(String),
    #[error("release does not contain a SHA256SUMS asset")]
    MissingChecksumsAsset,
    #[error("invalid SHA256SUMS line: {0}")]
    InvalidChecksumLine(String),
    #[error("SHA256SUMS does not contain an entry for {0}")]
    MissingArtifactChecksum(String),
    #[error("unsafe release asset name: {0}")]
    UnsafeAssetName(String),
    #[error("artifact {name} is too large ({size} bytes; maximum {max} bytes)")]
    ArtifactTooLarge { name: String, size: u64, max: u64 },
    #[error("artifact size mismatch for {name}: expected {expected} bytes, downloaded {actual}")]
    ArtifactSizeMismatch {
        name: String,
        expected: u64,
        actual: u64,
    },
    #[error("SHA-256 mismatch for {name}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        name: String,
        expected: String,
        actual: String,
    },
    #[error("state file could not be recovered: {0}")]
    StateCorrupt(String),
    #[error("there is no staged release to record as activated")]
    NoStagedRelease,
    #[error("there is no retained release to roll back to")]
    NoRollbackAvailable,
    #[error("another VEXA update operation is already running")]
    OperationInProgress,
    #[error("GitHub release contains a non-official asset URL: {0}")]
    UntrustedAssetUrl(String),
    #[error("release asset redirected outside the approved GitHub download hosts: {0}")]
    UnsafeRedirect(String),
    #[error("HTTP request returned unexpected status {0}")]
    HttpStatus(u16),
    #[error("staged version is {staged}, not requested activation version {requested}")]
    StagedVersionMismatch { staged: String, requested: String },
    #[error("there is no install-root parent for updater directory {0}")]
    MissingInstallRoot(PathBuf),
    #[error("unsupported package archive format: {0}")]
    UnsupportedArchive(String),
    #[error("release archive contains an unsafe or unexpected entry: {0}")]
    UnsafeArchiveEntry(String),
    #[error("release archive is missing required file {0}")]
    MissingPackageFile(String),
    #[error("version directory already exists and will not be overwritten: {0}")]
    VersionAlreadyInstalled(PathBuf),
    #[error("installed core is missing, non-regular, or not executable: {0}")]
    InvalidInstalledCore(PathBuf),
    #[error("unsafe install directory (expected a real directory): {0}")]
    UnsafeInstallDirectory(PathBuf),
    #[error("staged target {staged} does not match this host target {host}")]
    StagedTargetMismatch { staged: String, host: String },
    #[error("staged VEXA core health check failed with status {0}")]
    HealthCheckFailed(String),
}

pub type Result<T> = std::result::Result<T, UpdateError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatingSystem {
    Windows,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    X86_64,
    Aarch64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinuxLibc {
    Glibc,
    Musl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformTarget {
    pub os: OperatingSystem,
    pub arch: Architecture,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub libc: Option<LinuxLibc>,
}

impl PlatformTarget {
    /// Detect the supported release target for the current process.
    ///
    /// # Errors
    ///
    /// Returns [`UpdateError::UnsupportedTarget`] for operating systems or CPU architectures that
    /// VEXA does not publish updater artifacts for.
    pub fn detect() -> Result<Self> {
        let os = match std::env::consts::OS {
            "windows" => OperatingSystem::Windows,
            "linux" => OperatingSystem::Linux,
            other => return Err(UpdateError::UnsupportedTarget(other.to_owned())),
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => Architecture::X86_64,
            "aarch64" => Architecture::Aarch64,
            other => return Err(UpdateError::UnsupportedTarget(other.to_owned())),
        };
        if os == OperatingSystem::Windows && arch != Architecture::X86_64 {
            return Err(UpdateError::UnsupportedTarget(format!(
                "windows/{}",
                std::env::consts::ARCH
            )));
        }
        let libc = if os == OperatingSystem::Linux {
            if cfg!(target_env = "musl") {
                Some(LinuxLibc::Musl)
            } else if cfg!(target_env = "gnu") {
                Some(LinuxLibc::Glibc)
            } else {
                None
            }
        } else {
            None
        };
        Ok(Self { os, arch, libc })
    }
}

impl std::fmt::Display for PlatformTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let os = match self.os {
            OperatingSystem::Windows => "windows",
            OperatingSystem::Linux => "linux",
        };
        let arch = match self.arch {
            Architecture::X86_64 => "x86_64",
            Architecture::Aarch64 => "aarch64",
        };
        match self.libc {
            Some(LinuxLibc::Glibc) => write!(formatter, "{os}/{arch}/glibc"),
            Some(LinuxLibc::Musl) => write!(formatter, "{os}/{arch}/musl"),
            None => write!(formatter, "{os}/{arch}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub download_url: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseMetadata {
    pub version: String,
    pub tag_name: String,
    pub prerelease: bool,
    pub assets: Vec<ReleaseAsset>,
}

impl ReleaseMetadata {
    /// Parse GitHub release JSON into the updater's release model.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid JSON, draft releases, or non-semantic release tags.
    pub fn from_github_json(json: &str) -> Result<Self> {
        let release: GithubRelease = serde_json::from_str(json)?;
        Self::try_from(release)
    }

    #[must_use]
    pub fn checksums_asset(&self) -> Option<&ReleaseAsset> {
        self.assets
            .iter()
            .find(|asset| is_checksums_asset(&asset.name))
    }
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

impl TryFrom<GithubRelease> for ReleaseMetadata {
    type Error = UpdateError;

    fn try_from(value: GithubRelease) -> Result<Self> {
        if value.draft {
            return Err(UpdateError::DraftRelease);
        }
        parse_semantic_version(&value.tag_name)?;
        let version = value
            .tag_name
            .strip_prefix('v')
            .or_else(|| value.tag_name.strip_prefix('V'))
            .unwrap_or(&value.tag_name)
            .to_owned();
        let assets = value
            .assets
            .into_iter()
            .map(|asset| ReleaseAsset {
                name: asset.name,
                download_url: asset.browser_download_url,
                size: asset.size,
            })
            .collect();
        Ok(Self {
            version,
            tag_name: value.tag_name,
            prerelease: value.prerelease,
            assets,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableUpdate {
    pub release: ReleaseMetadata,
    pub artifact: ReleaseAsset,
    pub checksums_asset: ReleaseAsset,
    pub target: PlatformTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedRelease {
    pub version: String,
    pub tag_name: String,
    pub artifact_name: String,
    pub artifact_path: PathBuf,
    pub sha256: String,
    pub target: PlatformTarget,
    pub staged_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledVersion {
    pub version: String,
    pub artifact_name: String,
    pub artifact_path: PathBuf,
    pub sha256: String,
    pub activated_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackMetadata {
    pub from_version: String,
    pub to_version: String,
    pub artifact_path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateManifest {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<InstalledVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<InstalledVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged: Option<StagedRelease>,
    pub updated_unix_ms: u64,
}

impl Default for StateManifest {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            current: None,
            previous: None,
            staged: None,
            updated_unix_ms: now_unix_ms(),
        }
    }
}

impl StateManifest {
    #[must_use]
    pub fn rollback_metadata(&self) -> Option<RollbackMetadata> {
        let current = self.current.as_ref()?;
        let previous = self.previous.as_ref()?;
        Some(RollbackMetadata {
            from_version: current.version.clone(),
            to_version: previous.version.clone(),
            artifact_path: previous.artifact_path.clone(),
            sha256: previous.sha256.clone(),
        })
    }
}

pub struct Updater {
    root: PathBuf,
    agent: ureq::Agent,
}

struct UpdateLock {
    _file: File,
    _process_reservation: ProcessReservation,
}

struct ProcessReservation(PathBuf);

impl Drop for ProcessReservation {
    fn drop(&mut self) {
        let locks = PROCESS_LIFECYCLE_LOCKS.get_or_init(|| Mutex::new(BTreeSet::new()));
        if let Ok(mut locks) = locks.lock() {
            locks.remove(&self.0);
        }
    }
}

impl Updater {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(HTTP_TIMEOUT))
            .build()
            .into();
        Self {
            root: root.into(),
            agent,
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn acquire_lifecycle_lock(&self) -> Result<UpdateLock> {
        fs::create_dir_all(&self.root)?;
        let root_key = fs::canonicalize(&self.root)?;
        {
            let locks = PROCESS_LIFECYCLE_LOCKS.get_or_init(|| Mutex::new(BTreeSet::new()));
            let mut locks = locks.lock().map_err(|_| UpdateError::OperationInProgress)?;
            if !locks.insert(root_key.clone()) {
                return Err(UpdateError::OperationInProgress);
            }
        }
        let process_reservation = ProcessReservation(root_key);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join(LIFECYCLE_LOCK_FILE))?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(UpdateLock {
                _file: file,
                _process_reservation: process_reservation,
            }),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                Err(UpdateError::OperationInProgress)
            }
            Err(error) => Err(UpdateError::Io(error)),
        }
    }

    /// Fetch the latest official GitHub release metadata.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP request fails, the response exceeds the metadata limit, or
    /// GitHub returns release data that fails validation.
    pub fn fetch_latest_release(&self) -> Result<ReleaseMetadata> {
        let mut response = self
            .agent
            .get(LATEST_RELEASE_API)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", USER_AGENT)
            .call()?;
        ensure_success_status(response.status().as_u16())?;
        let release: GithubRelease = response
            .body_mut()
            .with_config()
            .limit(METADATA_MAX_BYTES)
            .read_json()?;
        ReleaseMetadata::try_from(release)
    }

    /// Check whether the latest official release is newer and select its assets for `target`.
    ///
    /// # Errors
    ///
    /// Returns an error for failed release discovery, invalid versions, unsupported release assets,
    /// or a missing `SHA256SUMS` asset.
    pub fn check_for_update(
        &self,
        current_version: &str,
        target: PlatformTarget,
    ) -> Result<Option<AvailableUpdate>> {
        let release = self.fetch_latest_release()?;
        if !is_version_newer(current_version, &release.version)? {
            return Ok(None);
        }
        let artifact = select_artifact(&release, target)?.clone();
        let checksums_asset = release
            .checksums_asset()
            .ok_or(UpdateError::MissingChecksumsAsset)?
            .clone();
        Ok(Some(AvailableUpdate {
            release,
            artifact,
            checksums_asset,
            target,
        }))
    }

    /// Download and checksum-verify an available release into the staging directory.
    ///
    /// # Errors
    ///
    /// Returns an error for failed downloads, unsafe asset names, size-limit violations, checksum
    /// failures, filesystem failures, or state persistence failures.
    pub fn stage_update(&self, update: &AvailableUpdate) -> Result<StagedRelease> {
        let _lock = self.acquire_lifecycle_lock()?;
        self.stage_update_locked(update)
    }

    fn stage_update_locked(&self, update: &AvailableUpdate) -> Result<StagedRelease> {
        if update.artifact.size > ARTIFACT_MAX_BYTES {
            return Err(UpdateError::ArtifactTooLarge {
                name: update.artifact.name.clone(),
                size: update.artifact.size,
                max: ARTIFACT_MAX_BYTES,
            });
        }
        validate_asset_name(&update.artifact.name)?;
        let tag_version = update
            .release
            .tag_name
            .strip_prefix('v')
            .or_else(|| update.release.tag_name.strip_prefix('V'))
            .unwrap_or(&update.release.tag_name);
        if update.checksums_asset.name != "SHA256SUMS"
            || update.release.version != tag_version
            || select_artifact(&update.release, update.target)?.name != update.artifact.name
        {
            return Err(UpdateError::UntrustedAssetUrl(
                update.artifact.download_url.clone(),
            ));
        }
        validate_release_asset_url(
            &update.release.tag_name,
            &update.checksums_asset.name,
            &update.checksums_asset.download_url,
        )?;
        validate_release_asset_url(
            &update.release.tag_name,
            &update.artifact.name,
            &update.artifact.download_url,
        )?;
        let checksums = self.download_checksums(&update.checksums_asset.download_url)?;
        let expected = checksum_for_asset(&checksums, &update.artifact.name)?;

        let mut response = self.official_asset_response(&update.artifact.download_url)?;
        let reader = response.body_mut().as_reader();
        self.stage_from_reader(update, &expected, reader)
    }

    /// Load updater state, falling back to the recovery copy when necessary.
    ///
    /// # Errors
    ///
    /// Returns an error if existing state and its recovery copy cannot be read or validated.
    pub fn load_state(&self) -> Result<StateManifest> {
        load_state_file(&self.state_path(), &self.backup_state_path())
    }

    /// Record an activation performed by a separate installer or launcher.
    ///
    /// This only changes metadata. The supplied path must still contain the same verified release
    /// payload that was staged, so activation cannot silently bless different bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested version was not staged, the activated payload does not
    /// match its verified checksum, or state cannot be persisted.
    pub fn record_activation(
        &self,
        version: &str,
        activated_artifact: impl AsRef<Path>,
    ) -> Result<StateManifest> {
        let _lock = self.acquire_lifecycle_lock()?;
        parse_semantic_version(version)?;
        let activated_artifact = activated_artifact.as_ref();
        let mut state = self.load_state()?;

        if let Some(current) = state.current.as_ref()
            && versions_equal(&current.version, version)?
        {
            verify_file_checksum(activated_artifact, &current.sha256, &current.artifact_name)?;
            return Ok(state);
        }

        let staged = state.staged.clone().ok_or(UpdateError::NoStagedRelease)?;
        if !versions_equal(&staged.version, version)? {
            return Err(UpdateError::StagedVersionMismatch {
                staged: staged.version,
                requested: version.to_owned(),
            });
        }
        verify_file_checksum(activated_artifact, &staged.sha256, &staged.artifact_name)?;

        let installed = InstalledVersion {
            version: staged.version,
            artifact_name: staged.artifact_name,
            artifact_path: activated_artifact.to_path_buf(),
            sha256: staged.sha256,
            activated_unix_ms: now_unix_ms(),
        };
        state.previous = state.current.take();
        state.current = Some(installed);
        state.staged = None;
        state.updated_unix_ms = now_unix_ms();
        self.write_state(&state)?;
        Ok(state)
    }

    /// Return metadata needed by an external installer to roll back to the previous release.
    ///
    /// # Errors
    ///
    /// Returns an error if updater state cannot be loaded or recovered.
    pub fn rollback_metadata(&self) -> Result<Option<RollbackMetadata>> {
        Ok(self.load_state()?.rollback_metadata())
    }

    /// Install a staged verified release, health-check it, and atomically select it in `current.json`.
    /// The former version directory is retained and boot metadata keeps it available for rollback.
    ///
    /// # Errors
    ///
    /// Returns an error when staging data fails verification, the package is unsafe or invalid,
    /// the core health check fails, or filesystem/manifest persistence fails.
    pub fn activate_staged(&self) -> Result<StateManifest> {
        let _lock = self.acquire_lifecycle_lock()?;
        self.activate_staged_inner(run_health_check)
    }

    #[cfg(test)]
    fn activate_staged_with(
        &self,
        health_check: impl FnOnce(&Path, &Path) -> Result<()>,
    ) -> Result<StateManifest> {
        let _lock = self.acquire_lifecycle_lock()?;
        self.activate_staged_inner(health_check)
    }

    /// Download, verify, install, health-check, and activate one release under a single lifecycle lock.
    ///
    /// # Errors
    ///
    /// Returns the staging or activation error without switching away from the working version.
    pub fn stage_and_activate(
        &self,
        update: &AvailableUpdate,
    ) -> Result<(StagedRelease, StateManifest)> {
        let _lock = self.acquire_lifecycle_lock()?;
        let staged = self.stage_update_locked(update)?;
        let state = self.activate_staged_inner(run_health_check)?;
        Ok((staged, state))
    }

    fn activate_staged_inner(
        &self,
        health_check: impl FnOnce(&Path, &Path) -> Result<()>,
    ) -> Result<StateManifest> {
        let install_root = self
            .root
            .parent()
            .ok_or_else(|| UpdateError::MissingInstallRoot(self.root.clone()))?;
        let mut state = self.load_state()?;
        let staged = state.staged.clone().ok_or(UpdateError::NoStagedRelease)?;
        let host_target = PlatformTarget::detect()?;
        if staged.target != host_target {
            return Err(UpdateError::StagedTargetMismatch {
                staged: staged.target.to_string(),
                host: host_target.to_string(),
            });
        }
        if !artifact_names_for(staged.version.as_str(), host_target)?
            .iter()
            .any(|name| name == &staged.artifact_name)
        {
            return Err(UpdateError::UnsafeAssetName(staged.artifact_name));
        }
        let current_manifest_path = install_root.join(vexa_boot::CURRENT_MANIFEST_FILE);
        match fs::symlink_metadata(&current_manifest_path) {
            Ok(_) => {
                let current_manifest = vexa_boot::load_current_manifest(install_root)?;
                state.current = Some(installed_core_metadata(
                    install_root,
                    &current_manifest.version,
                )?);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(UpdateError::Io(error)),
        }
        verify_file_checksum(&staged.artifact_path, &staged.sha256, &staged.artifact_name)?;
        validate_version_segment(&staged.version)?;
        let versions = install_root.join("versions");
        ensure_real_directory(&versions)?;
        let version_dir = versions.join(&staged.version);
        match fs::symlink_metadata(&version_dir) {
            Ok(_) => return Err(UpdateError::VersionAlreadyInstalled(version_dir)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(UpdateError::Io(error)),
        }
        let package_root = versions.join(format!(
            ".stage-{}-{}",
            staged.version,
            TEMP_COUNTER.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::create_dir(&package_root)?;
        if let Err(error) = extract_package(&staged, &package_root) {
            let _ = fs::remove_dir_all(&package_root);
            return Err(error);
        }
        let core = package_root.join(core_name());
        if let Err(error) = health_check(&core, install_root) {
            let _ = fs::remove_dir_all(&package_root);
            return Err(error);
        }
        if let Err(error) = fs::rename(&package_root, &version_dir) {
            let _ = fs::remove_dir_all(&package_root);
            return Err(UpdateError::Io(error));
        }
        let activated = match installed_core_metadata(install_root, &staged.version) {
            Ok(activated) => activated,
            Err(error) => {
                let _ = fs::remove_dir_all(&version_dir);
                return Err(error);
            }
        };
        if let Err(error) = vexa_boot::activate_version(install_root, &staged.version) {
            let _ = fs::remove_dir_all(&version_dir);
            return Err(error.into());
        }
        if let Some(current) = state.current.take() {
            state.previous = Some(current);
        }
        state.current = Some(activated);
        state.staged = None;
        state.updated_unix_ms = now_unix_ms();
        if let Err(error) = self.write_state(&state) {
            let manifest_restored = if state.previous.is_some() {
                vexa_boot::rollback_version(install_root).is_ok()
            } else {
                fs::remove_file(install_root.join(vexa_boot::CURRENT_MANIFEST_FILE)).is_ok()
            };
            if manifest_restored {
                let _ = fs::remove_dir_all(&version_dir);
            }
            return Err(error);
        }
        Ok(state)
    }

    /// Select an installer-verified, extracted version and retain hashes for rollback.
    ///
    /// The installer must authenticate its archive before calling this method. This method validates
    /// installed paths, runs the core health check, and serializes selection with updater operations.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid installed files, health failure, corrupt state, or persistence.
    pub fn activate_installed(&self, version: &str) -> Result<StateManifest> {
        let _lock = self.acquire_lifecycle_lock()?;
        self.activate_installed_inner(version, run_health_check)
    }

    fn activate_installed_inner(
        &self,
        version: &str,
        health_check: impl FnOnce(&Path, &Path) -> Result<()>,
    ) -> Result<StateManifest> {
        validate_version_segment(version)?;
        let install_root = self
            .root
            .parent()
            .ok_or_else(|| UpdateError::MissingInstallRoot(self.root.clone()))?;
        let mut state = self.load_state()?;
        let next = installed_core_metadata(install_root, version)?;
        health_check(&next.artifact_path, install_root)?;
        let old = match fs::symlink_metadata(install_root.join(vexa_boot::CURRENT_MANIFEST_FILE)) {
            Ok(_) => Some(vexa_boot::load_current_manifest(install_root)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let changed = old.as_ref().is_some_and(|old| old.version != version);
        if let Some(old) = old.as_ref().filter(|_| changed) {
            let retained = if let Some(record) = state.current.as_ref() {
                if record.version != old.version {
                    return Err(UpdateError::StateCorrupt(
                        "installer selection disagrees with updater state".to_owned(),
                    ));
                }
                verify_installed_core(install_root, record)?;
                record.clone()
            } else {
                installed_core_metadata(install_root, &old.version)?
            };
            state.previous = Some(retained);
        }
        vexa_boot::activate_version(install_root, version)?;
        state.current = Some(next);
        state.staged = None;
        state.updated_unix_ms = now_unix_ms();
        if let Err(error) = self.write_state(&state) {
            if changed {
                let _ = vexa_boot::rollback_version(install_root);
            } else if old.is_none() {
                let _ = fs::remove_file(install_root.join(vexa_boot::CURRENT_MANIFEST_FILE));
            }
            return Err(error);
        }
        Ok(state)
    }

    /// Switch to the previous installed release. Both release files and updater metadata remain on disk.
    ///
    /// # Errors
    ///
    /// Returns an error when no verified rollback target exists, its files fail checksum
    /// verification, the boot manifest disagrees, or persistence fails.
    pub fn rollback(&self) -> Result<StateManifest> {
        let install_root = self
            .root
            .parent()
            .ok_or_else(|| UpdateError::MissingInstallRoot(self.root.clone()))?;
        let _lock = self.acquire_lifecycle_lock()?;
        let mut state = self.load_state()?;
        let current = state
            .current
            .clone()
            .ok_or(UpdateError::NoRollbackAvailable)?;
        let previous = state
            .previous
            .clone()
            .ok_or(UpdateError::NoRollbackAvailable)?;
        let current_manifest = vexa_boot::load_current_manifest(install_root)?;
        if current_manifest.version != current.version {
            return Err(UpdateError::StateCorrupt(format!(
                "updater current version {} does not match boot current version {}",
                current.version, current_manifest.version
            )));
        }
        let boot_previous = vexa_boot::load_previous_manifest(install_root)?;
        if boot_previous.version != previous.version {
            return Err(UpdateError::StateCorrupt(format!(
                "updater rollback target {} does not match boot rollback target {}",
                previous.version, boot_previous.version
            )));
        }
        verify_installed_core(install_root, &current)?;
        verify_installed_core(install_root, &previous)?;
        vexa_boot::rollback_version(install_root)?;
        state.current = Some(previous);
        state.previous = Some(current);
        state.updated_unix_ms = now_unix_ms();
        if let Err(error) = self.write_state(&state) {
            let _ = vexa_boot::rollback_version(install_root);
            return Err(error);
        }
        Ok(state)
    }

    fn download_checksums(&self, url: &str) -> Result<String> {
        let mut response = self.official_asset_response(url)?;
        Ok(response
            .body_mut()
            .with_config()
            .limit(CHECKSUMS_MAX_BYTES)
            .read_to_string()?)
    }

    fn official_asset_response(&self, start_url: &str) -> Result<ureq::http::Response<ureq::Body>> {
        let mut url = start_url.to_owned();
        for redirect_count in 0..=MAX_REDIRECTS {
            let response = self
                .agent
                .get(&url)
                .header("User-Agent", USER_AGENT)
                .call()?;
            let status = response.status();
            if status.is_success() {
                return Ok(response);
            }
            if !(300..400).contains(&status.as_u16()) {
                return Err(UpdateError::HttpStatus(status.as_u16()));
            }
            if redirect_count == MAX_REDIRECTS {
                return Err(UpdateError::UnsafeRedirect(url));
            }
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| UpdateError::UnsafeRedirect(url.clone()))?;
            validate_redirect_url(location)?;
            location.clone_into(&mut url);
        }
        Err(UpdateError::UnsafeRedirect(url))
    }

    fn stage_from_reader(
        &self,
        update: &AvailableUpdate,
        expected_sha256: &str,
        mut reader: impl Read,
    ) -> Result<StagedRelease> {
        validate_asset_name(&update.artifact.name)?;
        let version_segment = validate_version_segment(&update.release.version)?;
        let stage_dir = self.root.join("staging").join(version_segment);
        fs::create_dir_all(&stage_dir)?;
        let final_path = stage_dir.join(&update.artifact.name);

        if final_path.exists() {
            verify_file_checksum(&final_path, expected_sha256, &update.artifact.name)?;
            let staged = Self::build_staged(update, final_path, expected_sha256);
            self.persist_staged(&staged)?;
            return Ok(staged);
        }

        let partial_path = unique_temp_path(&final_path, "part");
        let mut partial = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial_path)?;
        let mut hasher = Sha256::new();
        let mut total = 0_u64;
        let mut buffer = vec![0_u8; 64 * 1024];

        let copy_result: Result<()> = (|| {
            loop {
                let read = reader.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                total = total.saturating_add(read as u64);
                if total > ARTIFACT_MAX_BYTES {
                    return Err(UpdateError::ArtifactTooLarge {
                        name: update.artifact.name.clone(),
                        size: total,
                        max: ARTIFACT_MAX_BYTES,
                    });
                }
                hasher.update(&buffer[..read]);
                partial.write_all(&buffer[..read])?;
            }
            partial.sync_all()?;
            Ok(())
        })();

        if let Err(error) = copy_result {
            drop(partial);
            let _ = fs::remove_file(&partial_path);
            return Err(error);
        }
        drop(partial);

        if update.artifact.size != 0 && total != update.artifact.size {
            let _ = fs::remove_file(&partial_path);
            return Err(UpdateError::ArtifactSizeMismatch {
                name: update.artifact.name.clone(),
                expected: update.artifact.size,
                actual: total,
            });
        }

        let actual_sha256 = hex_digest(hasher.finalize().as_slice());
        if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
            let _ = fs::remove_file(&partial_path);
            return Err(UpdateError::ChecksumMismatch {
                name: update.artifact.name.clone(),
                expected: expected_sha256.to_ascii_lowercase(),
                actual: actual_sha256,
            });
        }

        if let Err(rename_error) = fs::rename(&partial_path, &final_path) {
            if final_path.exists()
                && verify_file_checksum(&final_path, expected_sha256, &update.artifact.name).is_ok()
            {
                let _ = fs::remove_file(&partial_path);
            } else {
                let _ = fs::remove_file(&partial_path);
                return Err(UpdateError::Io(rename_error));
            }
        }

        let staged = Self::build_staged(update, final_path, &actual_sha256);
        self.persist_staged(&staged)?;
        Ok(staged)
    }

    fn build_staged(
        update: &AvailableUpdate,
        artifact_path: PathBuf,
        sha256: &str,
    ) -> StagedRelease {
        StagedRelease {
            version: update.release.version.clone(),
            tag_name: update.release.tag_name.clone(),
            artifact_name: update.artifact.name.clone(),
            artifact_path,
            sha256: sha256.to_ascii_lowercase(),
            target: update.target,
            staged_unix_ms: now_unix_ms(),
        }
    }

    fn persist_staged(&self, staged: &StagedRelease) -> Result<()> {
        let mut state = self.load_state()?;
        state.staged = Some(staged.clone());
        state.updated_unix_ms = now_unix_ms();
        self.write_state(&state)
    }

    fn write_state(&self, state: &StateManifest) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        write_state_file(&self.state_path(), &self.backup_state_path(), state)
    }

    fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    fn backup_state_path(&self) -> PathBuf {
        self.root.join("state.json.bak")
    }
}

/// Compare two semantic versions using `SemVer` precedence rules.
///
/// # Errors
///
/// Returns [`UpdateError::InvalidVersion`] if either input is not a valid semantic version.
pub fn is_version_newer(current_version: &str, candidate_version: &str) -> Result<bool> {
    Ok(
        parse_semantic_version(candidate_version)?.cmp(&parse_semantic_version(current_version)?)
            == Ordering::Greater,
    )
}

/// Select the best official release artifact for a supported target.
///
/// # Errors
///
/// Returns [`UpdateError::NoMatchingArtifact`] if the release has no compatible VEXA artifact.
pub fn select_artifact(release: &ReleaseMetadata, target: PlatformTarget) -> Result<&ReleaseAsset> {
    for name in artifact_names_for(&release.version, target)? {
        let mut matches = release.assets.iter().filter(|asset| asset.name == name);
        if let Some(asset) = matches.next() {
            if matches.next().is_some() {
                return Err(UpdateError::DuplicateArtifact(name));
            }
            return Ok(asset);
        }
    }
    Err(UpdateError::NoMatchingArtifact(target.to_string()))
}

/// Parse GNU or BSD-style SHA-256 checksum lines into a deterministic map.
///
/// # Errors
///
/// Returns an error for malformed digests, malformed lines, or unsafe asset names.
pub fn parse_sha256sums(input: &str) -> Result<BTreeMap<String, String>> {
    let mut checksums = BTreeMap::new();
    for raw_line in input.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (digest, file_name) = if let Some(rest) = line.strip_prefix("SHA256 (") {
            let (name, digest) = rest
                .split_once(") = ")
                .ok_or_else(|| UpdateError::InvalidChecksumLine(line.to_owned()))?;
            (digest, name)
        } else {
            let mut parts = line.split_whitespace();
            let digest = parts
                .next()
                .ok_or_else(|| UpdateError::InvalidChecksumLine(line.to_owned()))?;
            let file_name = parts
                .next()
                .ok_or_else(|| UpdateError::InvalidChecksumLine(line.to_owned()))?
                .trim_start_matches('*')
                .trim_start_matches("./");
            if parts.next().is_some() {
                return Err(UpdateError::InvalidChecksumLine(line.to_owned()));
            }
            (digest, file_name)
        };

        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(UpdateError::InvalidChecksumLine(line.to_owned()));
        }
        validate_asset_name(file_name)?;
        checksums.insert(file_name.to_owned(), digest.to_ascii_lowercase());
    }
    Ok(checksums)
}

fn checksum_for_asset(checksum_text: &str, artifact_name: &str) -> Result<String> {
    let checksums = parse_sha256sums(checksum_text)?;
    checksums
        .get(artifact_name)
        .cloned()
        .ok_or_else(|| UpdateError::MissingArtifactChecksum(artifact_name.to_owned()))
}

fn is_checksums_asset(name: &str) -> bool {
    name.eq_ignore_ascii_case("SHA256SUMS") || name.eq_ignore_ascii_case("SHA256SUMS.txt")
}

fn artifact_names_for(version: &str, target: PlatformTarget) -> Result<Vec<String>> {
    let native = match (target.os, target.arch, target.libc) {
        (OperatingSystem::Windows, Architecture::X86_64, _) => {
            vec![("x86_64-pc-windows-msvc", "zip")]
        }
        (OperatingSystem::Linux, Architecture::X86_64, Some(LinuxLibc::Glibc)) => {
            vec![("x86_64-unknown-linux-gnu", "tar.gz")]
        }
        (OperatingSystem::Linux, Architecture::X86_64, Some(LinuxLibc::Musl)) => {
            vec![("x86_64-unknown-linux-musl", "tar.gz")]
        }
        (OperatingSystem::Linux, Architecture::Aarch64, Some(LinuxLibc::Glibc)) => {
            // The shipped AArch64 musl release is statically linked and runs on glibc too.
            // This is the sole libc fallback; OS and architecture always remain exact.
            vec![
                ("aarch64-unknown-linux-gnu", "tar.gz"),
                ("aarch64-unknown-linux-musl", "tar.gz"),
            ]
        }
        (OperatingSystem::Linux, Architecture::Aarch64, Some(LinuxLibc::Musl)) => {
            vec![("aarch64-unknown-linux-musl", "tar.gz")]
        }
        _ => return Err(UpdateError::UnsupportedTarget(target.to_string())),
    };
    let mut names = Vec::with_capacity(native.len() * 2);
    for (triple, extension) in native {
        names.push(format!("vexa-{version}-{triple}.{extension}"));
        // Legacy releases used the same exact target name without a version component.
        names.push(format!("vexa-{triple}.{extension}"));
    }
    Ok(names)
}

fn validate_asset_name(name: &str) -> Result<()> {
    let path = Path::new(name);
    if name.is_empty()
        || name == "."
        || name == ".."
        || path.file_name().and_then(|part| part.to_str()) != Some(name)
        || name.contains('/')
        || name.contains('\\')
    {
        return Err(UpdateError::UnsafeAssetName(name.to_owned()));
    }
    Ok(())
}

fn validate_version_segment(version: &str) -> Result<&str> {
    parse_semantic_version(version)?;
    if version.is_empty()
        || version.contains('/')
        || version.contains('\\')
        || version == "."
        || version == ".."
    {
        return Err(UpdateError::InvalidVersion(version.to_owned()));
    }
    Ok(version)
}

fn verify_file_checksum(path: &Path, expected: &str, name: &str) -> Result<()> {
    let actual = sha256_file(path)?;
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(UpdateError::ChecksumMismatch {
            name: name.to_owned(),
            expected: expected.to_ascii_lowercase(),
            actual,
        })
    }
}

fn sha256_file(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_digest(hasher.finalize().as_slice()))
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn load_state_file(path: &Path, backup: &Path) -> Result<StateManifest> {
    if !path.exists() {
        if backup.exists() {
            return read_state(backup);
        }
        return Ok(StateManifest::default());
    }
    match read_state(path) {
        Ok(state) => Ok(state),
        Err(primary_error) if backup.exists() => read_state(backup).map_err(|backup_error| {
            UpdateError::StateCorrupt(format!(
                "primary state error: {primary_error}; backup state error: {backup_error}"
            ))
        }),
        Err(error) => Err(UpdateError::StateCorrupt(error.to_string())),
    }
}

fn read_state(path: &Path) -> Result<StateManifest> {
    let file = File::open(path)?;
    let state: StateManifest = serde_json::from_reader(BufReader::new(file))?;
    if state.schema_version != STATE_SCHEMA_VERSION {
        return Err(UpdateError::StateCorrupt(format!(
            "unsupported schema version {}",
            state.schema_version
        )));
    }
    Ok(state)
}

fn write_state_file(path: &Path, backup: &Path, state: &StateManifest) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        UpdateError::StateCorrupt(format!("state path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent)?;
    let temp = unique_temp_path(path, "tmp");
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, state)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    drop(writer);

    let had_primary = path.exists();
    if had_primary {
        if backup.exists() {
            fs::remove_file(backup)?;
        }
        if let Err(error) = fs::rename(path, backup) {
            let _ = fs::remove_file(&temp);
            return Err(UpdateError::Io(error));
        }
    }

    if let Err(error) = fs::rename(&temp, path) {
        if had_primary && backup.exists() {
            let _ = fs::rename(backup, path);
        }
        let _ = fs::remove_file(&temp);
        return Err(UpdateError::Io(error));
    }
    if backup.exists() {
        let _ = fs::remove_file(backup);
    }
    Ok(())
}

fn unique_temp_path(path: &Path, suffix: &str) -> PathBuf {
    let counter = TEMP_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("vexa");
    path.with_file_name(format!(
        ".{file_name}.{suffix}.{}.{}",
        std::process::id(),
        counter
    ))
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SemanticVersion {
    major: u64,
    minor: u64,
    patch: u64,
    prerelease: Vec<PrereleaseIdentifier>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PrereleaseIdentifier {
    Numeric(u64),
    Text(String),
}

impl Ord for SemanticVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
            .then_with(|| compare_prerelease(&self.prerelease, &other.prerelease))
    }
}

impl PartialOrd for SemanticVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn parse_semantic_version(input: &str) -> Result<SemanticVersion> {
    let value = input
        .strip_prefix('v')
        .or_else(|| input.strip_prefix('V'))
        .unwrap_or(input);
    let (without_build, build) = match value.split_once('+') {
        Some((left, right)) => (left, Some(right)),
        None => (value, None),
    };
    if let Some(build) = build {
        validate_identifiers(build, false, input)?;
    }
    let (core, prerelease) = match without_build.split_once('-') {
        Some((left, right)) => (left, Some(right)),
        None => (without_build, None),
    };
    let mut core_parts = core.split('.');
    let major = parse_core_number(core_parts.next(), input)?;
    let minor = parse_core_number(core_parts.next(), input)?;
    let patch = parse_core_number(core_parts.next(), input)?;
    if core_parts.next().is_some() {
        return Err(UpdateError::InvalidVersion(input.to_owned()));
    }

    let prerelease = if let Some(prerelease) = prerelease {
        validate_identifiers(prerelease, true, input)?;
        prerelease
            .split('.')
            .map(|identifier| {
                if identifier.bytes().all(|byte| byte.is_ascii_digit()) {
                    identifier
                        .parse::<u64>()
                        .map(PrereleaseIdentifier::Numeric)
                        .map_err(|_| UpdateError::InvalidVersion(input.to_owned()))
                } else {
                    Ok(PrereleaseIdentifier::Text(identifier.to_owned()))
                }
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    Ok(SemanticVersion {
        major,
        minor,
        patch,
        prerelease,
    })
}

fn parse_core_number(value: Option<&str>, original: &str) -> Result<u64> {
    let value = value.ok_or_else(|| UpdateError::InvalidVersion(original.to_owned()))?;
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(UpdateError::InvalidVersion(original.to_owned()));
    }
    value
        .parse()
        .map_err(|_| UpdateError::InvalidVersion(original.to_owned()))
}

fn validate_identifiers(value: &str, prerelease: bool, original: &str) -> Result<()> {
    for identifier in value.split('.') {
        if identifier.is_empty()
            || !identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || (prerelease
                && identifier.len() > 1
                && identifier.starts_with('0')
                && identifier.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Err(UpdateError::InvalidVersion(original.to_owned()));
        }
    }
    Ok(())
}

fn compare_prerelease(left: &[PrereleaseIdentifier], right: &[PrereleaseIdentifier]) -> Ordering {
    match (left.is_empty(), right.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        (false, false) => {}
    }
    for (left, right) in left.iter().zip(right) {
        let ordering = match (left, right) {
            (PrereleaseIdentifier::Numeric(left), PrereleaseIdentifier::Numeric(right)) => {
                left.cmp(right)
            }
            (PrereleaseIdentifier::Numeric(_), PrereleaseIdentifier::Text(_)) => Ordering::Less,
            (PrereleaseIdentifier::Text(_), PrereleaseIdentifier::Numeric(_)) => Ordering::Greater,
            (PrereleaseIdentifier::Text(left), PrereleaseIdentifier::Text(right)) => {
                left.cmp(right)
            }
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

fn versions_equal(left: &str, right: &str) -> Result<bool> {
    Ok(parse_semantic_version(left)? == parse_semantic_version(right)?)
}

fn validate_release_asset_url(tag_name: &str, asset_name: &str, url: &str) -> Result<()> {
    validate_asset_name(asset_name)?;
    parse_semantic_version(tag_name)?;
    let expected = format!(
        "https://github.com/{OFFICIAL_REPOSITORY}/releases/download/{tag_name}/{asset_name}"
    );
    if url == expected {
        Ok(())
    } else {
        Err(UpdateError::UntrustedAssetUrl(url.to_owned()))
    }
}

fn validate_redirect_url(url: &str) -> Result<()> {
    if !url.starts_with("https://")
        || url.contains(['\\', '\r', '\n', '\t', ' '])
        || url.contains('#')
    {
        return Err(UpdateError::UnsafeRedirect(url.to_owned()));
    }
    let remainder = &url["https://".len()..];
    let authority_end = remainder.find(['/', '?']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    if authority.contains('@') {
        return Err(UpdateError::UnsafeRedirect(url.to_owned()));
    }
    let host = authority.to_ascii_lowercase();
    let path = &remainder[authority_end..];
    let allowed = match host.as_str() {
        "github.com" => path.starts_with(&format!("/{OFFICIAL_REPOSITORY}/releases/download/")),
        "release-assets.githubusercontent.com" | "objects.githubusercontent.com" => true,
        _ => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(UpdateError::UnsafeRedirect(url.to_owned()))
    }
}

fn ensure_success_status(status: u16) -> Result<()> {
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(UpdateError::HttpStatus(status))
    }
}

fn core_name() -> &'static str {
    if cfg!(windows) {
        "vexa-core.exe"
    } else {
        "vexa-core"
    }
}

fn boot_name() -> &'static str {
    if cfg!(windows) {
        "vexa-boot.exe"
    } else {
        "vexa-boot"
    }
}

fn installed_core_metadata(install_root: &Path, version: &str) -> Result<InstalledVersion> {
    validate_version_segment(version)?;
    for directory in [
        install_root.join("versions"),
        install_root.join("versions").join(version),
    ] {
        if !fs::symlink_metadata(&directory).is_ok_and(|metadata| metadata.file_type().is_dir()) {
            return Err(UpdateError::UnsafeInstallDirectory(directory));
        }
    }
    let core = install_root
        .join("versions")
        .join(version)
        .join(core_name());
    verify_regular_executable(&core)?;
    Ok(InstalledVersion {
        version: version.to_owned(),
        artifact_name: core_name().to_owned(),
        sha256: sha256_file(&core)?,
        artifact_path: core,
        activated_unix_ms: now_unix_ms(),
    })
}

fn verify_installed_core(install_root: &Path, version: &InstalledVersion) -> Result<()> {
    let installed = installed_core_metadata(install_root, &version.version)?;
    verify_file_checksum(&installed.artifact_path, &version.sha256, core_name())
}

fn verify_regular_executable(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| UpdateError::InvalidInstalledCore(path.to_path_buf()))?;
    if !metadata.file_type().is_file() {
        return Err(UpdateError::InvalidInstalledCore(path.to_path_buf()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(UpdateError::InvalidInstalledCore(path.to_path_buf()));
        }
    }
    Ok(())
}

fn ensure_real_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(UpdateError::UnsafeInstallDirectory(path.to_path_buf())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_dir() {
                Ok(())
            } else {
                Err(UpdateError::UnsafeInstallDirectory(path.to_path_buf()))
            }
        }
        Err(error) => Err(UpdateError::Io(error)),
    }
}

fn run_health_check(core: &Path, install_root: &Path) -> Result<()> {
    let mut child = Command::new(core)
        .arg("doctor")
        .env(vexa_boot::INSTALL_ROOT_ENV, install_root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < HEALTH_CHECK_TIMEOUT => {
                thread::sleep(Duration::from_millis(100));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(UpdateError::HealthCheckFailed(format!(
                    "timed out after {} seconds",
                    HEALTH_CHECK_TIMEOUT.as_secs()
                )));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(UpdateError::Io(error));
            }
        }
    };
    if status.success() {
        Ok(())
    } else {
        Err(UpdateError::HealthCheckFailed(status.code().map_or_else(
            || "terminated by signal".to_owned(),
            |code| code.to_string(),
        )))
    }
}

#[allow(clippy::too_many_lines)]
fn extract_package(staged: &StagedRelease, destination: &Path) -> Result<()> {
    let name = &staged.artifact_name;
    let (package_root, format) = if let Some(root) = name.strip_suffix(".tar.gz") {
        (root, "tar.gz")
    } else if let Some(root) = name.strip_suffix(".tgz") {
        (root, "tar.gz")
    } else if let Some(root) = name.strip_suffix(".zip") {
        (root, "zip")
    } else {
        return Err(UpdateError::UnsupportedArchive(name.clone()));
    };
    let files = [
        boot_name(),
        core_name(),
        "LICENSE",
        "THIRD_PARTY_LICENSES.txt",
    ];
    let required_files = [boot_name(), core_name(), "LICENSE"];
    let mut seen = BTreeSet::<String>::new();
    let mut extracted_total = 0_u64;
    match format {
        "tar.gz" => {
            let file = File::open(&staged.artifact_path)?;
            let decoder = flate2::read::GzDecoder::new(file);
            let mut archive = tar::Archive::new(decoder);
            for entry in archive.entries()? {
                let mut entry = entry?;
                let rendered = entry.path()?.to_string_lossy().into_owned();
                let is_dir = entry.header().entry_type().is_dir();
                let file_name = package_member(&rendered, package_root, &files, is_dir)?;
                let Some(file_name) = file_name else {
                    continue;
                };
                if !entry.header().entry_type().is_file() {
                    return Err(UpdateError::UnsafeArchiveEntry(rendered));
                }
                let output = destination.join(&file_name);
                let mut out = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&output)?;
                copy_limited(&mut entry, &mut out, &output, &mut extracted_total)?;
                out.sync_all()?;
                seen.insert(file_name);
            }
        }
        "zip" => {
            let file = File::open(&staged.artifact_path)?;
            let mut archive = zip::ZipArchive::new(file)
                .map_err(|error| UpdateError::UnsupportedArchive(error.to_string()))?;
            for index in 0..archive.len() {
                let mut entry = archive
                    .by_index(index)
                    .map_err(|error| UpdateError::UnsupportedArchive(error.to_string()))?;
                let rendered = entry.name().to_owned();
                let is_dir = entry.is_dir();
                let file_name = package_member(&rendered, package_root, &files, is_dir)?;
                let Some(file_name) = file_name else {
                    continue;
                };
                if is_dir
                    || entry.unix_mode().is_some_and(|mode| {
                        let file_type = mode & 0o170_000;
                        file_type != 0 && file_type != 0o100_000
                    })
                {
                    return Err(UpdateError::UnsafeArchiveEntry(rendered));
                }
                let output = destination.join(&file_name);
                let mut out = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&output)?;
                copy_limited(&mut entry, &mut out, &output, &mut extracted_total)?;
                out.sync_all()?;
                seen.insert(file_name);
            }
        }
        _ => unreachable!(),
    }
    for required in required_files {
        if !seen.contains(required) {
            return Err(UpdateError::MissingPackageFile(required.to_owned()));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for executable in [boot_name(), core_name()] {
            fs::set_permissions(
                destination.join(executable),
                fs::Permissions::from_mode(0o755),
            )?;
        }
    }
    Ok(())
}

fn package_member(
    path: &str,
    package_root: &str,
    allowed_files: &[&str],
    is_dir: bool,
) -> Result<Option<String>> {
    let normalized = path.replace('\\', "/");
    if is_dir && (normalized == package_root || normalized == format!("{package_root}/")) {
        return Ok(None);
    }
    let prefix = format!("{package_root}/");
    let Some(member) = normalized.strip_prefix(&prefix) else {
        return Err(UpdateError::UnsafeArchiveEntry(path.to_owned()));
    };
    if member.contains('/') || !allowed_files.contains(&member) {
        return Err(UpdateError::UnsafeArchiveEntry(path.to_owned()));
    }
    Ok(Some(member.to_owned()))
}

fn copy_limited(
    reader: &mut impl Read,
    writer: &mut impl Write,
    path: &Path,
    total: &mut u64,
) -> Result<()> {
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        *total = (*total).saturating_add(read as u64);
        if *total > ARTIFACT_MAX_BYTES {
            return Err(UpdateError::ArtifactTooLarge {
                name: path.display().to_string(),
                size: *total,
                max: ARTIFACT_MAX_BYTES,
            });
        }
        writer.write_all(&buffer[..read])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const RELEASE_FIXTURE: &str = include_str!("../tests/fixtures/release.json");
    const CHECKSUMS_FIXTURE: &str = include_str!("../tests/fixtures/SHA256SUMS");
    const ARTIFACT_FIXTURE: &[u8] =
        include_bytes!("../tests/fixtures/vexa-x86_64-unknown-linux-gnu.tar.gz");

    fn package_archive(version: &str, core_script: &[u8]) -> Vec<u8> {
        #[cfg(unix)]
        {
            use flate2::Compression;
            use flate2::write::GzEncoder;
            let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
            {
                let mut archive = tar::Builder::new(&mut gzip);
                let target = PlatformTarget::detect()
                    .unwrap_or_else(|error| panic!("detect fixture target failed: {error}"));
                let name = artifact_names_for(version, target)
                    .unwrap_or_else(|error| panic!("fixture artifact name failed: {error}"))[0]
                    .clone();
                let root = name.trim_end_matches(".tar.gz");
                for (name, bytes, mode) in [
                    ("vexa-boot", b"boot-placeholder".as_slice(), 0o755),
                    ("vexa-core", core_script, 0o755),
                    ("LICENSE", b"fixture license".as_slice(), 0o644),
                    (
                        "THIRD_PARTY_LICENSES.txt",
                        b"fixture notices".as_slice(),
                        0o644,
                    ),
                ] {
                    let mut header = tar::Header::new_gnu();
                    header.set_size(bytes.len() as u64);
                    header.set_mode(mode);
                    header.set_cksum();
                    archive
                        .append_data(&mut header, format!("{root}/{name}"), bytes)
                        .unwrap_or_else(|error| {
                            panic!("failed to add fixture package entry: {error}")
                        });
                }
                archive
                    .finish()
                    .unwrap_or_else(|error| panic!("failed to finish fixture archive: {error}"));
            }
            gzip.finish()
                .unwrap_or_else(|error| panic!("failed to finish gzip fixture: {error}"))
        }
        #[cfg(windows)]
        {
            use std::io::Write as _;
            use zip::write::SimpleFileOptions;
            let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
            let root = format!("vexa-{version}-x86_64-pc-windows-msvc");
            for (name, bytes) in [
                ("vexa-boot.exe", b"boot-placeholder".as_slice()),
                ("vexa-core.exe", core_script),
                ("LICENSE", b"fixture license".as_slice()),
                ("THIRD_PARTY_LICENSES.txt", b"fixture notices".as_slice()),
            ] {
                archive
                    .start_file(
                        format!("{root}/{name}"),
                        SimpleFileOptions::default().unix_permissions(0o755),
                    )
                    .unwrap_or_else(|error| panic!("failed to add fixture ZIP member: {error}"));
                archive
                    .write_all(bytes)
                    .unwrap_or_else(|error| panic!("failed to write fixture ZIP member: {error}"));
            }
            archive
                .finish()
                .unwrap_or_else(|error| panic!("failed to finish fixture ZIP: {error}"))
                .into_inner()
        }
    }

    fn stage_package(updater: &Updater, version: &str, bytes: &[u8]) {
        let target = PlatformTarget::detect()
            .unwrap_or_else(|error| panic!("detect fixture target failed: {error}"));
        let artifact_name = artifact_names_for(version, target)
            .unwrap_or_else(|error| panic!("fixture artifact name failed: {error}"))[0]
            .clone();
        let artifact_path = updater
            .root
            .join("staging")
            .join(version)
            .join(&artifact_name);
        fs::create_dir_all(
            artifact_path
                .parent()
                .unwrap_or_else(|| panic!("artifact parent missing")),
        )
        .unwrap_or_else(|error| panic!("failed to create stage fixture: {error}"));
        fs::write(&artifact_path, bytes)
            .unwrap_or_else(|error| panic!("failed to write archive fixture: {error}"));
        let staged = StagedRelease {
            version: version.to_owned(),
            tag_name: format!("v{version}"),
            artifact_name,
            artifact_path: artifact_path.clone(),
            sha256: sha256_file(&artifact_path)
                .unwrap_or_else(|error| panic!("hash failed: {error}")),
            target,
            staged_unix_ms: now_unix_ms(),
        };
        updater
            .persist_staged(&staged)
            .unwrap_or_else(|error| panic!("failed to persist staged fixture: {error}"));
    }

    fn make_fixture_executable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))
                .unwrap_or_else(|error| panic!("failed to mark fixture executable: {error}"));
        }
        #[cfg(windows)]
        let _ = path;
    }

    fn current_version(root: &Path) -> String {
        vexa_boot::load_current_manifest(root)
            .unwrap_or_else(|error| panic!("current manifest load failed: {error}"))
            .version
    }

    fn linux_glibc() -> PlatformTarget {
        PlatformTarget {
            os: OperatingSystem::Linux,
            arch: Architecture::X86_64,
            libc: Some(LinuxLibc::Glibc),
        }
    }

    fn available_update(version: &str) -> AvailableUpdate {
        let release = ReleaseMetadata::from_github_json(RELEASE_FIXTURE)
            .unwrap_or_else(|error| panic!("fixture release must parse: {error}"));
        let mut release = ReleaseMetadata {
            version: version.to_owned(),
            tag_name: format!("v{version}"),
            ..release
        };
        let artifact = select_artifact(&release, linux_glibc())
            .unwrap_or_else(|error| panic!("fixture artifact must select: {error}"))
            .clone();
        release.assets.retain(|asset| asset.name != artifact.name);
        release.assets.push(ReleaseAsset {
            size: ARTIFACT_FIXTURE.len() as u64,
            ..artifact.clone()
        });
        let artifact = select_artifact(&release, linux_glibc())
            .unwrap_or_else(|error| panic!("fixture artifact must reselect: {error}"))
            .clone();
        let checksums_asset = release
            .checksums_asset()
            .unwrap_or_else(|| panic!("fixture requires SHA256SUMS"))
            .clone();
        AvailableUpdate {
            release,
            artifact,
            checksums_asset,
            target: linux_glibc(),
        }
    }

    #[test]
    fn semantic_version_precedence_matches_release_rules() {
        assert!(is_version_newer("1.2.3", "v1.2.4").unwrap_or(false));
        assert!(is_version_newer("1.2.3-beta.2", "1.2.3").unwrap_or(false));
        assert!(is_version_newer("1.2.3-beta.2", "1.2.3-beta.11").unwrap_or(false));
        assert!(!is_version_newer("1.2.3", "1.2.3+build.9").unwrap_or(true));
        assert!(matches!(
            is_version_newer("1.2", "1.3.0"),
            Err(UpdateError::InvalidVersion(_))
        ));
    }

    #[test]
    fn release_asset_urls_and_redirects_are_host_and_path_pinned() {
        let valid =
            format!("https://github.com/{OFFICIAL_REPOSITORY}/releases/download/v1.2.3/SHA256SUMS");
        assert!(validate_release_asset_url("v1.2.3", "SHA256SUMS", &valid).is_ok());
        for invalid in [
            "http://github.com/bebabinlarsson-blip/VEXA/releases/download/v1.2.3/SHA256SUMS",
            "https://github.com.attacker.invalid/bebabinlarsson-blip/VEXA/releases/download/v1.2.3/SHA256SUMS",
            "https://github.com/other/repo/releases/download/v1.2.3/SHA256SUMS",
            "https://github.com/bebabinlarsson-blip/VEXA/releases/download/v1.2.3/../evil",
        ] {
            assert!(
                validate_release_asset_url("v1.2.3", "SHA256SUMS", invalid).is_err(),
                "accepted invalid asset URL: {invalid}"
            );
        }
        assert!(
            validate_redirect_url(
                "https://release-assets.githubusercontent.com/release/vexa?token=opaque"
            )
            .is_ok()
        );
        for invalid in [
            "http://release-assets.githubusercontent.com/release",
            "https://release-assets.githubusercontent.com.attacker.invalid/release",
            "https://github.com.attacker.invalid/bebabinlarsson-blip/VEXA/releases/download/v1.2.3/a",
            "https://user@release-assets.githubusercontent.com/release",
            "/relative/redirect",
        ] {
            assert!(
                matches!(
                    validate_redirect_url(invalid),
                    Err(UpdateError::UnsafeRedirect(_))
                ),
                "accepted unsafe redirect: {invalid}"
            );
        }
    }

    #[test]
    fn archive_member_validation_rejects_traversal_and_nested_paths() {
        let allowed = ["vexa-core", "LICENSE", "THIRD_PARTY_LICENSES.txt"];
        assert_eq!(
            package_member("vexa-1.2.3/vexa-core", "vexa-1.2.3", &allowed, false).ok(),
            Some(Some("vexa-core".to_owned()))
        );
        assert_eq!(
            package_member("vexa-1.2.3/", "vexa-1.2.3", &allowed, true).ok(),
            Some(None)
        );
        for path in [
            "../vexa-core",
            "vexa-1.2.3/../evil",
            "vexa-1.2.3/nested/vexa-core",
            "vexa-1.2.3/./vexa-core",
            "vexa-1.2.3\u{5c}..\u{5c}evil",
        ] {
            assert!(
                package_member(path, "vexa-1.2.3", &allowed, false).is_err(),
                "accepted: {path}"
            );
        }
    }

    #[test]
    fn lifecycle_lock_excludes_a_second_updater_and_releases_on_drop() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let first = Updater::new(temp.path());
        let second = Updater::new(temp.path());
        let lock = first
            .acquire_lifecycle_lock()
            .unwrap_or_else(|error| panic!("lock failed: {error}"));
        assert!(matches!(
            second.acquire_lifecycle_lock(),
            Err(UpdateError::OperationInProgress)
        ));
        drop(lock);
        assert!(second.acquire_lifecycle_lock().is_ok());
    }

    #[test]
    fn release_fixture_selects_each_supported_target() {
        let release = ReleaseMetadata::from_github_json(RELEASE_FIXTURE)
            .unwrap_or_else(|error| panic!("fixture release must parse: {error}"));
        let cases = [
            (
                PlatformTarget {
                    os: OperatingSystem::Windows,
                    arch: Architecture::X86_64,
                    libc: None,
                },
                "vexa-x86_64-pc-windows-msvc.zip",
            ),
            (linux_glibc(), "vexa-x86_64-unknown-linux-gnu.tar.gz"),
            (
                PlatformTarget {
                    os: OperatingSystem::Linux,
                    arch: Architecture::X86_64,
                    libc: Some(LinuxLibc::Musl),
                },
                "vexa-x86_64-unknown-linux-musl.tar.gz",
            ),
            (
                PlatformTarget {
                    os: OperatingSystem::Linux,
                    arch: Architecture::Aarch64,
                    libc: Some(LinuxLibc::Glibc),
                },
                "vexa-aarch64-unknown-linux-gnu.tar.gz",
            ),
            (
                PlatformTarget {
                    os: OperatingSystem::Linux,
                    arch: Architecture::Aarch64,
                    libc: Some(LinuxLibc::Musl),
                },
                "vexa-aarch64-unknown-linux-musl.tar.gz",
            ),
        ];
        for (target, expected) in cases {
            let selected = select_artifact(&release, target)
                .unwrap_or_else(|error| panic!("target {target} must select: {error}"));
            assert_eq!(selected.name, expected);
        }

        let arm_glibc = PlatformTarget {
            os: OperatingSystem::Linux,
            arch: Architecture::Aarch64,
            libc: Some(LinuxLibc::Glibc),
        };
        let mut musl_only_arm_release = release.clone();
        musl_only_arm_release
            .assets
            .retain(|asset| asset.name != "vexa-aarch64-unknown-linux-gnu.tar.gz");
        assert_eq!(
            select_artifact(&musl_only_arm_release, arm_glibc)
                .unwrap_or_else(|error| panic!(
                    "AArch64 glibc may use shipped static musl: {error}"
                ))
                .name,
            "vexa-aarch64-unknown-linux-musl.tar.gz"
        );

        let mut x86_musl_only_release = release.clone();
        x86_musl_only_release
            .assets
            .retain(|asset| asset.name != "vexa-x86_64-unknown-linux-gnu.tar.gz");
        assert!(matches!(
            select_artifact(&x86_musl_only_release, linux_glibc()),
            Err(UpdateError::NoMatchingArtifact(_))
        ));

        let unknown_libc = PlatformTarget {
            os: OperatingSystem::Linux,
            arch: Architecture::X86_64,
            libc: None,
        };
        assert!(matches!(
            select_artifact(&release, unknown_libc),
            Err(UpdateError::UnsupportedTarget(_))
        ));
    }

    #[test]
    fn checksum_fixture_parses_and_matches_artifact() {
        let checksums = parse_sha256sums(CHECKSUMS_FIXTURE)
            .unwrap_or_else(|error| panic!("fixture checksums must parse: {error}"));
        let expected = checksums
            .get("vexa-x86_64-unknown-linux-gnu.tar.gz")
            .unwrap_or_else(|| panic!("fixture artifact checksum missing"));
        let actual = hex_digest(Sha256::digest(ARTIFACT_FIXTURE).as_slice());
        assert_eq!(expected, &actual);
    }

    #[test]
    fn staging_verifies_before_finalizing_and_preserves_rollback_metadata() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let updater = Updater::new(temp.path());
        let expected =
            checksum_for_asset(CHECKSUMS_FIXTURE, "vexa-x86_64-unknown-linux-gnu.tar.gz")
                .unwrap_or_else(|error| panic!("fixture checksum lookup failed: {error}"));

        let first = available_update("1.2.3");
        let staged = updater
            .stage_from_reader(&first, &expected, Cursor::new(ARTIFACT_FIXTURE))
            .unwrap_or_else(|error| panic!("first staging failed: {error}"));
        assert!(staged.artifact_path.exists());
        let first_state = updater
            .record_activation("1.2.3", &staged.artifact_path)
            .unwrap_or_else(|error| panic!("first activation metadata failed: {error}"));
        assert_eq!(
            first_state
                .current
                .as_ref()
                .map(|entry| entry.version.as_str()),
            Some("1.2.3")
        );
        assert!(first_state.previous.is_none());

        let second = available_update("1.3.0");
        let staged = updater
            .stage_from_reader(&second, &expected, Cursor::new(ARTIFACT_FIXTURE))
            .unwrap_or_else(|error| panic!("second staging failed: {error}"));
        updater
            .record_activation("1.3.0", &staged.artifact_path)
            .unwrap_or_else(|error| panic!("second activation metadata failed: {error}"));
        let rollback = updater
            .rollback_metadata()
            .unwrap_or_else(|error| panic!("rollback metadata failed: {error}"))
            .unwrap_or_else(|| panic!("rollback metadata should exist"));
        assert_eq!(rollback.from_version, "1.3.0");
        assert_eq!(rollback.to_version, "1.2.3");
    }

    #[test]
    fn checksum_failure_never_creates_final_artifact() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let updater = Updater::new(temp.path());
        let update = available_update("1.2.3");
        let error = updater
            .stage_from_reader(
                &update,
                "0000000000000000000000000000000000000000000000000000000000000000",
                Cursor::new(ARTIFACT_FIXTURE),
            )
            .err()
            .unwrap_or_else(|| panic!("checksum mismatch expected"));
        assert!(matches!(error, UpdateError::ChecksumMismatch { .. }));
        let final_path = temp
            .path()
            .join("staging")
            .join("1.2.3")
            .join("vexa-x86_64-unknown-linux-gnu.tar.gz");
        assert!(!final_path.exists());
    }

    #[test]
    fn installer_activation_retains_rollback_across_reinstall_and_rejects_failed_health() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let root = temp.path();
        let updater = Updater::new(root.join("updates"));
        for version in ["0.9.0", "1.0.0", "1.1.0"] {
            let directory = root.join("versions").join(version);
            fs::create_dir_all(&directory)
                .unwrap_or_else(|error| panic!("create version failed: {error}"));
            let core = directory.join(core_name());
            fs::write(&core, version).unwrap_or_else(|error| panic!("write core failed: {error}"));
            make_fixture_executable(&core);
        }
        for version in ["0.9.0", "1.0.0", "1.0.0"] {
            updater
                .activate_installed_inner(version, |_, _| Ok(()))
                .unwrap_or_else(|error| panic!("installer activation failed: {error}"));
        }
        assert!(
            updater
                .activate_installed_inner("../0.9.0", |_, _| Ok(()))
                .is_err()
        );
        assert!(
            updater
                .activate_installed_inner("1.1.0", |_, _| {
                    Err(UpdateError::HealthCheckFailed("fixture failure".to_owned()))
                })
                .is_err()
        );
        assert_eq!(current_version(root), "1.0.0");
        updater
            .rollback()
            .unwrap_or_else(|error| panic!("rollback failed: {error}"));
        assert_eq!(current_version(root), "0.9.0");
        updater
            .rollback()
            .unwrap_or_else(|error| panic!("rollback forward failed: {error}"));
        assert_eq!(current_version(root), "1.0.0");
    }

    #[test]
    fn activation_health_checks_switches_manifest_and_rolls_back_without_deleting_versions() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let install_root = temp.path();
        let updater = Updater::new(install_root.join("updates"));
        let old_dir = install_root.join("versions").join("0.9.0");
        fs::create_dir_all(&old_dir)
            .unwrap_or_else(|error| panic!("old version dir failed: {error}"));
        let old_core = old_dir.join(core_name());
        fs::write(&old_core, b"old working core")
            .unwrap_or_else(|error| panic!("old core failed: {error}"));
        make_fixture_executable(&old_core);
        vexa_boot::activate_version(install_root, "0.9.0")
            .unwrap_or_else(|error| panic!("old selection failed: {error}"));

        let bytes = package_archive("1.0.0", b"#!/bin/sh\nprintf 'health-check output leak'\nprintf 'stderr leak' >&2\n[ \"$1\" = doctor ]\n");
        stage_package(&updater, "1.0.0", &bytes);
        let activation = if cfg!(unix) {
            updater.activate_staged()
        } else {
            updater.activate_staged_with(|core, root| {
                assert_eq!(
                    core.file_name().and_then(|name| name.to_str()),
                    Some(core_name())
                );
                assert_eq!(root, install_root);
                Ok(())
            })
        };
        let state = activation.unwrap_or_else(|error| panic!("activation failed: {error}"));
        assert_eq!(
            state
                .current
                .as_ref()
                .map(|version| version.version.as_str()),
            Some("1.0.0")
        );
        assert_eq!(current_version(install_root), "1.0.0");
        assert!(old_dir.join(core_name()).exists());
        assert!(
            install_root
                .join("versions/1.0.0")
                .join(core_name())
                .exists()
        );

        let current_core = install_root.join("versions/1.0.0").join(core_name());
        let current_bytes = fs::read(&current_core)
            .unwrap_or_else(|error| panic!("failed to read current core fixture: {error}"));
        fs::write(&current_core, b"tampered current core")
            .unwrap_or_else(|error| panic!("failed to tamper current core: {error}"));
        assert!(matches!(
            updater.rollback(),
            Err(UpdateError::ChecksumMismatch { .. })
        ));
        assert_eq!(current_version(install_root), "1.0.0");
        fs::write(&current_core, &current_bytes)
            .unwrap_or_else(|error| panic!("failed to restore current core fixture: {error}"));

        let previous_core = old_dir.join(core_name());
        fs::write(&previous_core, b"tampered previous core")
            .unwrap_or_else(|error| panic!("failed to tamper previous core: {error}"));
        assert!(matches!(
            updater.rollback(),
            Err(UpdateError::ChecksumMismatch { .. })
        ));
        assert_eq!(current_version(install_root), "1.0.0");
        fs::write(&previous_core, b"old working core")
            .unwrap_or_else(|error| panic!("failed to restore previous core fixture: {error}"));
        make_fixture_executable(&previous_core);

        let state = updater
            .rollback()
            .unwrap_or_else(|error| panic!("rollback failed: {error}"));
        assert_eq!(
            state
                .current
                .as_ref()
                .map(|version| version.version.as_str()),
            Some("0.9.0")
        );
        assert_eq!(current_version(install_root), "0.9.0");
        assert!(
            install_root
                .join("versions/1.0.0")
                .join(core_name())
                .exists()
        );
    }

    #[test]
    fn failed_core_health_check_keeps_previous_manifest_selected() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let install_root = temp.path();
        let updater = Updater::new(install_root.join("updates"));
        let old_dir = install_root.join("versions/0.9.0");
        fs::create_dir_all(&old_dir)
            .unwrap_or_else(|error| panic!("old version dir failed: {error}"));
        let old_core = old_dir.join(core_name());
        fs::write(&old_core, b"old working core")
            .unwrap_or_else(|error| panic!("old core failed: {error}"));
        make_fixture_executable(&old_core);
        vexa_boot::activate_version(install_root, "0.9.0")
            .unwrap_or_else(|error| panic!("old selection failed: {error}"));
        let bytes = package_archive("1.0.0", b"#!/bin/sh\nexit 7\n");
        stage_package(&updater, "1.0.0", &bytes);
        let activation = if cfg!(unix) {
            updater.activate_staged()
        } else {
            updater.activate_staged_with(|_, _| {
                Err(UpdateError::HealthCheckFailed(
                    "fixture rejection".to_owned(),
                ))
            })
        };
        assert!(matches!(activation, Err(UpdateError::HealthCheckFailed(_))));
        assert_eq!(current_version(install_root), "0.9.0");
        assert!(!install_root.join("versions/1.0.0").exists());
    }

    #[test]
    fn first_install_has_no_rollback_target() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let install_root = temp.path();
        let updater = Updater::new(install_root.join("updates"));
        let bytes = package_archive("1.0.0", b"benign core fixture");
        stage_package(&updater, "1.0.0", &bytes);
        updater
            .activate_staged_with(|_, _| Ok(()))
            .unwrap_or_else(|error| panic!("first activation failed: {error}"));
        assert!(matches!(
            updater.rollback(),
            Err(UpdateError::NoRollbackAvailable)
        ));
        assert_eq!(current_version(install_root), "1.0.0");
    }
}
