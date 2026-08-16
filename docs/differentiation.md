# bbg Differentiation Research — Final

**Date:** 2026-08-16 (third revision)
**Status:** Definitive — the "better way" has been identified with a validated citation, and the implementation space is open.

---

## The answer: Cache-Aware Prompt Compression (CAPC)

**"Cache-Aware Prompt Compression: A Two-Tier Cost Model for LLM API Caching"** (arXiv 2607.15516) is the well-regarded, peer-reviewed strategy that figures out the tradeoff neither Caveman nor TOON addresses. It's the "better way" — and it has **no production implementation**.

### What it proves (empirically, on Anthropic Sonnet 4.6)
1. **Caching is not free or ideal.** Sonnet's cache is a *two-tier* architecture with a sharp threshold near **3,500 tokens**; below it the hit rate plateaus at rho ≈ 0.83, not the rho=1.0 the literature assumes.
2. **Query-aware compression silently kills the cache.** Compression methods that produce a different prefix per query invalidate the prefix-strict cache every call — paying full price. This is the mechanism behind "Token Reduction Is Not Cost Reduction."
3. **The decision is a ratio tradeoff.** Under realistic cache-hit rates, query-aware compression only beats caching at high compression ratios (r ≥ 6).
4. **The fix — CAPC:** pair **query-agnostic compression** (stable prefix) with explicit `cache_control` + a **tier-preserving ratio bound** that stops over-compression from pushing the cached prefix into the hot (expensive) tier.

### Results
- Cheapest strategy in **16/16** configurations on LongBench-v2
- Mean savings: **49% over cache-only, 64% over query-aware compression, 90% over vanilla**
- Quality within **0.05** of uncompressed baseline
- Validated on production workloads incl. a 94k-token tool-schema prefix (**51.7% cost reduction**)

---

## Why this is bbg's winning position

| Tool | What it optimizes | Cache-aware? | Cost model? |
|---|---|---|---|
| Caveman | token count | has a cache *planner* but no cost model | no |
| TOON | token count (format) | no | no |
| tokenfold (19★) | token count | no | no |
| **bbg (proposed)** | **billed cost, not tokens** | **yes (CAPC-derived)** | **yes (two-tier rho)** |

**The implementation space is empty.** GitHub search shows only 0-1 star repos touching this (cache guardian, cost tracking). CAPC is the theory; **no one has shipped it as a tool.**

---

## What bbg becomes

A **cost-aware context proxy** built on the CAPC model:

1. **Cost model (per provider):** input / cached-input / output prices, plus the two-tier cache structure and realistic rho per session length. (Anthropic threshold ≈ 3,500 tokens; others differ.)
2. **The CAPC decision:** for each payload, compute whether compression pays:
   - **Query-agnostic** transforms only (stable prefix) — never per-query rewrites that break cache
   - Apply explicit `cache_control` on the stable prefix
   - Enforce the **tier-preserving ratio bound** (don't compress so hard the prefix drops into the hot tier)
3. **Integrate TOON** as the query-agnostic structured-data encoder (stable, deterministic — cache-friendly by construction)
4. **Report cost, not just tokens:** `estimated billed cost before/after`, `cache hit rate`, `tier status` (labeled `inferred` per Caveman's honest-evidence practice)
5. **D2 context visibility** (VISTA): inject live context budget/usage into the prompt so agents make informed keep-or-archive decisions

**Positioning:** *Caveman and TOON make context smaller. bbg makes context **economically optimal** — the only proxy that treats the cache as real, models the two-tier threshold, and tells you what it saved in dollars, not tokens.*

---

## Build order

1. **Cost model + CAPC decision engine** (Rust in the existing proxy): provider price tables, two-tier cache model, ratio-bound check, `should_compress` verdict
2. **TOON integration** as the stable-prefix encoder (crate or vendored reimplement)
3. **Cost/receipts reporting** in proxy responses (`x-bbg-cost-saved`, `x-bbg-cache-tier`)
4. **D2 context visibility** (dashboard + prompt injection)
5. Eval: replicate CAPC's LongBench-v2-style measurement for our own claims

---

## Sources
- CAPC: https://arxiv.org/abs/2607.15516
- Token Reduction Is Not Cost Reduction: https://arxiv.org/abs/2607.12161
- Don't Break the Cache: https://arxiv.org/abs/2601.06007
- TOON: https://github.com/toon-format/toon · spec: https://github.com/toon-format/spec
- VISTA: https://arxiv.org/abs/2606.30005
- Notation Matters: https://arxiv.org/abs/2605.29676
