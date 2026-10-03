//! Durable, provenance-aware project memory.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use vexa_types::{MemoryFact, ProjectMemory, VerificationState};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Errors produced while loading or persisting project memory.
#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("failed to access memory at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("memory JSON at {path} is invalid: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("memory path has no file name: {0}")]
    InvalidPath(PathBuf),
}

/// A compact, serializable view of what the memory currently contains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryInspection {
    pub facts: usize,
    pub verified: usize,
    pub stale: usize,
    pub unverified: usize,
    pub contradicted: usize,
    pub unresolved: usize,
    pub keys: Vec<String>,
}

/// Records which facts changed verification state during invalidation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvalidationReport {
    pub verified: Vec<String>,
    pub stale: Vec<String>,
    pub unknown: Vec<String>,
    pub contradicted: Vec<String>,
}

/// Loads project memory. A missing file is treated as empty memory.
///
/// # Errors
///
/// Returns [`MemoryError`] when an existing file cannot be read or contains invalid JSON.
pub fn load(path: &Path) -> Result<ProjectMemory, MemoryError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| MemoryError::Json {
            path: path.to_path_buf(),
            source,
        }),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(ProjectMemory::default()),
        Err(source) => Err(MemoryError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Persists project memory with a same-directory atomic replacement.
///
/// # Errors
///
/// Returns [`MemoryError`] when serialization, directory creation, writing, syncing,
/// or the final atomic replacement fails.
pub fn save(path: &Path, memory: &ProjectMemory) -> Result<(), MemoryError> {
    let bytes = serde_json::to_vec_pretty(memory).map_err(|source| MemoryError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| MemoryError::Io {
        path: parent.to_path_buf(),
        source,
    })?;

    let (temp_path, mut temp_file) = create_temp_file(path, parent)?;
    let write_result = (|| -> io::Result<()> {
        temp_file.write_all(&bytes)?;
        temp_file.write_all(b"\n")?;
        temp_file.flush()?;
        temp_file.sync_all()
    })();
    if let Err(source) = write_result {
        drop(temp_file);
        let _ = fs::remove_file(&temp_path);
        return Err(MemoryError::Io {
            path: temp_path,
            source,
        });
    }
    drop(temp_file);

    if let Err(source) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(MemoryError::Io {
            path: path.to_path_buf(),
            source,
        });
    }

    // Directory syncing is supported on Unix and may fail for directory handles on
    // Windows. The file replacement itself has already succeeded, so this is best-effort.
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

/// Deletes persisted memory. Returns `true` when a file was removed.
///
/// # Errors
///
/// Returns [`MemoryError`] when the memory file exists but cannot be removed.
pub fn clear(path: &Path) -> Result<bool, MemoryError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(MemoryError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Adds or replaces a fact by key and keeps fact order deterministic.
pub fn add(memory: &mut ProjectMemory, fact: MemoryFact) {
    memory.facts.retain(|existing| existing.key != fact.key);
    memory.facts.push(fact);
    memory.facts.sort_by(|left, right| left.key.cmp(&right.key));
}

/// Returns a deterministic summary suitable for CLI or machine output.
#[must_use]
pub fn inspect(memory: &ProjectMemory) -> MemoryInspection {
    let mut inspection = MemoryInspection {
        facts: memory.facts.len(),
        verified: 0,
        stale: 0,
        unverified: 0,
        contradicted: 0,
        unresolved: memory.unresolved.len(),
        keys: memory.facts.iter().map(|fact| fact.key.clone()).collect(),
    };

    inspection.keys.sort();
    inspection.keys.dedup();
    for fact in &memory.facts {
        match fact.source.verification {
            VerificationState::Verified => inspection.verified += 1,
            VerificationState::Stale => inspection.stale += 1,
            VerificationState::Unverified => inspection.unverified += 1,
            VerificationState::Contradicted => inspection.contradicted += 1,
        }
    }
    inspection
}

/// Re-evaluates facts against a current revision and a live/indexed path-to-hash map.
///
/// Revision-linked facts become stale when their stored revision differs. Facts with
/// a known path hash become stale on hash mismatch and verified on an exact match.
/// When no current hash is available, the result is explicitly unverified.
#[must_use]
pub fn invalidate(
    memory: &mut ProjectMemory,
    current_revision: Option<&str>,
    current_hashes: &BTreeMap<String, String>,
) -> InvalidationReport {
    let mut report = InvalidationReport::default();

    for fact in &mut memory.facts {
        let revision_stale = current_revision.is_some_and(|revision| {
            fact.revision
                .as_deref()
                .is_some_and(|stored| stored != revision)
                || fact
                    .source
                    .git_revision
                    .as_deref()
                    .is_some_and(|stored| stored != revision)
        });
        let current_hash = current_hashes.get(&fact.source.path);
        let hash_stale = current_hash.is_some_and(|hash| hash != &fact.source.sha256);

        fact.source.verification = if fact.source.verification == VerificationState::Contradicted {
            VerificationState::Contradicted
        } else if revision_stale || hash_stale {
            VerificationState::Stale
        } else if current_hash.is_some()
            && fact.source.trust != vexa_types::TrustLevel::UserProvided
        {
            VerificationState::Verified
        } else {
            VerificationState::Unverified
        };

        match fact.source.verification {
            VerificationState::Verified => report.verified.push(fact.key.clone()),
            VerificationState::Stale => report.stale.push(fact.key.clone()),
            VerificationState::Unverified => report.unknown.push(fact.key.clone()),
            VerificationState::Contradicted => report.contradicted.push(fact.key.clone()),
        }
    }

    memory.revision = current_revision.map(str::to_owned);
    report
}

fn create_temp_file(path: &Path, parent: &Path) -> Result<(PathBuf, File), MemoryError> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| MemoryError::InvalidPath(path.to_path_buf()))?;
    let process_id = std::process::id();

    for _ in 0..1024 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(".{file_name}.{process_id}.{sequence}.tmp"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(MemoryError::Io {
                    path: temp_path,
                    source,
                });
            }
        }
    }

    Err(MemoryError::Io {
        path: path.to_path_buf(),
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique atomic-write temporary file",
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;
    use vexa_types::{Provenance, TrustLevel};

    fn fact(key: &str, hash: &str, revision: Option<&str>) -> MemoryFact {
        MemoryFact {
            key: key.to_owned(),
            value: json!({"answer": 42}),
            source: Provenance {
                path: "src/lib.rs".to_owned(),
                start_line: 1,
                end_line: 3,
                sha256: hash.to_owned(),
                git_revision: revision.map(str::to_owned),
                observed_unix_ms: 1,
                trust: TrustLevel::VerifiedRepositoryEvidence,
                verification: VerificationState::Verified,
            },
            revision: revision.map(str::to_owned),
            created_unix_ms: 1,
        }
    }

    #[test]
    fn round_trips_atomically_and_clears() {
        let directory = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = directory.path().join("nested").join("memory.json");
        let mut memory = ProjectMemory::default();
        add(&mut memory, fact("answer", "abc", Some("r1")));

        save(&path, &memory).unwrap_or_else(|error| panic!("save failed: {error}"));
        add(&mut memory, fact("second", "def", Some("r1")));
        save(&path, &memory).unwrap_or_else(|error| panic!("replace save failed: {error}"));
        let loaded = load(&path).unwrap_or_else(|error| panic!("load failed: {error}"));
        assert_eq!(loaded.facts.len(), 2);
        assert_eq!(loaded.facts[0].key, "answer");
        assert!(clear(&path).unwrap_or_else(|error| panic!("clear failed: {error}")));
        assert!(!clear(&path).unwrap_or_else(|error| panic!("second clear failed: {error}")));
        assert!(
            load(&path)
                .unwrap_or_else(|error| panic!("empty load failed: {error}"))
                .facts
                .is_empty()
        );
    }

    #[test]
    fn add_replaces_duplicate_keys_and_inspection_is_stable() {
        let mut memory = ProjectMemory::default();
        add(&mut memory, fact("zeta", "old", Some("r1")));
        add(&mut memory, fact("alpha", "a", Some("r1")));
        add(&mut memory, fact("zeta", "new", Some("r1")));

        let inspection = inspect(&memory);
        assert_eq!(memory.facts.len(), 2);
        assert_eq!(memory.facts[0].key, "alpha");
        assert_eq!(memory.facts[1].source.sha256, "new");
        assert_eq!(inspection.keys, vec!["alpha", "zeta"]);
        assert_eq!(inspection.verified, 2);
    }

    #[test]
    fn adversarial_revision_change_marks_fact_stale() {
        let mut memory = ProjectMemory::default();
        add(
            &mut memory,
            fact("architecture", "same-hash", Some("old-rev")),
        );
        let hashes = BTreeMap::from([("src/lib.rs".to_owned(), "same-hash".to_owned())]);

        let report = invalidate(&mut memory, Some("new-rev"), &hashes);
        assert_eq!(
            memory.facts[0].source.verification,
            VerificationState::Stale
        );
        assert_eq!(report.stale, vec!["architecture"]);
    }

    #[test]
    fn adversarial_hash_change_marks_fact_stale() {
        let mut memory = ProjectMemory::default();
        add(&mut memory, fact("entrypoint", "old-hash", Some("r1")));
        let hashes = BTreeMap::from([("src/lib.rs".to_owned(), "new-hash".to_owned())]);

        let report = invalidate(&mut memory, Some("r1"), &hashes);
        assert_eq!(
            memory.facts[0].source.verification,
            VerificationState::Stale
        );
        assert_eq!(report.stale, vec!["entrypoint"]);
    }

    #[test]
    fn missing_current_hash_downgrades_to_unverified() {
        let mut memory = ProjectMemory::default();
        add(&mut memory, fact("unknown", "hash", Some("r1")));

        let report = invalidate(&mut memory, Some("r1"), &BTreeMap::new());
        assert_eq!(
            memory.facts[0].source.verification,
            VerificationState::Unverified
        );
        assert_eq!(report.unknown, vec!["unknown"]);
    }

    #[test]
    fn matching_source_does_not_prove_a_user_assertion() {
        let mut memory = ProjectMemory::default();
        let mut assertion = fact("claim", "same", None);
        assertion.source.trust = TrustLevel::UserProvided;
        assertion.source.verification = VerificationState::Unverified;
        add(&mut memory, assertion);
        let hashes = BTreeMap::from([("src/lib.rs".to_owned(), "same".to_owned())]);
        let report = invalidate(&mut memory, None, &hashes);
        assert_eq!(
            memory.facts[0].source.verification,
            VerificationState::Unverified
        );
        assert_eq!(report.unknown, vec!["claim"]);
    }
}
