# Harness full readiness implementation plan

> **For agentic workers:** execution is already authorized by the user. Independent agents own separate subsystems; the coordinator integrates and verifies the complete tree.

**Goal:** Implement missing runtime contracts and establish an evidence-based verdict against the existing full release criteria.

**Architecture:** Keep one Rust coordinator and SQLite source of truth. Extend command supervision, bounded scoped context caching, durable outbox delivery and historical replay. A separate trusted benchmark controller evaluates frozen exact commits; development fixtures never become production evidence.

**Tech Stack:** Rust 1.99.0, Tokio, SQLite, Git, platform process controls; Python for trusted evaluation orchestration.

**Spec:** [Architecture](../../ARCHITECTURE_PLAN.md), [evaluation](../../EVALUATION_PLAN.md), [131 mandatory criteria](../../../evals/scenarios.json).

## Global constraints

- Preserve grants, exact action/candidate binding, original budgets and unknown-effect HOLD.
- No silent native downgrade; unsupported requested guarantees fail before dispatch.
- Per-process limits do not imply aggregate limits; real OS probes determine availability.
- Full release requires 20 dev and 10 holdout tasks, three fresh trials and B0/B1/H comparison; public toy pilot is separate evidence.
- Linux/macOS/Windows execution evidence is mandatory for those supported platforms; cross-compilation alone is insufficient.
- Existing 0.1.2 reports and archives remain unchanged.

## Review focus

- Limits inherited by descendants and risk binding cannot be widened by explicit command fields.
- Dirty/new/revoked sources and different roles cannot reuse context cache entries.
- Lost responses and expired workers cannot repeat uncertain non-idempotent effects.
- Historical replay cannot authorize fresh verification or launch effects/model calls.
- Missing platform, holdout, calibration or resource evidence cannot become a ready verdict.

## Tasks

1. Audit each mandatory criterion and probe available execution environments; save a machine-readable matrix with exact evidence and blockers.
2. Add optional command resource limits, trusted supervisor enforcement and explicit capability failures. Run safe real-process RED/GREEN tests.
3. Add bounded scoped context cache and raw-source evidence expansion; test dirty input, grant revocation, redaction and cross-role/project separation.
4. Add fenced durable outbox leases, idempotent retry and uncertain-effect reconciliation; test real local durable effects across lost replies/reopen.
5. Add historical replay and a strict full-readiness evaluator/CLI; test forged, stale, partial or incompatible receipts and zero effect dispatch.
6. Build 30 independent frozen benchmark fixtures, known-valid controls and external oracles; validate baseline FAIL/control PASS before real trials.
7. Run negative CLI and DAG/DecisionService scenarios with trap counters. Preserve actual failing receipts before fixing the underlying production code.
8. Integrate subsystem APIs, run full fmt/clippy/tests/release, independent review and real pilot on the new frozen binary.
9. Execute full required comparisons where the environment supports them; record unavailable dependencies as BLOCKED rather than PASS. Update readiness report and package reviewed source/evidence.

Each production change begins with a failing behavior test. Final verification runs once the shared tree is stable; repeated targeted tests are not counted as additional unique tests.
