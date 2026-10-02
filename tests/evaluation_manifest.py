#!/usr/bin/env python3
"""Actual evaluation CLI must validate frozen inputs before any dispatch."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import runpy
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
RUNNER = ROOT / "evals/live/run.py"
CONTROLLER = ROOT / "evals/live/accept.py"
FIXTURE = ROOT / "evals/live/fixtures/Q03"
TASK = ROOT / "evals/live/tasks/Q03.json"
ORACLE = ROOT / "evals/live/oracles/Q03"
DISPATCH_FILES = ("running.json", "run.stdout.json", "run.stderr.log", "report.json",
                  "status-recovery.json", "oracle.stdout.json", "oracle.stderr.log")


class FrozenManifestCli(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.oracle_api = runpy.run_path(str(CONTROLLER), run_name="manifest_contract_api")

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="harness-manifest-contract-")
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)

    def case(self, optimized):
        directory = self.directory / ("optimized" if optimized else "normal")
        directory.mkdir()
        source = directory / "fixture-source"
        seed = directory / "committed-seed"
        oracle = directory / "oracle"
        shutil.copytree(FIXTURE, source)
        shutil.copytree(FIXTURE, seed)
        shutil.copytree(ORACLE, oracle)
        for arguments in (["init", "-q"], ["add", "."],
                          ["-c", "user.name=Manifest Contract", "-c", "user.email=manifest@localhost",
                           "-c", "commit.gpgsign=false", "commit", "-qm", "Frozen test seed"]):
            process = subprocess.run(["git", "-C", str(seed), *arguments],
                                     capture_output=True, text=True, timeout=10)
            self.assertEqual(process.returncode, 0, process.stderr)
        baseline = subprocess.check_output(
            ["git", "-C", str(seed), "rev-parse", "HEAD"], text=True).strip()
        task_file = directory / "task.json"
        task_file.write_bytes(TASK.read_bytes())
        binary = directory / "not-an-executable-harness"
        binary.write_text("CANARY: this file must never be dispatched\n")
        binary.chmod(0o600)
        manifest = {
            "schema_version": 1,
            "controller_sha256": self.oracle_api["file_hash"](CONTROLLER),
            "structure_checker_sources_sha256": self.oracle_api["structure_checker_sources_hash"](),
            "tasks": [{
                "id": "Q03", "task_file": str(task_file), "fixture_dir": str(seed),
                "fixture_source_dir": str(source), "oracle_file": str(oracle / "tests.rs"),
                "baseline_sha": baseline,
                "task_sha256": self.oracle_api["file_hash"](task_file),
                "fixture_sha256": self.oracle_api["directory_hash"](source),
                "oracle_sha256": self.oracle_api["directory_hash"](oracle),
            }],
        }
        manifest_path = directory / "manifest.json"
        output = directory / "output"
        return {"directory": directory, "source": source, "seed": seed,
                "oracle": oracle, "task_file": task_file, "binary": binary,
                "manifest": manifest, "manifest_path": manifest_path,
                "output": output, "trial": output / "Q03/trial-1"}

    def invoke(self, case, optimized, extra_arguments=()):
        case["manifest_path"].write_text(json.dumps(case["manifest"]) + "\n")
        command = [sys.executable]
        if optimized:
            command.append("-O")
        command += [str(RUNNER), "--manifest", str(case["manifest_path"]),
                    "--output", str(case["output"]), "--binary", str(case["binary"]),
                    "--task-id", "Q03", "--repeats", "1", "--workers", "1"]
        command += list(extra_arguments)
        process = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=15)
        self.assertNotEqual(process.returncode, 0, process.stdout + process.stderr)
        self.assertNotIn('"status": "RUNNING"', process.stdout)
        self.assert_no_dispatch(case)
        return process

    def assert_no_dispatch(self, case):
        for name in DISPATCH_FILES:
            self.assertFalse((case["trial"] / name).exists(), f"dispatch evidence unexpectedly created: {name}")
        self.assertFalse((case["trial"] / "repo").exists(), "trial repository created before frozen input validation")

    def global_rejection(self, mutate, message):
        for optimized in (False, True):
            with self.subTest(optimized=optimized):
                case = self.case(optimized)
                case["trial"].mkdir(parents=True)
                receipt = case["trial"] / "result.json"
                original = b'{"historical_receipt":"preserve byte-for-byte"}\n'
                receipt.write_bytes(original)
                mutate(case)
                process = self.invoke(case, optimized)
                self.assertIn(message, process.stderr)
                self.assertEqual(receipt.read_bytes(), original)
                self.assertFalse((case["output"] / "summary.json").exists())
                self.assertFalse((case["output"] / "progress.json").exists())

    def per_trial_rejection(self, mutate, message):
        for optimized in (False, True):
            with self.subTest(optimized=optimized):
                case = self.case(optimized)
                mutate(case)
                process = self.invoke(case, optimized)
                self.assertEqual(process.returncode, 2, process.stdout + process.stderr)
                result = json.loads((case["trial"] / "result.json").read_text())
                self.assertEqual(result["status"], "ERROR")
                self.assertFalse(result["trial_success"])
                self.assertIn(message, result["controller_error"])
                summary = json.loads((case["output"] / "summary.json").read_text())
                self.assertEqual(summary["errors"], 1)
                self.assertEqual(summary["passed"], 0)

    def test_changed_frozen_controller_digest_rejected_before_dispatch(self):
        self.global_rejection(
            lambda case: case["manifest"].update(controller_sha256="0" * 64),
            "Frozen controller changed")

    def test_changed_frozen_ast_source_digest_rejected_before_dispatch(self):
        self.global_rejection(
            lambda case: case["manifest"].update(structure_checker_sources_sha256="0" * 64),
            "Frozen structure checker changed")

    def test_historical_manifest_missing_checker_digest_rejected_before_dispatch(self):
        self.global_rejection(
            lambda case: case["manifest"].pop("structure_checker_sources_sha256"),
            "structure_checker_sources_sha256")

    def test_task_tamper_rejected_before_repository_or_model_dispatch(self):
        self.per_trial_rejection(
            lambda case: case["task_file"].write_text(case["task_file"].read_text() + "\n"),
            "Frozen task hash mismatch")

    def test_fixture_source_tamper_rejected_before_repository_or_model_dispatch(self):
        def mutate(case):
            path = case["source"] / "src/lib.rs"
            path.write_text(path.read_text() + "\n// changed fixture source\n")
        self.per_trial_rejection(mutate, "Frozen fixture source changed")

    def test_oracle_tamper_rejected_before_repository_or_model_dispatch(self):
        def mutate(case):
            path = case["oracle"] / "tests.rs"
            path.write_text(path.read_text() + "\n// changed oracle\n")
        self.per_trial_rejection(mutate, "Frozen oracle changed")

    def test_baseline_tamper_rejected_before_repository_or_model_dispatch(self):
        self.per_trial_rejection(
            lambda case: case["manifest"]["tasks"][0].update(baseline_sha="0" * 40),
            "Fixture baseline changed")

    def test_existing_error_receipt_is_never_overwritten(self):
        for optimized in (False, True):
            with self.subTest(optimized=optimized):
                case = self.case(optimized)
                original_task = case["task_file"].read_bytes()
                case["task_file"].write_bytes(original_task + b"\n")
                self.invoke(case, optimized)
                receipt = case["trial"] / "result.json"
                original = receipt.read_bytes()
                original_digest = hashlib.sha256(original).hexdigest()
                case["task_file"].write_bytes(original_task)
                process = self.invoke(case, optimized)
                self.assertIn("Existing trial must not be overwritten", process.stderr)
                self.assertEqual(hashlib.sha256(receipt.read_bytes()).hexdigest(), original_digest)
                self.assertEqual(receipt.read_bytes(), original)

    def test_skip_existing_cannot_reuse_stale_pass_after_task_changes(self):
        for optimized in (False, True):
            with self.subTest(optimized=optimized):
                case = self.case(optimized)
                case["trial"].mkdir(parents=True)
                receipt = case["trial"] / "result.json"
                historical = {
                    "task_id": "Q03", "repeat": 1, "status": "PASS", "trial_success": True,
                    "execution_mode": "live", "model": "historical-fixture-model",
                    "task_sha256": case["manifest"]["tasks"][0]["task_sha256"],
                    "candidate_sha": case["manifest"]["tasks"][0]["baseline_sha"],
                }
                original = (json.dumps(historical, indent=2) + "\n").encode()
                receipt.write_bytes(original)
                case["task_file"].write_bytes(case["task_file"].read_bytes() + b"\n")
                self.invoke(case, optimized, extra_arguments=["--skip-existing"])
                self.assertEqual(receipt.read_bytes(), original)
                self.assertFalse((case["output"] / "progress.json").exists())
                self.assertFalse((case["output"] / "summary.json").exists())


if __name__ == "__main__":
    unittest.main()
