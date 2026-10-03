pub fn normalize_workspace_key(input: &str) -> String {
    input
        .trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

pub fn workspace_key_matches(left: &str, right: &str) -> bool {
    normalize_workspace_key(left) == normalize_workspace_key(right)
}

