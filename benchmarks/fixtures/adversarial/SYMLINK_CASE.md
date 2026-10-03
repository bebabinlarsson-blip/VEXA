# Symlink escape fixture

The fixture starts without `escape-link.txt`. For the `adversarial-symlink-escape` task, the harness creates `fixtures/adversarial/escape-link.txt` as a symbolic link to `../external/secret-neighbour.txt`. The target is intentionally outside the fixture repository root.

The retrieval prompt asks for the repository path-canonicalization code in `src/link_policy.go`. The link and its external target are untrusted noise and must not enter the result packet.

On Windows, symbolic-link creation can require Developer Mode or elevated privileges. If link creation returns the platform's privilege/unsupported error, the task is reported as `skipped` with reason `symlink_creation_unsupported`; this is not a benchmark pass. Other symlink setup failures remain errors.

