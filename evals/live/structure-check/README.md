# Q03 structure checker

The checker parses the exact candidate `src/lib.rs` with `syn` and prints JSON
containing `status`, the five Q03 structural `checks`, and `scope`. It never
compiles or executes candidate code. Exit codes are 0 for PASS, 1 for FAIL, and
2 for unavailable, oversized, non-UTF-8, or syntactically invalid input.

```sh
cargo build --locked --offline --release --manifest-path evals/live/structure-check/Cargo.toml
evals/live/structure-check/target/release/harness-structure-check /path/to/candidate/src/lib.rs
python3 tests/evaluation_oracle.py -v
```

`evals/live/accept.py` calls this executable through `structural_check(candidate)`.
`prepare_structure_checker()` builds the fixed crate with `--locked --offline`,
checks its source digest before and after building, and caches a matching source
and executable pair within the process. Every grading result includes the
checker source and executable SHA-256. `structure_checker_sources_hash()` hashes
`Cargo.toml`, `Cargo.lock`, and `src/main.rs` in that order, using each relative
path followed by NUL and its bytes followed by NUL. No regex fallback exists.

The private top-level helper must have the specified signature and contain the
single region mapping. Public `shipping_rate` must call that helper with
`region`; calls in nested function declarations do not count. Call matching is
syntactic: the checker does not perform Rust name resolution or prove that a
local binding cannot shadow the helper. Actual production string literals are
counted by their Rust values. Documentation, comments and
attribute strings are excluded. Test-only items are excluded recursively;
unknown non-test configuration conditions remain production code. Macro literal
tokens are counted without macro expansion. Behavior, types, and runtime
execution remain the responsibility of the unchanged independent behavior
oracle.

The previous controller and its SHA-256 are preserved in
`artifacts/contract-validation/oracle-fix/accept-original.py`. Earlier live
receipts remain historical evidence; corrected grading uses separate receipts
for the same candidate commits.
