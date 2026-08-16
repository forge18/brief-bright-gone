# bbg Differentiation Research — Corrected (v2)

**Date:** 2026-08-16 (revised after deeper search)
**Status:** Updated to reflect the discovery of TOON as the well-regarded existing solution.

---

## The corrected picture

**The user's instinct was right: there IS a well-regarded tool that figured this out.** It's **TOON — Token-Oriented Object Notation** ([toon-format/toon](https://github.com/toon-format/toon)):

- **25,180 stars**, 1,115 forks — the leading community-validated answer to "structured data costs too many tokens in LLM prompts"
- **Official spec** ([toon-format/spec](https://github.com/toon-format/spec), v4.1) with strict-mode validation, conformance requirements, canonical number handling, and explicit error semantics
- **MIT licensed**, TypeScript SDK + CLI + benchmarks, active (pushed Aug 2026)
- **Ecosystem**: implementations in Java, Python, Laravel, Delphi, JVM — real adoption beyond one language

**What TOON does** (the D5 we were designing, done well):
- Declares array length + field names **once** in a header, then one row per element — CSV-like compactness with explicit structure
- Nested uniform objects fold into the header (`temp{min,max}`)
- **Deterministic, lossless round-trips** back to the JSON data model
- "LLM-Friendly Guardrails": explicit `[N]` lengths + `{fields}` lists give models a clear schema, **improving parsing reliability**
- **When-not-to-use is explicit**: deeply nested / non-uniform data → JSON wins; semi-uniform → stay on JSON; purely tabular → CSV is smaller

**Its risk-minimization design** (matching what we spec'd):
- Lossless by construction (JSON-model equality rules in the spec)
- Strict-mode errors with authoritative diagnostics
- Explicit out-of-domain number policy (lossless-first recommended)
- Deterministic key ordering, documented delimiter scoping

---

## The corrected recommendation

**Do NOT build our own D5.** TOON is the well-regarded, community-validated, MIT solution with an official spec. Rebuilding it would be reinventing a solved problem with 25K stars of validation. (tokenfold was the 19-star clue that led here; TOON is the real answer.)

**What bbg should be instead — the genuinely unclaimed space:**

Neither **Caveman** (byte compression + cache planner) nor **TOON** (token-efficient structured format) addresses:

1. **D1 — Cost-aware compression.** Both optimize *token count*. The "Token Reduction Is Not Cost Reduction" paper (arXiv 2607.12161) proved that's the wrong metric: compression that breaks prompt-cache hits can *increase* billed cost (measured: -38.4% tokens, +6.8% cost). A cost-aware engine deciding *whether* to compress based on provider pricing + cache economics is unclaimed.

2. **D2 — Context visibility (VISTA).** Models are proprioceptively blind to their own context — can't see token usage, recency, or remaining budget. VISTA (arXiv 2606.30005) showed making context visible lifts agent performance (22.7% → 50.7%). Neither tool gives agents this self-awareness.

3. **Adoption as a format layer:** bbg can **integrate TOON** (as the structured-data encoder) rather than compete with it — the translation layer ("use JSON programmatically, encode to TOON for LLM input") is exactly what an agent proxy should do.

---

## Revised build strategy

| Capability | Decision | Why |
|---|---|---|
| Structured-data token compression | **Integrate TOON** (crate/CLI) | 25K-star validated solution; don't rebuild |
| Cost-aware "should we compress?" | **Build (D1)** | Unclaimed by Caveman/TOON; research-backed |
| Context visibility dashboard | **Build (D2)** | Unclaimed; VISTA-validated |
| Byte compression / output style | Reference Caveman's approach | Already solved; don't compete |
| Cache-prefix planning | Reference Caveman's cacheengine | Already solved |

**Positioning statement for bbg:** *Caveman and TOON make context smaller. bbg makes context **visible and economically sane** — deciding whether compression pays for itself, and telling the agent what it's actually working with.*

---

## Sources
- TOON: https://github.com/toon-format/toon · https://github.com/toon-format/spec
- Token Reduction Is Not Cost Reduction: https://arxiv.org/abs/2607.12161
- Don't Break the Cache: https://arxiv.org/abs/2601.06007
- VISTA / LLM Agents Are Latent Context Managers: https://arxiv.org/abs/2606.30005
- Notation Matters (TOON/TRON in agentic loops): https://arxiv.org/abs/2605.29676
