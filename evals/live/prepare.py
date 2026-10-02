#!/usr/bin/env python3
"""Create committed toy seeds and freeze baseline/control oracle evidence."""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from accept import ROOT, IDS, directory_hash, evaluate, file_hash, git

CLASSES = {"Q01": "feature", "Q02": "bug_fix", "Q03": "behavior_preserving_refactor",
           "Q04": "parallel_components", "Q05": "dependency_integration", "Q06": "test_quality_mutation"}


def initialize(repo: Path) -> str:
    env = os.environ.copy()
    env.update({"GIT_AUTHOR_NAME": "Harness Evaluation", "GIT_AUTHOR_EMAIL": "eval@localhost",
                "GIT_COMMITTER_NAME": "Harness Evaluation", "GIT_COMMITTER_EMAIL": "eval@localhost",
                "GIT_AUTHOR_DATE": "2026-10-02T00:00:00+00:00",
                "GIT_COMMITTER_DATE": "2026-10-02T00:00:00+00:00"})
    for command in [["git", "init", "-q", "-b", "main"], ["git", "add", "."],
                    ["git", "-c", "commit.gpgsign=false", "commit", "-qm", "Frozen toy baseline"]]:
        subprocess.run(command, cwd=repo, env=env, check=True)
    return git(repo, "rev-parse", "HEAD")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT.parents[1] / "artifacts/live-fixtures")
    parser.add_argument("--manifest", type=Path, default=ROOT / "manifest.json")
    parser.add_argument("--task-id", action="append", choices=IDS)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    manifest = {"schema_version": 1, "suite": "six_curated_rust_toy_tasks",
                "created_utc": datetime.now(timezone.utc).isoformat(),
                "model": "gpt-6.1-sol", "trials_per_task": 3, "full_30_task_benchmark": False,
                "oracle_profile": "native-trusted", "external_oracles_isolated": False,
                "controller_sha256": file_hash(ROOT / "accept.py"), "tasks": []}
    success = True
    for task_id in args.task_id or IDS:
        seed = output / task_id
        if seed.exists():
            raise SystemExit(f"Seed already exists; choose a fresh --output directory: {seed}")
        shutil.copytree(ROOT / "fixtures" / task_id, seed)
        baseline_sha = initialize(seed)
        baseline = evaluate(task_id, seed, baseline_sha)
        (output / f"{task_id}-baseline.json").write_text(json.dumps(baseline, indent=2) + "\n")
        with tempfile.TemporaryDirectory(prefix=f"harness-control-{task_id}-") as temporary:
            control = Path(temporary) / "control"
            subprocess.run(["git", "clone", "-q", "--local", "--no-hardlinks", str(seed), str(control)], check=True)
            for path in (ROOT / "controls" / task_id).rglob("*"):
                if path.is_file():
                    target = control / path.relative_to(ROOT / "controls" / task_id)
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(path, target)
            subprocess.run(["git", "add", "."], cwd=control, check=True)
            subprocess.run(["git", "-c", "user.name=Harness Evaluation", "-c", "user.email=eval@localhost",
                            "-c", "commit.gpgsign=false", "commit", "-qm", "Known valid control"], cwd=control, check=True)
            control_sha = git(control, "rev-parse", "HEAD")
            valid = evaluate(task_id, control, control_sha)
            # Control must also pass the same protected command checks used by the harness.
            command = subprocess.run(["cargo", "test", "--locked", "--offline"], cwd=control,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=120)
            valid["protected_cargo_check"] = {"exit_code": command.returncode,
                                               "stdout": command.stdout[-20000:], "stderr": command.stderr[-20000:]}
            valid["status"] = valid["status"] if command.returncode == 0 else "FAIL"
            (output / f"{task_id}-control.json").write_text(json.dumps(valid, indent=2) + "\n")
        frozen = {"id": task_id, "class": CLASSES[task_id],
                  "fixture_dir": str(seed), "fixture_source_dir": str((ROOT / "fixtures" / task_id).relative_to(ROOT.parents[1])),
                  "task_file": str((ROOT / "tasks" / f"{task_id}.json").relative_to(ROOT.parents[1])),
                  "oracle_file": str((ROOT / "oracles" / task_id / "tests.rs").relative_to(ROOT.parents[1])),
                  "baseline_sha": baseline_sha, "baseline_tree": git(seed, "rev-parse", "HEAD^{tree}"),
                  "task_sha256": file_hash(ROOT / "tasks" / f"{task_id}.json"),
                  "fixture_sha256": directory_hash(ROOT / "fixtures" / task_id),
                  "oracle_sha256": directory_hash(ROOT / "oracles" / task_id),
                  "control_sha256": directory_hash(ROOT / "controls" / task_id),
                  "baseline_oracle": baseline["status"], "valid_control_oracle": valid["status"],
                  "baseline_evidence": str(output / f"{task_id}-baseline.json"),
                  "control_evidence": str(output / f"{task_id}-control.json")}
        manifest["tasks"].append(frozen)
        success = success and baseline["status"] == "FAIL" and valid["status"] == "PASS"
        args.manifest.parent.mkdir(parents=True, exist_ok=True)
        args.manifest.write_text(json.dumps(manifest, indent=2) + "\n")
        print(f"{task_id}: baseline={baseline['status']}, valid control={valid['status']}, seed={seed}", flush=True)
    manifest["red_green_validation"] = "PASS" if success else "FAIL"
    args.manifest.write_text(json.dumps(manifest, indent=2) + "\n")
    return 0 if success else 1


if __name__ == "__main__":
    raise SystemExit(main())
