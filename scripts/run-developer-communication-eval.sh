#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  cat <<'USAGE'
usage: scripts/run-developer-communication-eval.sh [cases.json]

Runs paired baseline and bbg-skill cases through a production bbg OpenAI proxy.
Required environment:
  BBG_BENCH_PROXY_URL                 full /v1/chat/completions proxy URL
  BBG_BENCH_INPUT_PER_MILLION_USD     target-model input price
  BBG_BENCH_OUTPUT_PER_MILLION_USD    target-model output price
Optional environment:
  BBG_BENCH_MODEL (default: gpt-5.6-terra)
  BBG_BENCH_MAX_USD (default: 10)
  BBG_BENCH_REPETITIONS (default: 3)
  BBG_BENCH_PROXY_TOKEN
  BBG_BENCH_OUT_DIR
USAGE
  exit 0
fi

cases="${1:-${BBG_BENCH_CASES:-$root/evals/cases/developer-communication.json}}"
model="${BBG_BENCH_MODEL:-gpt-5.6-terra}"
proxy_url="${BBG_BENCH_PROXY_URL:?set BBG_BENCH_PROXY_URL to the production bbg OpenAI endpoint}"
input_rate="${BBG_BENCH_INPUT_PER_MILLION_USD:?set BBG_BENCH_INPUT_PER_MILLION_USD}"
output_rate="${BBG_BENCH_OUTPUT_PER_MILLION_USD:?set BBG_BENCH_OUTPUT_PER_MILLION_USD}"
max_usd="${BBG_BENCH_MAX_USD:-10}"
repetitions="${BBG_BENCH_REPETITIONS:-3}"
out_dir="${BBG_BENCH_OUT_DIR:-$root/artifacts/benchmarks/$(date -u +%Y%m%dT%H%M%SZ)}"

for command in curl jq cargo awk; do
  command -v "$command" >/dev/null || { echo "error: $command is required" >&2; exit 2; }
done
[[ -f "$cases" ]] || { echo "error: missing cases: $cases" >&2; exit 2; }
[[ "$repetitions" =~ ^[1-9][0-9]*$ ]] || {
  echo "error: BBG_BENCH_REPETITIONS must be a positive integer" >&2
  exit 2
}

mkdir -p "$out_dir/responses"
cp "$cases" "$out_dir/cases.json"
skill="$(cd "$root" && cargo run --quiet --bin bbg -- skill)"
printf '%s' "$skill" > "$out_dir/skill.md"
: > "$out_dir/results.jsonl"
total_estimated_usd=0

auth_args=()
if [[ -n "${BBG_BENCH_PROXY_TOKEN:-}" ]]; then
  auth_args=(-H "Authorization: Bearer $BBG_BENCH_PROXY_TOKEN")
fi

run_arm() {
  local case_id="$1" prompt="$2" arm="$3" run="$4" case_json="$5"
  local response payload content required_terms=0 required_found=0 structure_lines
  local prompt_tokens output_tokens estimated_usd
  if [[ "$arm" == "bbg" ]]; then
    payload="$(jq -n --arg model "$model" --arg system "$skill" --arg prompt "$prompt" \
      '{model: $model, messages: [{role: "system", content: $system}, {role: "user", content: $prompt}]}')"
  else
    payload="$(jq -n --arg model "$model" --arg prompt "$prompt" \
      '{model: $model, messages: [{role: "user", content: $prompt}]}')"
  fi
  response="$(curl --fail --silent --show-error --max-time 600 \
    -H 'content-type: application/json' "${auth_args[@]}" \
    -d "$payload" "$proxy_url")"
  printf '%s\n' "$response" > "$out_dir/responses/$case_id-$arm-$run.json"
  content="$(jq -r '.choices[0].message.content // empty' <<<"$response")"
  [[ -n "$content" ]] || { echo "error: empty $arm response for $case_id" >&2; exit 1; }
  prompt_tokens="$(jq '.usage.prompt_tokens // 0' <<<"$response")"
  output_tokens="$(jq '.usage.completion_tokens // 0' <<<"$response")"
  estimated_usd="$(awk -v input="$prompt_tokens" -v output="$output_tokens" -v ir="$input_rate" -v or="$output_rate" 'BEGIN { printf "%.12f", (input * ir + output * or) / 1000000 }')"
  total_estimated_usd="$(awk -v total="$total_estimated_usd" -v add="$estimated_usd" 'BEGIN { printf "%.12f", total + add }')"
  if ! awk -v total="$total_estimated_usd" -v cap="$max_usd" 'BEGIN { exit !(total <= cap) }'; then
    echo "error: estimated benchmark billing $total_estimated_usd exceeds cap $max_usd" >&2
    exit 1
  fi
  while IFS= read -r term; do
    required_terms=$((required_terms + 1))
    if grep -Fqi -- "$term" <<<"$content"; then
      required_found=$((required_found + 1))
    fi
  done < <(jq -r '.required_terms[]' <<<"$case_json")
  structure_lines="$(grep -Ec '^[[:space:]]*(§|!|>|~|-[#]?[[:space:]]|[0-9]+[.)][[:space:]])' <<<"$content" || true)"
  jq -n \
    --arg case_id "$case_id" --arg arm "$arm" --argjson run "$run" --arg model "$model" \
    --arg response_path "responses/$case_id-$arm-$run.json" \
    --argjson prompt_tokens "$prompt_tokens" --argjson output_tokens "$output_tokens" \
    --argjson estimated_billing_usd "$estimated_usd" \
    --argjson required_terms "$required_terms" --argjson required_found "$required_found" \
    --argjson structure_lines "$structure_lines" \
    '{case_id: $case_id, arm: $arm, run: $run, model: $model, response_path: $response_path, prompt_tokens: $prompt_tokens, output_tokens: $output_tokens, estimated_billing_usd: $estimated_billing_usd, required_terms: $required_terms, required_found: $required_found, structure_lines: $structure_lines, quality_floor_passed: ($required_terms == $required_found)}' \
    >> "$out_dir/results.jsonl"
  printf '\n'
}

while IFS= read -r case_json; do
  case_id="$(jq -r '.id' <<<"$case_json")"
  prompt="$(jq -r '.prompt' <<<"$case_json")"
  for run in $(seq 1 "$repetitions"); do
    printf 'running %s (%s/%s): baseline\n' "$case_id" "$run" "$repetitions" >&2
    run_arm "$case_id" "$prompt" baseline "$run" "$case_json"
    printf 'running %s (%s/%s): bbg\n' "$case_id" "$run" "$repetitions" >&2
    run_arm "$case_id" "$prompt" bbg "$run" "$case_json"
  done
done < <(jq -c '.cases[]' "$cases")

jq -s --arg model "$model" --argjson repetitions "$repetitions" --argjson max_usd "$max_usd" \
  '{schema_version: 1, model: $model, repetitions: $repetitions, max_estimated_billing_usd: $max_usd, method: "paired production-proxy baseline versus bbg skill", cases: .}' \
  "$out_dir/results.jsonl" > "$out_dir/results.json"

jq -r '
  "# Developer communication evaluation\n",
  "Model: `\(.model)`  |  Repetitions: \(.repetitions)  |  Maximum estimated billing: $\(.max_estimated_billing_usd)",
  "",
  "| Case | Arm | Runs | Prompt tokens | Output tokens | Estimated billing | Required-term pass | Structure lines |",
  "|---|---|---:|---:|---:|---:|---:|---:|",
  (.cases | group_by([.case_id, .arm])[] |
    {case_id: .[0].case_id, arm: .[0].arm, runs: length,
     prompt: (map(.prompt_tokens) | add), output: (map(.output_tokens) | add),
     billed: (map(.estimated_billing_usd) | add), passed: (map(select(.quality_floor_passed)) | length),
     structure: (map(.structure_lines) | add)} |
    "| \(.case_id) | \(.arm) | \(.runs) | \(.prompt) | \(.output) | $\(.billed) | \(.passed)/\(.runs) | \(.structure) |"),
  "",
  "Required terms are deterministic quality floors, not a human-quality verdict. Billing is calculated from the supplied target-model rates; retain the proxy cost ledger as the provider-observed source of record. Raw responses are in `responses/`."
' "$out_dir/results.json" > "$out_dir/REPORT.md"

printf 'wrote %s (estimated billing: $%s)\n' "$out_dir" "$total_estimated_usd"
