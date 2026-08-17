# bbg Differentiation Research — Complete Landscape

**Date:** 2026-08-16 (final)
**Status:** Eight validated approaches + two well-regarded tools mapped. Includes a research-backed caution that changes one of our earlier ideas.

---

## The well-regarded tools that exist (must-know before building)

| Tool | Stars | Approach | License |
|---|---|---|---|
| **TOON** | 25.2K | Token-efficient structured format (declared headers, lossless round-trip) | MIT |
| **OpenViking** (Volcengine/ByteDance) | 28.6K | Context database: `viking://` virtual FS, L0/L1/L2 tiered loading on demand | AGPLv3 |
| **context-mode** | 19.9K | MCP server: sandbox tools (98% reduction), SQLite FTS5 session continuity, "think in code" | ELv2 |
| **Caveman** | 98K | Output skill + input proxy + cacheengine + CCR | MIT/BSL |

### OpenViking — the tiered-loading design (validated, big numbers)
- Content stored as `viking://` filesystem; agent browses with `ls`/`tree`/`find` (deterministic, debuggable — no black-box vector store)
- **Three tiers on write**: L0 abstract (~100 tokens) → L1 overview (~2k) → L2 full details (loaded on demand)
- Results: LoCoMo accuracy 24-57% native → **80-83% with OpenViking**, input tokens **-34% to -91%**, latency -58-66%
- This is the "reversible, just-in-time" pattern (same as our Phase A artifacts) — but mature, with benchmark numbers

### context-mode — the "think in code" paradigm + session continuity
- **Sandbox tools** keep raw data out: 315KB → 5.4KB (98%)
- **SQLite + FTS5 + BM25**: indexes events (edits, git ops, decisions); compaction doesn't dump it back — retrieves only what's relevant
- **"Think in code"**: the LLM should *program* the analysis (one `ctx_execute` script replaces 47 reads = 700KB → 3.6KB), not be a data processor
- **CRITICAL CAUTION: "No prose-style enforcement"** — context-mode explicitly refuses to dictate how the model writes, citing: **"aggressive brevity prompts have been shown to degrade coding/reasoning benchmarks"** (Moonshot AI on kimi-k2.5). This challenges our earlier terse-mode/Be-Brief-Bright-Gone idea.

---

## The eight validated approaches (the answers)

| # | Approach | Paper/Tool | Core idea | bbg capability |
|---|---|---|---|---|
| 1 | **CAPC — cache-aware compression** | 2607.15516 | two-tier cache cost model; compress only when it pays | cost model + verdict |
| 2 | **CWL — structured eviction** | 2606.11213 | deterministic LLM-free episode eviction | eviction policy |
| 3 | **VISTA — context proprioception** | 2606.30005 | make context visible (budget/recency) | visibility dashboard |
| 4 | **Governance Decay / ConstraintRot** | 2606.22528 | compaction erases safety constraints (0→30→59%) | constraint-survival guarantee |
| 5 | **Tokalator** | 2604.08290 | break-even calculators + budget monitor | economics tooling |
| 6 | **Memory vs long-context** | 2603.04814 | neither wins universally; cost-modeled routing | routing advisor |
| 7 | **ToolBudgetBench / progressive disclosure** | github LI-Jialu | adaptive tool portfolios beat all-tools (0.727 vs 0.265, 443 vs 22k tokens) | tool-schema disclosure |
| 8 | **Tiered loading (L0/L1/L2)** | OpenViking | abstract→overview→details on demand | reversible just-in-time tiers |

---

## The critical lesson that changes our plan

**context-mode's stance + Moonshot citation:** aggressive brevity *prompts* (output-side terse-mode) **degrade coding/reasoning benchmarks**. The winning tools all move work OUT of context (sandbox, tiers, FTS5 retrieval) — they do NOT make the model talk terser.

**Implication for bbg:** deprioritize the output-side "terse style" entirely. The validated play is **context-side**:
- Keep raw data out (sandbox/tiered/CCR)
- Make retrieval cheap and deterministic (FTS5/BM25, viking://-style)
- Make the economics visible (CAPC cost model)
- Preserve safety constraints across eviction (ConstraintRot)

This also retroactively validates our Phase A (SQLite artifact store) and Phase C (compaction) direction — they're the same pattern as OpenViking/context-mode, just less mature.

---

## What's genuinely unclaimed (the defensible bbg space)

1. **Cost-modeled compression verdict (CAPC)** — no shipped tool; Caveman has a cache *planner* but no cost model, context-mode has no cost model
2. **Constraint-survival guarantee** — no tool guarantees governance tokens survive compaction/eviction
3. **Context proprioception injected into prompt (VISTA)** — context-mode/OpenViking manage context but don't make the model *aware of its own budget* in-prompt
4. **Cross-tool evaluation/benchmarks** — ToolBudgetBench shows measurement is a gap; bbg could ship honest `inferred`/`verified` cost+accuracy reporting

**Where bbg should NOT compete:** byte compression (Caveman), structured format (TOON), tiered context DB (OpenViking), sandbox+retrieval MCP (context-mode), output-style terse-mode (research says it hurts).

---

## Recommended final positioning

> **bbg is the context-economics and context-safety layer**: it tells you (and the model) what context costs (CAPC), guarantees safety constraints survive eviction (ConstraintRot), and makes context visible (VISTA) — the operations layer under any agent, complementary to TOON/OpenViking/context-mode rather than competing with them.

Build order: **#1 cost model + #3 visibility** first (coherent, low-risk), **#4 constraint-survival** second (safety differentiator), **#2 structured eviction** third. Drop the terse-mode output-style direction per the Moonshot evidence.

---

## Sources
- context-mode: https://github.com/mksglu/context-mode · Moonshot brevity citation: anomalyco/opencode#20258
- OpenViking: https://github.com/volcengine/OpenViking
- TOON: https://github.com/toon-format/toon
- CAPC: https://arxiv.org/abs/2607.15516 · Token Reduction: 2607.12161 · Don't Break Cache: 2601.06007
- CWL: 2606.11213 · VISTA: 2606.30005 · Governance Decay: 2606.22528 · Tokalator: 2604.08290 · Memory vs LC: 2603.04814
- ToolBudgetBench: https://github.com/LI-Jialu/ToolBudgetBench
