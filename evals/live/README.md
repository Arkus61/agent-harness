# Six curated live development tasks

This is a small repeatable pilot, not the 30-task benchmark or all 42 system
scenarios. Each task uses a no-dependency Rust library with a frozen baseline,
public complete requirements, a pinned `gpt-6.1-sol` model, and three fresh trials.
The trusted controller must clone each baseline with `--local --no-hardlinks`;
never seed another trial from a prior candidate.

`prepare.py --output DIR` creates committed seeds, freezes a manifest, and proves
that each baseline fails its oracle and each known valid control passes. The
controls are validation data and must not be copied into the agent's repository.

From the harness project root, with Rust, Cargo, Git, Python 3.12+ and authenticated
Codex on `PATH`, reproduce the complete preparation and run fresh trials:

```sh
python3 evals/live/prepare.py --output artifacts/fresh-seeds --manifest artifacts/fresh-manifest.json
python3 evals/live/run.py --manifest artifacts/fresh-manifest.json --output artifacts/fresh-live --repeats 3 --workers 2
```

Both output directories must be fresh. Preparation records separate baseline and
known-control JSON evidence, and the known control must pass both the external
oracle and the same `cargo test --locked --offline` check used by the harness.
The tracked `manifest.json` describes the original validation seeds under
`artifacts/full-validation/fixture-repos`; those Git repositories are generated
data rather than tracked templates. Reproduction generates a new manifest with
its own absolute seed paths. Keep task/oracle/controller hashes fixed throughout
one run. `--skip-existing` fails closed: historical receipts are retained for
inspection and cannot certify a fresh trial. Preparation also freezes the AST checker's Cargo.toml, Cargo.lock and
src/main.rs digest. Historical manifests without that field remain evidence;
generate a fresh manifest to run the current controller. The controls and oracle tests are curated for this pilot, not a hidden
holdout set.

`accept.py --task-id Q02 --repo REPO --candidate COMMIT --output RESULT.json`
extracts that exact commit using `git archive`. It compiles frozen external Rust
tests in a separate path-dependent acceptance crate, records the exact commit,
tree, task and oracle SHA256 hashes, and returns zero only on PASS. All dependencies
are local and Cargo runs with `--locked --offline`.

| ID | Class | Oracle |
|---|---|---|
| Q01 | New port parsing behavior | Accepted decimal boundaries and malformed inputs |
| Q02 | Clamp bug fix | Relative positions, equal bounds and extreme integers |
| Q03 | Behavior-preserving refactor | Shipping behavior plus explicit single-table structure |
| Q04 | Parallel components | Wide percent arithmetic, money formatting and composition |
| Q05 | Dependency integration | CSV parser, billing and malformed/overflow cases |
| Q06 | Test quality | Correct median behavior and three test-detected mutants |

Q03's baseline already has correct behavior. It fails the explicit structural
contract: a private `rate_table` must hold each region mapping once and
`shipping_rate` must delegate to it. This syntax-level check is intentionally
narrow and is not a general code-quality measure. The trusted standalone
[Rust AST checker](structure-check/README.md) uses `syn`, so placing the helper
before or after the public function has identical results. Its dependencies must
first be available in the Cargo cache; grading builds with `--locked --offline`,
records source and executable SHA256, and fails closed without a text fallback.
Run `python3 tests/evaluation_oracle.py -v` for its CLI/controller contracts.
Run `python3 tests/evaluation_manifest.py -v` to verify frozen-input rejection
before dispatch, including optimized Python and preservation of existing receipts.

The 0.1.2 original frozen receipts remain 15/18 PASS. The three Q03 failures were
caused by the old checker including following functions in `shipping_rate`'s
body. Separate supplemental receipts recheck the same commits with unchanged
requirements and behavior tests: corrected acceptance is 18/18. See
[amendment evidence](../../artifacts/contract-validation/oracle-amendment/summary.json).

Q06's baseline library is also correct. Its original smoke test cannot detect the
`return b` mutant. The external controller substitutes three predefined faulty
library implementations in temporary copies and runs the candidate's committed
integration tests. A mutant counts as killed only if test execution reports an
assertion failure; compilation errors do not count. Production source must remain
byte-for-byte unchanged in this task.

For Q01–Q05 the external oracle establishes the functional requirements and,
for Q03, the stated structural requirements. It does not independently establish
that the agent added sufficient regression tests: assess the exact candidate's
test changes separately, and record incomplete coverage when that evidence is
missing. Protected Cargo success and a model's tests review alone do not turn
that part into an independent acceptance result. Q06 provides the explicit
mutation-based check of test adequacy.

Q04 intentionally covers both maximal `u32` inputs. The intermediate product
`u32::MAX * (100 + u32::MAX)` exceeds `u64::MAX`, even though division by 100 fits
the returned `u64`; the public contract therefore calls for sufficiently wide
intermediate arithmetic. Its validated control uses `u128`.

Q04 has two disjoint parallel owners. Q05 has a parser node followed by a billing
node, each owning a separate source file. Other tasks have one owner. Agents have
no command grants; the controller runs the protected Cargo checks. Oracles and
controls are outside agent repositories and grants. `native-trusted` still runs
Cargo under the user's native permissions and is **not** an isolation boundary.
Use these fixtures as trusted code; this setup does not prevent adversarial test
tampering through native process or filesystem access.
