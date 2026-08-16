# bbg Differentiation Research — Final (multi-answer edition)

**Date:** 2026-08-16 (fourth revision)
**Status:** Multiple validated approaches identified — each is a distinct, defensible bbg capability. The implementation space is open for all of them.

---

## The core insight (still true)

Token-reduction tools (Caveman, TOON, tokenfold) optimize **token count**, but "Token Reduction Is Not Cost Reduction" (arXiv 2607.12161) proved the real metric is **billed cost** — compression that breaks prompt-cache hits can cost *more*. Every approach below is a validated answer to *that* problem, not to "shrink tokens."

---

## Answer 1 — CAPC: Cache-Aware Prompt Compression (cost model)

**"Cache-Aware Prompt Compression: A Two-Tier Cost Model for LLM API Caching"** (arXiv 2607.15516)

- Anthropic's cache is **two-tier** with a sharp threshold at ~3,500 tokens; hit rate plateaus at rho≈0.83 (not the assumed 1.0)
- Query-aware compression invalidates the prefix cache every call → full price
- **CAPC:** query-agnostic compression + explicit `cache_control` + tier-preserving ratio bound (don't over-compress into the hot tier)
- Results: cheapest 16/16 LongBench-v2 configs; **49% over cache-only, 64% over query-aware, 90% over vanilla**; quality within 0.05; validated on a 94k-token tool-schema prefix (51.7% reduction)

**bbg capability:** provider cost model + "should we compress at all?" verdict + tier-aware ratio bound.

---

## Answer 2 — CWL: Structured Context Eviction (deterministic, LLM-free)

**"Beyond Compaction: Structured Context Eviction for Long-Horizon Agents"** (arXiv 2606.11213)

- Compaction (LLM summarization) is expensive and lossy; CWL instead does **graduated, semantically-aware eviction** with a **deterministic, LLM-free policy**
- The agent annotates its trajectory as typed, dependency-linked **episodes**; when budget is exceeded, evict in priority order: keep user turns + actively-reasoned-over context, shed action episodes **whose effects are already persisted in the environment**
- Keeps context near a stable ceiling, avoiding performance cliffs

**bbg capability:** episode-aware eviction policy — a deterministic alternative to LLM-compaction, cost ~0, no summarization quality risk.

---

## Answer 3 — VISTA: Context Proprioception / Visible State (visibility)

**"LLM Agents Are Latent Context Managers"** (arXiv 2606.30005)

- Models are **proprioceptively blind** to their own context (can't see size/recency/budget)
- VISTA = training-free layer: typed addressable blocks + runtime dashboard + recoverable archives
- Lifted Gemini-3-Flash 22.7% → 50.7% on LOCA-Bench

**bbg capability:** inject live context budget/usage/archive status into the prompt; agents make informed keep-or-archive decisions.

---

## Answer 4 — Governance Decay / ConstraintRot (safety-preserving eviction)

**"Governance Decay: How Context Compaction Silently Erases Safety Constraints"** (arXiv 2606.22528)

- **Compaction is a safety-critical failure surface**: in-context governance constraints reliably obeyed while visible get **silently removed by compaction**
- ConstraintRot benchmark: violations rise **0% → 30% after compaction, 59% for some models**; when the constraint survives, violations stay low
- **bbg capability:** constraint-survival guarantee — a protected-segments mechanism (like tokenfold's but for safety rules, not just system turns) that compaction/eviction must never drop; verify governance tokens survive every transform.

---

## Answer 5 — Tokalator: context-engineering toolkit (monitoring + economics education)

**"Tokalator: A Context Engineering Toolkit for AI Coding Assistants"** (arXiv 2604.08290)

- Open-source VS Code extension: **real-time budget monitoring**, 11 slash commands, 9 web calculators (Cobb-Douglas quality modeling, **caching break-even analysis**, O(T²) conversation-cost proofs), community catalog
- 17 stars — early, but the *concepts* (break-even analysis, budget monitoring) are validated
- **bbg capability:** break-even calculators + budget monitoring surfaced from the proxy — "at what session length does caching beat compression?" as a runtime answer.

---

## Answer 6 — Memory-vs-LongContext cost analysis (architecture choice)

**"Beyond the Context Window: Cost-Performance Analysis of Fact-Based Memory vs. Long-Context LLMs"** (arXiv 2603.04814)

- Memory systems (Mem0-style) vs long-context LLMs: **neither wins universally** — depends on task (recall vs persona-consistency) and on a cost model that incorporates prompt caching
- **bbg capability:** a routing advisor — "this task wants full context; this one wants fact memory" based on measured cost+accuracy, rather than one-size.

---

## The synthesis: bbg = the context-operations layer

Each answer is a *policy* a context proxy can implement. bbg's coherent identity:

> **bbg makes context economically optimal, visibly managed, and safety-preserving — the operations layer under any agent.**

Capability map (all Rust, in the existing proxy):

| # | Capability | Validated by | Build cost |
|---|---|---|---|
| 1 | Cost model + cache-aware compression verdict | CAPC (2607.15516) | Medium |
| 2 | Deterministic structured eviction (CWL) | CWL (2606.11213) | Medium |
| 3 | Context visibility dashboard | VISTA (2606.30005) | Low |
| 4 | Constraint-survival guarantee | Governance Decay (2606.22528) | Low-Med |
| 5 | Break-even calculators + budget monitor | Tokalator (2604.08290) | Low |
| 6 | Memory-vs-longcontext routing advisor | 2603.04814 | Med-High |

---

## Recommended build order

1. **#3 visibility + #1 cost model** first (coherent, low-risk, immediate value; the "visible + economically sane" story)
2. **#4 constraint-survival** (safety — differentiator no compression tool has)
3. **#2 structured eviction** (the LLM-free compaction alternative)
4. **#5 calculators, #6 routing** later (tooling/advisor layer)

---

## Sources
- CAPC: https://arxiv.org/abs/2607.15516
- Token Reduction Is Not Cost Reduction: https://arxiv.org/abs/2607.12161
- Don't Break the Cache: https://arxiv.org/abs/2601.06007
- CWL / Beyond Compaction: https://arxiv.org/abs/2606.11213
- VISTA: https://arxiv.org/abs/2606.30005
- Governance Decay / ConstraintRot: https://arxiv.org/abs/2606.22528
- Tokalator: https://arxiv.org/abs/2604.08290
- Memory vs Long-Context: https://arxiv.org/abs/2603.04814
- TOON: https://github.com/toon-format/toon · spec: https://github.com/toon-format/spec
