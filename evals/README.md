# Developer communication evaluation

`cases/developer-communication.json` is the fixed corpus for bbg's public communication claims: explain, debug, plan, review, and status.

`../scripts/run-developer-communication-eval.sh` runs each case twice through a production bbg OpenAI proxy:

1. baseline — user prompt only;
2. bbg — the same prompt plus the installed bbg skill.

The runner records raw replies, provider-reported prompt/output tokens, estimated billing from explicit model rates, deterministic required-term coverage, and structural-line counts. Required terms are a quality floor; they do not replace human review. The proxy cost ledger remains the provider-observed billing source of record.

The runner never uses local Ollama. It requires a full production proxy endpoint and explicit target-model rates so a hard estimated-billing cap can stop the run.

```bash
BBG_BENCH_PROXY_URL=http://127.0.0.1:8088/v1/chat/completions \
BBG_BENCH_MODEL=gpt-5.6-terra \
BBG_BENCH_INPUT_PER_MILLION_USD=<input-rate> \
BBG_BENCH_OUTPUT_PER_MILLION_USD=<output-rate> \
BBG_BENCH_MAX_USD=10 \
scripts/run-developer-communication-eval.sh
```

Set `BBG_BENCH_PROXY_TOKEN` if the proxy requires a bearer token. Set `BBG_BENCH_REPETITIONS` or `BBG_BENCH_OUT_DIR` to override the run count or output directory. The runner stores immutable local artifacts under `artifacts/benchmarks/` by default.
