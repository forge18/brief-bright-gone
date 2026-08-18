#!/usr/bin/env python3
"""Measure deterministic local test fixtures without provider credentials."""
from __future__ import annotations

import argparse
import json
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path


def run(name: str, command: list[str]) -> dict[str, object]:
    started = time.perf_counter()
    result = subprocess.run(command, capture_output=True, text=True, check=False)
    return {
        "name": name,
        "command": command,
        "elapsed_ms": round((time.perf_counter() - started) * 1000, 3),
        "exit_code": result.returncode,
        "stdout_tail": result.stdout[-2000:],
        "stderr_tail": result.stderr[-2000:],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    measurements = [
        run("sigil-and-redaction", ["cargo", "test", "--release", "--test", "redaction_sigil_regression"]),
        run("store-and-recovery", ["cargo", "test", "--release", "--test", "filesystem_integration"]),
    ]
    report = {
        "schema_version": 1,
        "created_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "kind": "fixed_fixture_performance",
        "measurements": measurements,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    return 0 if all(item["exit_code"] == 0 for item in measurements) else 1


if __name__ == "__main__":
    raise SystemExit(main())
