#!/usr/bin/env python3
"""Trusted toy-suite oracle: extract an exact Git commit and run frozen tests.

This controller is external to the agent's source repository, but native-trusted
Cargo execution is not an isolation boundary. Use only these trusted fixtures.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile
import time

ROOT = Path(__file__).resolve().parent
IDS = [f"Q{i:02}" for i in range(1, 7)]
STRUCTURE_CHECKER = ROOT / "structure-check"
STRUCTURE_SOURCES = ("Cargo.toml", "Cargo.lock", "src/main.rs")
STRUCTURE_CRITERIA = {
    "private_rate_table", "single_local_literal", "single_remote_literal",
    "shipping_calls_table", "shipping_no_region_literals",
}
_STRUCTURE_CHECKER_CACHE = None


def file_hash(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def directory_hash(path: Path) -> str:
    digest = hashlib.sha256()
    for file in sorted(p for p in path.rglob("*") if p.is_file()):
        digest.update(file.relative_to(path).as_posix().encode() + b"\0")
        digest.update(file.read_bytes() + b"\0")
    return digest.hexdigest()


def invoke(args: list[str], cwd: Path, timeout: int = 120) -> dict:
    env = os.environ.copy()
    env["CARGO_TERM_COLOR"] = "never"
    env["CARGO_NET_OFFLINE"] = "true"
    env["CARGO_TARGET_DIR"] = str(cwd / "target")
    started = time.monotonic()
    try:
        result = subprocess.run(args, cwd=cwd, env=env, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, text=True, timeout=timeout)
        combined = result.stdout + "\n" + result.stderr
        return {"command": args, "exit_code": result.returncode,
                "elapsed_seconds": round(time.monotonic() - started, 3),
                "tests": re.findall(r"^test (\S+) \.\.\. (ok|FAILED)$", result.stdout, re.M),
                "assertion_failure": "test result: FAILED" in combined,
                "stdout": result.stdout[-20000:], "stderr": result.stderr[-20000:],
                "timed_out": False}
    except subprocess.TimeoutExpired as exc:
        def decoded(value):
            return value.decode(errors="replace") if isinstance(value, bytes) else value or ""
        return {"command": args, "exit_code": None,
                "elapsed_seconds": round(time.monotonic() - started, 3),
                "tests": [], "assertion_failure": False, "timed_out": True,
                "stdout": decoded(exc.stdout)[-20000:], "stderr": decoded(exc.stderr)[-20000:]}


def git(repo: Path, *args: str) -> str:
    return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()


def structure_checker_sources_hash() -> str:
    """Canonical digest of the three fixed checker build inputs, in this order."""
    digest = hashlib.sha256()
    for relative in STRUCTURE_SOURCES:
        digest.update(relative.encode() + b"\0")
        digest.update((STRUCTURE_CHECKER / relative).read_bytes() + b"\0")
    return digest.hexdigest()


def prepare_structure_checker() -> tuple[Path, dict]:
    """Build trusted fixed sources; cache only a still-matching source/binary pair."""
    global _STRUCTURE_CHECKER_CACHE
    source_hash = structure_checker_sources_hash()
    executable = STRUCTURE_CHECKER / "target/release" / (
        "harness-structure-check.exe" if os.name == "nt" else "harness-structure-check")
    if _STRUCTURE_CHECKER_CACHE is not None:
        previous = _STRUCTURE_CHECKER_CACHE
        if (previous["checker_source_sha256"] == source_hash and executable.is_file()
                and file_hash(executable) == previous["checker_executable_sha256"]):
            return executable, dict(previous)
    command = ["cargo", "build", "--locked", "--offline", "--release",
               "--manifest-path", str(STRUCTURE_CHECKER / "Cargo.toml"),
               "--target-dir", str(STRUCTURE_CHECKER / "target")]
    env = os.environ.copy()
    env["CARGO_NET_OFFLINE"] = "true"
    env["CARGO_TERM_COLOR"] = "never"
    try:
        build = subprocess.run(command, cwd=ROOT, env=env, capture_output=True,
                               text=True, timeout=180)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise RuntimeError("trusted AST checker build unavailable or timed out") from exc
    if build.returncode != 0 or not executable.is_file():
        raise RuntimeError("trusted AST checker build failed")
    if structure_checker_sources_hash() != source_hash:
        raise RuntimeError("trusted AST checker sources changed during build")
    provenance = {
        "checker_source_sha256": source_hash,
        "checker_executable_sha256": file_hash(executable),
        "checker_source_files_sha256": {
            relative: file_hash(STRUCTURE_CHECKER / relative)
            for relative in STRUCTURE_SOURCES
        },
        "checker_build": {"command": command, "locked": True, "offline": True},
    }
    _STRUCTURE_CHECKER_CACHE = provenance
    return executable, dict(provenance)


def structural_check(candidate: Path) -> dict:
    """Inspect an exact Rust AST; unavailable or invalid evidence fails closed."""
    provenance = {}
    try:
        executable, provenance = prepare_structure_checker()
        process = subprocess.run([str(executable), str(candidate / "src/lib.rs")],
                                 cwd=ROOT, capture_output=True, text=True, timeout=10)
        if len(process.stdout.encode()) > 65536:
            raise RuntimeError("AST checker output exceeded limit")
        result = json.loads(process.stdout)
        if not isinstance(result, dict) or result.get("status") not in {"PASS", "FAIL", "ERROR"}:
            raise RuntimeError("AST checker returned an invalid status")
        if process.returncode != {"PASS": 0, "FAIL": 1, "ERROR": 2}[result["status"]]:
            raise RuntimeError("AST checker status and exit code disagree")
        if result["status"] != "ERROR":
            checks = result.get("checks", {})
            if (not isinstance(checks, dict) or set(checks) != STRUCTURE_CRITERIA
                    or not all(type(value) is bool for value in checks.values())
                    or (result["status"] == "PASS") != all(checks.values())):
                raise RuntimeError("AST checker returned invalid criteria")
        if (structure_checker_sources_hash() != provenance["checker_source_sha256"]
                or file_hash(executable) != provenance["checker_executable_sha256"]):
            raise RuntimeError("AST checker sources or executable changed during grading")
        return {**result, **provenance}
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired):
        return {"status": "ERROR", "checks": {},
                "error": "trusted AST structure checker unavailable or invalid; no fallback",
                **provenance}


def evaluate(task_id: str, repo: Path, candidate_ref: str) -> dict:
    started = time.monotonic()
    sha = git(repo, "rev-parse", "--verify", candidate_ref + "^{commit}")
    tree = git(repo, "rev-parse", sha + "^{tree}")
    outcome = {"schema_version": 1, "task_id": task_id, "candidate_sha": sha,
               "candidate_tree": tree, "status": "FAIL",
               "task_sha256": file_hash(ROOT / "tasks" / f"{task_id}.json"),
               "oracle_sha256": directory_hash(ROOT / "oracles" / task_id),
               "baseline_fixture_sha256": directory_hash(ROOT / "fixtures" / task_id),
               "controller_sha256": file_hash(Path(__file__)),
               "profile": "native-trusted", "external_oracle_isolated": False}
    with tempfile.TemporaryDirectory(prefix=f"harness-oracle-{task_id}-") as temporary:
        temporary = Path(temporary)
        candidate = temporary / "candidate"
        candidate.mkdir()
        archive = subprocess.check_output(["git", "-C", str(repo), "archive", sha])
        with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
            bundle.extractall(candidate, filter="data")
        fixture = ROOT / "fixtures" / task_id
        # Cargo/build configuration is outside every task's permitted write scope.
        protected = {p: (candidate / p).is_file() and
                     (candidate / p).read_bytes() == (fixture / p).read_bytes()
                     for p in ["Cargo.toml", "Cargo.lock"]}
        protected["no_build_script"] = not (candidate / "build.rs").exists()
        if task_id == "Q06":
            protected["unchanged_library"] = (candidate / "src/lib.rs").read_bytes() == (fixture / "src/lib.rs").read_bytes()
        outcome["protected_contract"] = protected
        if not all(protected.values()):
            outcome["reason"] = "protected source/build contract changed"
            return outcome
        acceptance = temporary / "acceptance"
        (acceptance / "tests").mkdir(parents=True)
        (acceptance / "Cargo.toml").write_text(
            '[package]\nname="harness_acceptance"\nversion="0.1.0"\nedition="2021"\n'
            '[dependencies]\nfixture={package="harness_fixture",path="../candidate"}\n')
        (acceptance / "tests/contract.rs").write_bytes((ROOT / "oracles" / task_id / "tests.rs").read_bytes())
        # The frozen crate has no remote dependencies. Generate its own path-only lockfile.
        lock = invoke(["cargo", "generate-lockfile", "--offline"], acceptance)
        outcome["oracle_lockfile"] = lock
        if lock["exit_code"] != 0:
            outcome["reason"] = "external oracle could not prepare lockfile"
            return outcome
        behavior = invoke(["cargo", "test", "--locked", "--offline", "--test", "contract"], acceptance)
        outcome["behavior"] = behavior
        if task_id == "Q03":
            outcome["structure"] = structural_check(candidate)
        if task_id == "Q06":
            normal = invoke(["cargo", "test", "--locked", "--offline", "--tests"], candidate)
            outcome["candidate_tests"] = normal
            mutants = json.loads((ROOT / "oracles/Q06/mutants.json").read_text())
            original = (candidate / "src/lib.rs").read_bytes()
            mutation_results = {}
            for mutant, source in mutants.items():
                (candidate / "src/lib.rs").write_text(source)
                receipt = invoke(["cargo", "test", "--locked", "--offline", "--tests"], candidate)
                receipt["killed"] = receipt["exit_code"] == 101 and receipt["assertion_failure"]
                mutation_results[mutant] = receipt
            (candidate / "src/lib.rs").write_bytes(original)
            outcome["mutants"] = mutation_results
        passed = behavior["exit_code"] == 0 and bool(behavior["tests"])
        if task_id == "Q03":
            passed = passed and outcome["structure"]["status"] == "PASS"
        if task_id == "Q06":
            passed = passed and outcome["candidate_tests"]["exit_code"] == 0 and all(
                mutant["killed"] for mutant in outcome["mutants"].values())
        outcome["status"] = "PASS" if passed else "FAIL"
    outcome["elapsed_seconds"] = round(time.monotonic() - started, 3)
    return outcome


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--task-id", required=True, choices=IDS)
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--candidate", required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        result = evaluate(args.task_id, args.repo.resolve(), args.candidate)
    except Exception as exc:
        result = {"schema_version": 1, "task_id": args.task_id, "status": "ERROR",
                  "error_type": type(exc).__name__, "error": str(exc)}
    rendered = json.dumps(result, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered)
    print(rendered, end="")
    return 0 if result["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
