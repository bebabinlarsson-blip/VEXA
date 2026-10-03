# Routing (Laya)

Laya (`typed-decisions` checkpoint) is the only local router. Its
`LOCAL_LAYA_ROUTER` advisory chooses tier, execution shape, context budget,
and risk. It is advisory; tests, compilers, runtime behavior, and Git are
authoritative.

NO HAIKU. Never configure, recommend, or intentionally invoke Haiku.

## Tiers

| Tier | Use for | Codex | Claude Code | Role |
|---|---|---|---|---|
| JUNIOR | narrow search, mechanical, low-risk | GPT-6 Luna, Low | Sonnet 5.5, Low | benji-junior |
| MIDDLE | normal implementation | GPT-6 Luna, Medium | Sonnet 5.5, Medium | main root / benji-middle |
| SENIOR | difficult reasoning | GPT-6.1 Sol, Medium | Opus 5.5, Medium | benji-senior |
| GOAT | critical / high-risk | GPT-6.1 Sol, High | Opus 5.5, XHigh | benji-goat |

## Per unit of work

1. Read the Laya advisory if present.
2. Classify actual scope and risk.
3. Pick the cheapest capable tier.
4. Decide whether delegation adds value.
5. Do the work.
6. Run deterministic verification (see VERIFY.md).
7. Escalate only the difficult unresolved boundary.

Examples: find implementation / trace a reference → JUNIOR. Endpoint, UI
feature, ordinary bug, normal multi-file feature → MIDDLE. Cross-module root
cause, major architecture decision → SENIOR. Security-sensitive design,
dangerous concurrent migration, data-integrity-critical operation → GOAT.

Never classify a whole repository as GOAT because it is large. Split large work
(architecture/contracts, backend, frontend, storage, networking, tests,
build/release, performance, security, docs) and classify each part. Most work
stays MIDDLE.

## Execution and parallelism

`DIRECT`, `ONE_WORKER`, `PARALLEL_2` are ceilings, not quotas.
Maximum normal parallel workers: **2**. Parallelize only independent scopes.
Avoid duplicate exploration, workers editing the same files, and several
expensive specialists on one issue.

## Context budget (Codex delegation)

MINIMAL → `fork_turns = "1"`; RECENT → `"2"`; EXTENDED → `"4"`;
ROOT_ONLY → do not delegate. Avoid full-history forks.

## Senior / GOAT handoff packet

OBJECTIVE · DIFFICULT REMAINING QUESTION · RELEVANT FILES · RELEVANT
ARCHITECTURE · EVIDENCE · WHAT MIDDLE ALREADY TRIED · EXACT FAILURE ·
CONSTRAINTS · EXPECTED RETURN

The specialist solves that boundary without redoing discovery. Afterwards,
implementation returns to MIDDLE.
