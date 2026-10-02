#!/usr/bin/env python3
"""Run fresh, bounded ChatGPT trials against frozen toy fixtures.

The controller evaluates committed candidates independently. It does not provide
an isolation boundary against hostile native code; these fixtures are trusted.
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import threading
import time

import accept


ROOT = Path(__file__).resolve().parents[2]
LOCK = threading.Lock()


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def save(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
    temporary.replace(path)


def resolved(value):
    path = Path(value)
    return path if path.is_absolute() else ROOT / path


def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()


def tracked_snapshot(repo):
    names = subprocess.check_output(
        ["git", "-C", str(repo), "ls-files", "-z"]
    ).decode().split("\0")
    return {name: digest(repo / name) for name in names if name}


def run_trial(task, repeat, config):
    started = time.monotonic()
    trial_dir = config.output / task["id"] / f"trial-{repeat}"
    result_file = trial_dir / "result.json"
    if result_file.exists():
        result = json.loads(result_file.read_text())
        if config.skip_existing:
            return result
        raise RuntimeError(f"Existing trial must not be overwritten: {trial_dir}")
    trial_dir.mkdir(parents=True, exist_ok=False)
    repo = trial_dir / "repo"
    task_file = resolved(task["task_file"])
    fixture = resolved(task["fixture_dir"])
    task_hash = digest(task_file)
    task_spec = json.loads(task_file.read_text())
    command = [str(config.binary), "--repo", str(repo), "run", "--task", str(task_file)]
    result = {
        "task_id": task["id"], "repeat": repeat, "execution_mode": "live",
        "model": task_spec["provider"]["model"], "task_sha256": task_hash,
        "binary_sha256": digest(config.binary), "command": command,
        "repo": str(repo), "status": "ERROR", "trial_success": False,
        "limits": "Trusted native toy fixture; independent oracle is not an isolation boundary.",
    }
    try:
        assert task_hash == task["task_sha256"], "Frozen task hash mismatch"
        assert accept.directory_hash(resolved(task["fixture_source_dir"])) == task["fixture_sha256"], "Frozen fixture source changed"
        assert accept.directory_hash(resolved(task["oracle_file"]).parent) == task["oracle_sha256"], "Frozen oracle changed"
        subprocess.run(
            ["git", "clone", "--quiet", "--local", "--no-hardlinks", str(fixture), str(repo)],
            check=True, capture_output=True,
        )
        baseline = git(repo, "rev-parse", "HEAD")
        assert baseline == task["baseline_sha"], "Fixture baseline changed"
        before = tracked_snapshot(repo)
        assert not git(repo, "status", "--porcelain", "--untracked-files=no")
        result["baseline_sha"] = baseline
        with LOCK:
            print(json.dumps({"task_id": task["id"], "repeat": repeat, "status": "RUNNING", "baseline_sha": baseline}), flush=True)
        environment = os.environ.copy()
        cargo_home = config.cargo_home or environment.get("CARGO_HOME")
        rustup_home = config.rustup_home or environment.get("RUSTUP_HOME")
        if not cargo_home:
            local_home = ROOT.parent / ".cargo"
            cargo_home = local_home if (local_home / "bin/cargo").exists() else Path.home() / ".cargo"
        if not rustup_home:
            local_home = ROOT.parent / ".rustup"
            rustup_home = local_home if local_home.is_dir() else Path.home() / ".rustup"
        environment["CARGO_HOME"] = str(cargo_home)
        environment["RUSTUP_HOME"] = str(rustup_home)
        environment["PATH"] = str(Path(cargo_home) / "bin") + os.pathsep + environment.get("PATH", "")
        environment.pop("OPENAI_API_KEY", None)
        environment.pop("OPENAI_BASE_URL", None)
        timeout = task_spec["budget"]["deadline_secs"] + 30
        with (trial_dir / "run.stdout.json").open("w") as stdout, (trial_dir / "run.stderr.log").open("w") as stderr:
            process = subprocess.Popen(command, stdout=stdout, stderr=stderr, env=environment, start_new_session=True)
            save(trial_dir / "running.json", {"pid": process.pid, "task_id": task["id"], "repeat": repeat, "repo": str(repo), "monotonic_started": started})
            try:
                exit_code = process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                if os.name == "posix":
                    os.killpg(process.pid, signal.SIGINT)
                else:
                    process.terminate()
                try:
                    exit_code = process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    if os.name == "posix":
                        os.killpg(process.pid, signal.SIGKILL)
                    else:
                        process.kill()
                    exit_code = process.wait()
                result["controller_timeout"] = True
        result["exit_code"] = exit_code
        result["elapsed_seconds"] = round(time.monotonic() - started, 3)
        result["original_head_preserved"] = git(repo, "rev-parse", "HEAD") == baseline
        result["original_files_preserved"] = tracked_snapshot(repo) == before
        result["task_unchanged"] = digest(task_file) == task_hash
        report_path = trial_dir / "report.json"
        try:
            report = json.loads((trial_dir / "run.stdout.json").read_text())
            if "run" not in report:
                raise ValueError("No run in stdout")
        except (ValueError, json.JSONDecodeError):
            status = subprocess.run([str(config.binary), "--repo", str(repo), "status"], text=True, capture_output=True, env=environment)
            save(trial_dir / "status-recovery.json", {"exit_code": status.returncode, "stdout": status.stdout, "stderr": status.stderr})
            runs = json.loads(status.stdout) if status.returncode == 0 else []
            if not runs:
                raise RuntimeError("No durable run after CLI failure")
            run_id = runs[-1]["id"]
            inspected = subprocess.run([str(config.binary), "--repo", str(repo), "inspect", run_id], text=True, capture_output=True, env=environment, check=True)
            report = json.loads(inspected.stdout)
        save(report_path, report)
        run = report["run"]
        result.update({"run_id": run["id"], "harness_state": run["state"], "candidate_sha": run["candidate_sha"], "harness_error": run["error"], "spent_tokens": run["spent_tokens"], "reserved_tokens": run["reserved_tokens"], "report": str(report_path)})
        events = report["events"]
        receipts = [event["payload"] for event in events if event["kind"] == "model.receipt"]
        unknowns = [event["payload"] for event in events if event["kind"] == "model.unknown"]
        total_input = sum(receipt["usage"]["input_tokens"] for receipt in receipts)
        total_output = sum(receipt["usage"]["output_tokens"] for receipt in receipts)
        result["usage"] = {"input_tokens": total_input, "output_tokens": total_output, "model_calls_with_receipts": len(receipts), "unknown_calls": len(unknowns), "all_receipts_complete": all(receipt["usage"]["complete"] for receipt in receipts), "ledger_matches_observed_usage": total_input + total_output == run["spent_tokens"]}
        result["automatic_publication_absent"] = not any(event["kind"] == "publication.receipt" for event in events)
        if run["candidate_sha"]:
            oracle_command = ["python3", str(ROOT / "evals/live/accept.py"), "--task-id", task["id"], "--repo", str(repo), "--candidate", run["candidate_sha"]]
            oracle = subprocess.run(oracle_command, text=True, capture_output=True, env=environment, timeout=180)
            (trial_dir / "oracle.stdout.json").write_text(oracle.stdout)
            (trial_dir / "oracle.stderr.log").write_text(oracle.stderr)
            result["oracle_exit_code"] = oracle.returncode
            result["oracle"] = json.loads(oracle.stdout)
            result["oracle_integrity_preserved"] = (
                result["oracle"].get("task_sha256") == task["task_sha256"]
                and result["oracle"].get("oracle_sha256") == task["oracle_sha256"]
                and result["oracle"].get("baseline_fixture_sha256") == task["fixture_sha256"]
                and result["oracle"].get("controller_sha256") == config.controller_sha256
            )
        else:
            result["oracle"] = {"status": "NOT_EXECUTED", "reason": "No candidate commit"}
            result["oracle_integrity_preserved"] = None
        candidate_hash = report["checks"][-1]["candidate_hash"] if report["checks"] else None
        current_checks = [check for check in report["checks"] if check["candidate_hash"] == candidate_hash]
        current_reviews = [review for review in report["reviews"] if review["candidate_hash"] == candidate_hash]
        roles = {review["role"]: review["verdict"] for review in current_reviews}
        result["review_verdicts"] = roles
        result["candidate_checks_pass"] = bool(current_checks) and all(check["verdict"] == "PASS" for check in current_checks)
        result["four_reviews_pass"] = all(roles.get(role) == "PASS" for role in ["requirements", "code", "tests", "security"])
        result["false_verified"] = run["state"] == "VERIFIED" and result["oracle"].get("status") == "FAIL"
        result["trial_success"] = all([
            exit_code == 0, run["state"] == "VERIFIED",
            result["oracle"].get("status") == "PASS", result["candidate_checks_pass"],
            result["four_reviews_pass"], result["original_head_preserved"],
            result["original_files_preserved"], result["task_unchanged"],
            result["automatic_publication_absent"], not unknowns,
            run["reserved_tokens"] == 0, result["usage"]["ledger_matches_observed_usage"],
            result["oracle_integrity_preserved"],
        ])
        result["status"] = "PASS" if result["trial_success"] else "FAIL"
    except Exception as error:
        result["controller_error"] = f"{type(error).__name__}: {error}"
    result["elapsed_seconds"] = round(time.monotonic() - started, 3)
    save(result_file, result)
    with LOCK:
        print(json.dumps({key: result.get(key) for key in ["task_id", "repeat", "status", "harness_state", "harness_error", "controller_error", "spent_tokens", "elapsed_seconds"]}), flush=True)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, default=ROOT / "evals/live/manifest.json")
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/full-validation/live")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/harness")
    parser.add_argument("--task-id", action="append")
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--workers", type=int, default=2)
    parser.add_argument("--skip-existing", action="store_true")
    parser.add_argument("--cargo-home", type=Path)
    parser.add_argument("--rustup-home", type=Path)
    config = parser.parse_args()
    config.output = config.output.resolve()
    config.binary = config.binary.resolve()
    manifest = json.loads(config.manifest.read_text())
    config.controller_sha256 = manifest["controller_sha256"]
    assert digest(ROOT / "evals/live/accept.py") == config.controller_sha256, "Frozen controller changed"
    tasks = [task for task in manifest["tasks"] if not config.task_id or task["id"] in config.task_id]
    if not tasks:
        parser.error("No selected tasks")
    jobs = [(task, repeat) for repeat in range(1, config.repeats + 1) for task in tasks]
    results = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=config.workers) as executor:
        futures = [executor.submit(run_trial, task, repeat, config) for task, repeat in jobs]
        for future in concurrent.futures.as_completed(futures):
            results.append(future.result())
            save(config.output / "progress.json", results)
    results.sort(key=lambda value: (value["task_id"], value["repeat"]))
    summary = {
        "schema_version": 1, "execution_mode": "live", "manifest_sha256": digest(config.manifest),
        "tasks": len(tasks), "trials": len(results), "passed": sum(result["trial_success"] for result in results),
        "failed": sum(result["status"] == "FAIL" for result in results), "errors": sum(result["status"] == "ERROR" for result in results),
        "stable_tasks": sum(all(result["trial_success"] for result in results if result["task_id"] == task["id"]) for task in tasks),
        "repeats_per_task": config.repeats, "results": results,
        "limits": "Six public trusted toy fixtures, not the planned 30-task benchmark or isolated holdout; no Windows/macOS execution.",
    }
    save(config.output / "summary.json", summary)
    print(json.dumps({key: value for key, value in summary.items() if key != "results"}), flush=True)
    return 0 if summary["passed"] == summary["trials"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
