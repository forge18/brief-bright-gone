# bbg Differentiation Research — How to Exceed Caveman

**Date:** 2026-08-16
**Purpose:** Identify capabilities that make bbg genuinely better/different than Caveman, grounded in academic research + tech blogs, not guessing.

---

## 1. What Caveman already does (so we don't duplicate)

From reading its source + docs:
- **Output skill** (terse replies) with intensity levels + guardrails
- **Input proxy** (compression of tool output, files, logs, history) with content-type routing
- **CCR reversible compression** (originals stored, retrievable)
- **`cacheengine`**: prompt-cache planner — "selects positive-break-even prefix points," maximizes stable prefixes. So it's already cache-aware.
- **`caveman learn`**: scans history, ranks token sinks by flow, "Cave Score," inferred-vs-verified evidence labeling
- **TOON/Pixel** (structured-data + image transforms)
- **Accounting/evidence**: `inferred` vs `verified` labels; offline never claims verified

**Conclusion: Caveman is already cache-aware, reversible, measurable, and multi-agent. Byte-compression and cache-planner are NOT differentiators.**

---

## 2. The research finding that reframes the problem

### "Token Reduction Is Not Cost Reduction" (arXiv 2607.12161) — the critical paper
- Evaluated three token-reduction tools against a Claude Code baseline, measuring **provider-billed cost, task success, cache traffic**.
- **The largest compression setup cut delivered tool-output tokens by 38.4% but INCREASED billed cost by 6.8%.**
- Lighter compression: "small and statistically uncertain savings."
- **Token reduction weakly correlated with cost reduction: Pearson r = 0.15.**
- Cause: **prompt-cache creation/reads dominate input-side cost** — compression changes the cached prefix, breaking cache hits and paying full price for re-processing.
- Compression can also **alter agent trajectories** (behavior change, not just byte change).

**Implication for bbg:** measuring "tokens saved" is the wrong north star. Cost, cache-hit rate, and task success are the real metrics. A system that maximizes tokens-removed can be *more expensive*.

### "Don't Break the Cache" (arXiv 2601.06007)
- Quantifies prompt-cache benefits across OpenAI/Anthropic/Google for multi-turn agentic workloads.
- Compares full-context caching vs system-prompt-only vs caching-that-excludes-dynamic-tool-results.
- Lesson: **what you include/exclude from the cached prefix matters as much as compression.**

### VISTA — "LLM Agents Are Latent Context Managers" (arXiv 2606.30005) — the differentiator
- **Models are proprioceptively blind to their own context**: from the prompt alone they cannot infer block size, recency, or remaining budget — all needed for keep-or-archive decisions.
- VISTA = training-free, model-agnostic layer: **visible internal state** (typed addressable blocks + runtime dashboard of token usage/recency/archive status/remaining budget + recoverable full-fidelity archives).
- Results: lifts Gemini-3-Flash from 22.7% → 50.7% on LOCA-Bench; 58% on BrowseComp-Plus.
- **This is a capability Caveman does not have** — Caveman compresses bytes; VISTA gives the agent *self-awareness* about context.

---

## 3. The differentiation opportunities for bbg

### D1. Cost-aware compression (not token-aware) — beats Caveman's premise
Instead of "minimize tokens," bbg's proxy should **minimize billed cost**:
- Model the provider's pricing (input, cached-input, output) per model.
- Before compressing a chunk, estimate: does the saved input cost exceed the **cache-break penalty** (re-reading a changed prefix)?
- **Never compress inside the stable cached prefix** — only compress content that is already outside cache or where savings > break cost.
- Report `estimated cost saved` (labeled `inferred`), not just `tokens saved`.
- This directly implements the "Token Reduction Is Not Cost Reduction" finding — Caveman's `cacheengine` plans prefixes, but a cost-aware compressor that *decides whether to compress at all* based on cache economics is a sharper, research-backed feature.

### D2. Context proprioception / visible state (VISTA-style) — Caveman doesn't have it
- Give the agent a **runtime context dashboard**: token budget used, remaining, per-section sizes, recency, archive status.
- Expose a `bbg context` command / SSE endpoint that injects a small, always-current **context summary** into the prompt: "context 62% used (41K/64K); system 3K; history 30K; tool results 8K; user msg 200."
- Let the agent make *informed keep-or-archive decisions* (VISTA's core) instead of blind compression.
- Integrate with the harness's Phase C compaction + Phase A ledger: bbg becomes the "visible state" layer over our storage.

### D3. Behavior-preserving compression with trajectory diffing — honesty beyond Caveman
- Caveman labels evidence `inferred` vs `verified`, but doesn't verify that compression *preserves behavior*.
- bbg can add a **behavior guard**: when compression would touch instructions/negations/action verbs, refuse (already in our AGORA guardrail) AND, in proxy mode, optionally diff the agent's trajectory with/without compression on a shadow request (eval-gated).
- This operationalizes AGORA's action-grammar warning as a *runtime* check, not just a heuristic.

### D4. Tool-menu shaping (ToolMenuBench, GIST-CMTF)
- ToolMenuBench: the visible tool menu shapes reliability/efficiency/risk more than tool correctness.
- GIST-CMTF / ToolChoiceConfusion: **causal minimal tool filtering** reduces wrong-tool calls, premature actions, and cost.
- bbg can add a `tools` subcommand: given the task + active tools, return the minimal visible tool set (with rationale) — cutting the largest token sink (tool schemas) AND improving reliability.
- This is complementary to compression: it removes *schema* tokens at the source.

### D5. Cache-aware structured output (schema preservation)
- For JSON/structured outputs, compress with schema awareness so downstream parsing is never broken (unlike lossy token deletion that breaks JSON).
- Combined with cache economics: JSON tool results are a cache-busting source; bbg can decide between TOON-style compact encoding vs keeping original-for-cache.

---

## 4. What bbg should NOT compete on (Caveman already wins)

- Byte compression of tool outputs (Caveman is mature there)
- Cache-prefix planning (their `cacheengine`)
- Reversible CCR storage (they have it)
- Multi-agent wrapping (they wrap 7 agents natively)

---

## 5. Recommended build order for differentiation

1. **D1 cost model** — add provider cost tables + "should we compress at all?" decision in the proxy. This is the research-backed differentiator with the sharpest story.
2. **D2 context visibility** — `bbg context` dashboard + prompt injection. VISTA-backed, model-agnostic, training-free.
3. **D4 tool-menu shaping** — `bbg tools` minimal-set recommendation. Large win, well-benchmarked.
4. **D3 behavior guard** — trajectory-diff honesty feature (eval-gated).
5. **D5 schema-aware JSON** — later.

---

## 6. Open question

Which differentiator(s) do we build first? My recommendation: **D1 (cost-aware) + D2 (context visibility)** together — they form a coherent "make context visible and economically sane" story that Caveman doesn't offer, and both are directly implementable in the existing Rust proxy.
