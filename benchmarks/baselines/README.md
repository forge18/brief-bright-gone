# Measurement Baselines

Baseline reports are generated artifacts for a selected representative
provider/model run. bbg itself is model-agnostic: no single model is its
compatibility target, and one report cannot establish universal behavior.
Reports are intentionally not checked in until a frozen skill file, provider
endpoint, selected evaluation model, and disposable API credential are
supplied.

Run the raw probe directly against the provider:

```bash
python3 benchmarks/run_measurement.py r14 \
  --provider openai \
  --endpoint "$OPENAI_BASE_URL/chat/completions" \
  --model "$BBG_MODEL" \
  --skill path/to/frozen-skill.txt \
  --out benchmarks/baselines/$(date -u +%Y%m%dT%H%M%SZ)-v1.json
```

The paired harness uses the same corpus with randomized `skill_off` and
`skill_on` arms. Supply provider pricing to calculate estimated billed cost:

```bash
python3 benchmarks/run_measurement.py paired \
  --provider openai \
  --endpoint "$OPENAI_BASE_URL/chat/completions" \
  --model "$BBG_MODEL" \
  --skill path/to/frozen-skill.txt \
  --input-price-per-million "$BBG_INPUT_PRICE" \
  --output-price-per-million "$BBG_OUTPUT_PRICE" \
  --out benchmarks/baselines/paired-$(date -u +%Y%m%dT%H%M%SZ)-v1.json
```

The report must be reviewed for zero-sigil, malformed, and silently misdecoded
responses before the encoding is treated as evidence for that evaluation model;
it is not a universal claim about all models. The runner
records silently misdecoded as manual review rather than guessing. Correctness
and follow-up fields are intentionally manual labels: the harness never treats
model self-report as task success.
