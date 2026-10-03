# Ownership

- Every implementation worker owns explicit files or a subsystem.
- Two agents do not concurrently modify the same files without deliberate coordination.
- Read-only investigation may overlap.
- Git status and diff determine what changed.
- Handoffs report changed paths, checks, and limitations.
- The root integrates unresolved conflicts and reports completion.
