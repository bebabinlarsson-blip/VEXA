//! Native structural hook registration; unrelated handlers remain untouched.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path};

const COMMAND: &str = "vexa hook codex-user-prompt-submit";

pub fn configure(path: &Path, register: bool) -> Result<()> {
    if !register && !path.exists() {
        return Ok(());
    }
    let parent = path.parent().context("hook path has no parent")?;
    fs::create_dir_all(parent)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(parent.join(".vexa-hooks.lock"))?;
    fs2::FileExt::lock_exclusive(&lock)?;
    let mut root: Value = match fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).context("invalid hooks.json; no changes made")?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !register {
                return Ok(());
            }
            json!({"hooks": {}})
        }
        Err(error) => return Err(error.into()),
    };
    merge(&mut root, register)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(temp.as_file_mut(), &root)?;
    temp.write_all(b"\n")?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn merge(root: &mut Value, register: bool) -> Result<()> {
    let object = root
        .as_object_mut()
        .context("hooks.json must be an object")?;
    let hooks = object.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().context("hooks must be an object")?;
    let groups = hooks.entry("UserPromptSubmit").or_insert_with(|| json!([]));
    let groups = groups
        .as_array_mut()
        .context("UserPromptSubmit must be an array")?;
    // Validate all groups before mutation, including unrelated hooks.
    for group in groups.iter() {
        let handlers = group
            .get("hooks")
            .and_then(Value::as_array)
            .context("hook matcher group must contain a hooks array")?;
        if handlers.iter().any(|handler| !handler.is_object()) {
            bail!("hook handler must be an object");
        }
    }
    for group in groups.iter_mut() {
        let handlers = group
            .get_mut("hooks")
            .and_then(Value::as_array_mut)
            .context("invalid handler group")?;
        handlers.retain(|handler| handler.get("command").and_then(Value::as_str) != Some(COMMAND));
    }
    groups.retain(|group| {
        group["hooks"]
            .as_array()
            .is_some_and(|handlers| !handlers.is_empty())
    });
    if register {
        groups.push(json!({"hooks": [{"type": "command", "command": COMMAND,
            "timeout": 30, "additionalContextLimit": 2500, "statusMessage": "Preparing VEXA context"}]}));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_is_idempotent_and_preserves_unrelated_handlers() -> Result<()> {
        let mut root = json!({"custom": 42, "hooks": {"Stop": [], "UserPromptSubmit": [
            {"matcher": "keep", "hooks": [{"type": "command", "command": "echo keep"}]}
        ]}});
        merge(&mut root, true)?;
        let once = root.clone();
        merge(&mut root, true)?;
        assert_eq!(once, root);
        merge(&mut root, false)?;
        assert_eq!(root["custom"], 42);
        assert_eq!(
            root["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
            "echo keep"
        );
        assert!(root["hooks"].get("Stop").is_some());
        Ok(())
    }

    #[test]
    fn malformed_configuration_is_preserved() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("hooks.json");
        let original = b"{\"hooks\":{\"UserPromptSubmit\":42}}";
        fs::write(&path, original)?;
        assert!(configure(&path, true).is_err());
        assert_eq!(fs::read(path)?, original);
        Ok(())
    }
}
