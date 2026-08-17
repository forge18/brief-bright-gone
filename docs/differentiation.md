# bbg Options Survey — Complete Landscape (v3)

**Date:** 2026-08-16
**Status:** Full survey of well-regarded tools + academic approaches. Gap analysis for bbg.

---

## The well-regarded tools (stars = community validation)

| Tool | Stars | Architecture | What it does | License |
|---|---|---|---|---|
| **Caveman** | 98K | skill + proxy + cacheengine + CCR | output terse + input compression | MIT/BSL |
| **OpenViking** (ByteDance) | 28.6K | context DB (`viking://` FS) | L0/L1/L2 tiered loading, -34-91% tokens, LoCoMo 24→82% | AGPLv3 |
| **TOON** | 25.2K | format | token-efficient structured data (lossless round-trip) | MIT |
| **context-mode** | 19.9K | MCP server | sandbox tools (98%), FTS5 session continuity, "think in code" | ELv2 |
| **paritok** | 1.4K | drop-in proxy gateway | tool-schema filter + 4B model content compression + history summarization; non-destructive (`read_original`); cache-friendly frozen tool selection; embedding tool filter (CPU-local) | Apache-2.0 |
| **SuperCompress** | 55 | query-aware compression | keep evidence, drop filler | MIT |
| **nocturne_memory** | 1.3K | MCP memory server | rollbackable long-term memory, SQLite | MIT |

---

## paritok — the closest existing tool to "our bbg" (key reference)

**What it gets right (we should learn from):**
- **Drop-in proxy** — agent points `BASE_URL` at it; zero agent changes
- **Non-destructive** — everything compressed is recoverable via `read_original` (local, instant)
- **Cache-friendly tool filtering** — tool selection *frozen per conversation* so the `tools[]` block stays byte-stable → never invalidates KV cache. Directly implements the "Don't Break the Cache" lesson.
- **Three levers ranked by impact**: (1) tool-schema filter = biggest single-turn win (29K→8K tokens), (2) content compression, (3) history summarization
- **Trained 4B compression model** (45K trajectories) — knows signatures from debug lines, protects identifiers/errors
- Results: 25% cut turn-one, 85%+ in long sessions

**What it does NOT do (the gap):**
- No **cost model** (billed-cost-aware "should we compress?" decisions) — it compresses always, doesn't weigh cache-break economics
- No **context visibility** (agent can't see its own budget/usage — VISTA)
- No **constraint-survival guarantee** (Governance Decay risk not addressed)
- No honest **cost-vs-accuracy reporting** beyond marketing claims

---

## The genuinely unclaimed space (bbg's defensible position)

Across ALL surveyed tools (Caveman, OpenViking, TOON, context-mode, paritok, SuperCompress):

1. **Cost-modeled compression verdict** (CAPC, arXiv 2607.15516) — no tool decides *whether* compression pays for itself under provider pricing + cache economics
2. **Context proprioception / visibility** (VISTA, 2606.30005) — no tool injects live budget/usage into the prompt
3. **Constraint-survival guarantee** (Governance Decay, 2606.22528) — no tool guarantees safety constraints survive eviction
4. **Honest `inferred`/`verified` cost+accuracy reporting** — Caveman labels evidence honestly but doesn't tie it to cost; nobody else does either

---

## The research-backed caution (still applies)

context-mode cites Moonshot AI: **aggressive brevity prompts degrade coding/reasoning benchmarks.** The winning tools move work *out of* context (sandbox/tiers/retrieval/compression) — they don't make the model talk terser. → Drop the output-side terse-mode idea; focus context-side.

---

## Options for bbg's path forward

**Option A — Differentiate on the gaps (recommended):** build the cost model + visibility + constraint-survival as a layer *complementary* to existing tools. bbg = "context economics + safety + visibility," not another compressor.

**Option B — Integrate paritok's approach:** adopt the drop-in-proxy + non-destructive + frozen-tool-selection pattern (Apache-2.0, we can learn from it), implement in Rust. More proven path but competes directly with paritok.

**Option C — Hybrid:** take paritok's architecture lessons (frozen tool selection, non-destructive recovery, ranked levers) AND add the unclaimed capabilities (cost model, visibility, constraint-survival). bbg = the *next-gen* context gateway that existing tools converge toward.

**My recommendation: Option C** — the landscape is converging on "drop-in proxy + non-destructive + cache-aware," and the unclaimed differentiators (cost, visibility, safety) are the ones with peer-reviewed validation. bbg doesn't need to beat Caveman/paritok at compression; it needs to be the layer that makes compression *smart, visible, and safe*.

---

## Sources
- paritok: https://github.com/Paritok-official/paritok-4b-v1
- SuperCompress: https://github.com/Supercompress/Supercompress
- context-mode: https://github.com/mksglu/context-mode
- OpenViking: https://github.com/volcengine/OpenViking
- TOON: https://github.com/toon-format/toon
- nocturne_memory: https://github.com/Dataojitori/nocturne_memory
- Papers: CAPC 2607.15516 · VISTA 2606.30005 · Governance Decay 2606.22528 · Token Reduction 2607.12161 · Don't Break Cache 2601.06007 · CWL 2606.11213 · ToolBudgetBench github LI-Jialu
