#!/usr/bin/env python3
"""Run the versioned bbg R14 probe and paired raw-API benchmark.

This runner deliberately talks directly to a provider. It does not use the bbg
proxy, so encoding compliance is measured before format-dependent infrastructure.
Credentials are read from the environment and never written to reports.
"""
from __future__ import annotations

import argparse
import json
import os
import random
import re
import statistics
import sys
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).parent
DEFAULT_CORPUS = ROOT / "v1" / "corpus.json"
DEFAULT_CONFIG = ROOT / "v1" / "config.json"
SIGIL_LINE = re.compile(r"^(?:§|>|!|~|\.|\?|x)(?:\s|$)|^-+(?:[#>!~]?)(?:\s|$)|^\\[§>!~.?x-]")


def load_json(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


def now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def classify_sigil_output(text: str) -> dict[str, Any]:
    """Classify observable format compliance without pretending to decode prose."""
    lines = text.splitlines()
    sigil_lines: list[str] = []
    zero_sigil = False
    in_fence = False
    terminals: list[int] = []
    malformed: list[str] = []

    for index, line in enumerate(lines):
        if line.startswith("```"):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        if SIGIL_LINE.match(line):
            sigil_lines.append(line)
        if line.startswith((".", "?", "x")) and (len(line) == 1 or line[1].isspace()):
            terminals.append(index)
        if line.startswith("|") and line.endswith("|"):
            malformed.append(f"line {index + 1}: trailing table separator")
        if line.startswith("-") and len(line) > 1 and line[1] not in "-#>!~ \t":
            malformed.append(f"line {index + 1}: ambiguous dash prefix")

    if in_fence:
        malformed.append("unterminated fence")
    zero_sigil = not sigil_lines
    last_nonblank = max((i for i, line in enumerate(lines) if line.strip()), default=None)
    if terminals and (len(terminals) != 1 or terminals[0] != last_nonblank):
        malformed.append("terminal is missing, duplicated, or not final")

    return {
        "zero_sigil": zero_sigil,
        "sigil_line_count": len(sigil_lines),
        "malformed": bool(malformed),
        "malformed_reasons": malformed,
        "silently_misdecoded": None,
        "manual_review_required": bool(sigil_lines) and not malformed,
        "terminal_count": len(terminals),
    }


def request(provider: str, endpoint: str, api_key: str, model: str, prompt: str, system: str | None) -> dict[str, Any]:
    if provider == "anthropic":
        payload: dict[str, Any] = {"model": model, "max_tokens": 2048, "messages": [{"role": "user", "content": prompt}]}
        if system:
            payload["system"] = system
        headers = {"x-api-key": api_key, "anthropic-version": "2023-06-01", "content-type": "application/json"}
    else:
        messages = ([{"role": "system", "content": system}] if system else []) + [{"role": "user", "content": prompt}]
        payload = {"model": model, "messages": messages, "max_tokens": 2048}
        headers = {"authorization": f"Bearer {api_key}", "content-type": "application/json"}

    data = json.dumps(payload).encode()
    req = urllib.request.Request(endpoint, data=data, headers=headers, method="POST")
    started = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=120) as response:
            body = json.loads(response.read().decode())
            status = response.status
    except Exception as error:  # noqa: BLE001 - report provider failures without secrets
        return {"status": "error", "error": type(error).__name__, "elapsed_ms": round((time.monotonic() - started) * 1000)}

    if provider == "anthropic":
        content = "".join(part.get("text", "") for part in body.get("content", []) if part.get("type") == "text")
    else:
        content = body.get("choices", [{}])[0].get("message", {}).get("content", "")
    return {
        "status": "ok" if 200 <= status < 300 else "provider_error",
        "http_status": status,
        "content": content,
        "usage": body.get("usage", {}),
        "elapsed_ms": round((time.monotonic() - started) * 1000),
    }


def run_probe(args: argparse.Namespace, corpus: dict[str, Any], config: dict[str, Any]) -> dict[str, Any]:
    system = Path(args.skill).read_text(encoding="utf-8")
    results = []
    for task in corpus["tasks"]:
        response = request(args.provider, args.endpoint, args.api_key, args.model, task["prompt"], system)
        compliance = classify_sigil_output(response.get("content", "")) if response.get("status") == "ok" else {"provider_error": True}
        results.append({"task_id": task["id"], "response": response, "compliance": compliance})
    successful = [item for item in results if item["response"].get("status") == "ok"]
    return {
        "schema_version": 1,
        "kind": "r14",
        "created_at": now(),
        "provider": args.provider,
        "model": args.model,
        "corpus_version": corpus["corpus_version"],
        "thresholds": config["r14"],
        "results": results,
        "summary": {
            "requests": len(results),
            "provider_errors": len(results) - len(successful),
            "zero_sigil_rate": sum(item["compliance"].get("zero_sigil", False) for item in successful) / len(successful) if successful else None,
            "malformed_rate": sum(item["compliance"].get("malformed", False) for item in successful) / len(successful) if successful else None,
            "silently_misdecoded": "manual review required for parseable sigil responses",
        },
    }


def usage_counts(response: dict[str, Any]) -> tuple[int, int] | None:
    usage = response.get("usage", {})
    input_tokens = usage.get("input_tokens", usage.get("prompt_tokens"))
    output_tokens = usage.get("output_tokens", usage.get("completion_tokens"))
    if isinstance(input_tokens, int) and isinstance(output_tokens, int):
        return input_tokens, output_tokens
    return None


def percentile(values: list[float], probability: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    position = (len(ordered) - 1) * probability
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    weight = position - lower
    return ordered[lower] * (1 - weight) + ordered[upper] * weight


def bootstrap_median_ci(values: list[float], seed: int) -> dict[str, float | None]:
    if len(values) < 5:
        return {"low": None, "median": percentile(values, 0.5), "high": None}
    rng = random.Random(seed)
    medians = [statistics.median(rng.choices(values, k=len(values))) for _ in range(1000)]
    return {"low": percentile(medians, 0.025), "median": statistics.median(values), "high": percentile(medians, 0.975)}


def run_paired(args: argparse.Namespace, corpus: dict[str, Any], config: dict[str, Any]) -> dict[str, Any]:
    rng = random.Random(args.seed)
    repetitions = max(args.repetitions, config["paired_benchmark"]["minimum_repetitions_per_task_per_arm"])
    skill = Path(args.skill).read_text(encoding="utf-8")
    observations = []
    for repetition in range(repetitions):
        for task in corpus["tasks"]:
            arms = ["skill_off", "skill_on"]
            rng.shuffle(arms)
            for arm in arms:
                response = request(args.provider, args.endpoint, args.api_key, args.model, task["prompt"], skill if arm == "skill_on" else None)
                counts = usage_counts(response)
                cost = None
                if counts is not None and args.input_price_per_million is not None and args.output_price_per_million is not None:
                    cost = (counts[0] * args.input_price_per_million + counts[1] * args.output_price_per_million) / 1_000_000
                observations.append({"repetition": repetition, "task_id": task["id"], "arm": arm, "response": response, "usage_counts": counts, "estimated_cost": cost, "correct": None, "follow_up_turns": None})
    by_arm = {}
    for arm in ("skill_off", "skill_on"):
        rows = [row for row in observations if row["arm"] == arm]
        tokens = [sum(row["usage_counts"]) for row in rows if row["usage_counts"] is not None]
        costs = [row["estimated_cost"] for row in rows if row["estimated_cost"] is not None]
        by_arm[arm] = {
            "runs": len(rows),
            "median_tokens": bootstrap_median_ci(tokens, args.seed),
            "total_estimated_cost": sum(costs) if costs else None,
            "correctness": "manual evaluation required",
            "follow_up_turns": "manual evaluation required",
        }
    return {
        "schema_version": 1,
        "kind": "paired_benchmark",
        "created_at": now(),
        "seed": args.seed,
        "provider": args.provider,
        "model": args.model,
        "corpus_version": corpus["corpus_version"],
        "pricing": {"input_per_million": args.input_price_per_million, "output_per_million": args.output_price_per_million},
        "observations": observations,
        "summary": by_arm,
        "confidence": {"method": "bootstrap 95% interval for median tokens; correctness and follow-up intervals require manual labels", "status": "inconclusive until manual labels and provider pricing are supplied"},
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["r14", "paired"])
    parser.add_argument("--skill", required=True, help="Frozen skill text used for the skill_on arm")
    parser.add_argument("--provider", choices=["openai", "anthropic"], default="openai")
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--api-key", default=os.environ.get("BBG_API_KEY", ""))
    parser.add_argument("--model", required=True)
    parser.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--repetitions", type=int, default=5)
    parser.add_argument("--input-price-per-million", type=float)
    parser.add_argument("--output-price-per-million", type=float)
    parser.add_argument("--seed", type=int, default=1)
    args = parser.parse_args()
    if not args.api_key:
        parser.error("provide --api-key or BBG_API_KEY")
    corpus = load_json(args.corpus)
    config = load_json(args.config)
    report = run_probe(args, corpus, config) if args.mode == "r14" else run_paired(args, corpus, config)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(args.out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
