#!/usr/bin/env python3
"""Measure a fresh --version process with a warm filesystem cache.

Python is a benchmark dependency only; the harness has no Python runtime dependency.
"""

import argparse
import hashlib
import json
import math
import os
import platform
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--warmup", type=int, default=20)
    parser.add_argument("--samples", type=int, default=200)
    args = parser.parse_args()
    if args.warmup < 0 or args.samples < 1:
        parser.error("warmup must be >= 0 and samples must be >= 1")
    binary = args.binary.resolve()
    command = [str(binary), "--version"]
    source_root = Path(__file__).resolve().parent.parent
    sources = [source_root / "Cargo.toml", source_root / "Cargo.lock", source_root / "rust-toolchain.toml"]
    sources.extend(sorted((source_root / "src").rglob("*.rs")))
    source_digest = hashlib.sha256()
    for path in sources:
        source_digest.update(path.relative_to(source_root).as_posix().encode())
        source_digest.update(b"\0")
        source_digest.update(path.read_bytes())
        source_digest.update(b"\0")
    version = None
    samples = []
    for ordinal in range(args.warmup + args.samples):
        start = time.perf_counter_ns()
        result = subprocess.run(command, check=True, capture_output=True)
        elapsed_ms = (time.perf_counter_ns() - start) / 1_000_000
        observed = result.stdout.decode().strip()
        if version is None:
            version = observed
        if observed != version or result.stderr:
            raise RuntimeError("inconsistent --version response")
        if ordinal >= args.warmup:
            samples.append(elapsed_ms)
    cpu = platform.processor() or None
    mem_bytes = None
    if Path("/proc/cpuinfo").exists():
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
    if hasattr(os, "sysconf"):
        try:
            mem_bytes = os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE")
        except (ValueError, OSError):
            pass
    filesystem = None
    if platform.system() == "Linux":
        filesystem = subprocess.run(
            ["stat", "-f", "-c", "%T", str(binary)], check=True, capture_output=True, text=True
        ).stdout.strip()
    try:
        toolchain = subprocess.run(["rustc", "--version"], check=True, capture_output=True, text=True).stdout.strip()
    except FileNotFoundError:
        toolchain = None
    report = {
        "schema_version": 1,
        "measured_at": datetime.now(timezone.utc).isoformat(),
        "metric": "fresh_process_warm_filesystem_version_wall_ms",
        "method": {
            "argv": [binary.name, "--version"],
            "clock": "Python time.perf_counter_ns (monotonic)",
            "warmup": args.warmup,
            "samples": args.samples,
            "percentile": "nearest rank",
            "includes": ["process creation", "CLI parse", "stdout capture", "process exit"],
            "excludes": ["model calls", "SQLite open", "Git", "agent runtime initialization"],
            "cold_cache": False,
            "power_profile": "not controlled",
            "machine_scope": "local execution container; not a user laptop benchmark",
        },
        "machine": {
            "os": platform.system(),
            "kernel": platform.release(),
            "architecture": platform.machine(),
            "cpu": cpu,
            "logical_cpus_visible": os.cpu_count(),
            "host_memory_bytes_visible": mem_bytes,
            "filesystem": filesystem,
        },
        "binary": {"version": version, "bytes": binary.stat().st_size, "sha256": sha256(binary)},
        "source_sha256": source_digest.hexdigest(),
        "toolchain": toolchain,
        "summary_ms": {"p50": percentile(samples, 0.50), "p95": percentile(samples, 0.95), "max": max(samples)},
        "raw_ms": samples,
        "limitations": [
            "No cold-cache or multiple-machine comparison",
            "No idle RSS, command dispatch, cancellation or full-task benchmark",
            "The virtualized host may have other concurrent workloads",
            "This single observation does not close evaluation scenario S40",
        ],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"output": str(args.output), "summary_ms": report["summary_ms"], "binary_bytes": report["binary"]["bytes"]}))


if __name__ == "__main__":
    main()
