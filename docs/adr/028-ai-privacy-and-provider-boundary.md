# ADR-028: AI Privacy and Provider Boundary

**Status**: Accepted  
**Date**: 2026-07-09  
**Deciders**: kafkade

## Context

Epic #36 (moonshot features) includes AI-assisted organization and retrieval:
summarization, suggested tags, highlight suggestions, semantic "related content,"
question answering over the local library, digest prioritization, and turning
highlights into draft flashcards (roadmap §7.1). This ADR defines the **privacy
and provider boundary** for all of that. It is the AI-track prerequisite in the
epic and blocks #136 (AI-assisted organization/retrieval, which owns the concrete
`pergamon-ai` crate and provider plumbing) and #140 (semantic clustering and
topic views). It decides policy, not implementation: no crate or code ships with
this ADR.

> **Numbering note.** Epic #36 named this "ADR-026", but ADR numbers 025–027 were
> assigned to sync topics before the AI track was scheduled. This ADR takes the
> next free number, **028**; the epic's "ADR-026" label refers to this document.

pergamon's product identity is local-first ownership of a durable personal
archive (ADR-001, ADR-016) with reproducible, exportable content (ADR-010). AI is
explicitly a **Phase 8 moonshot** that must not distort that identity. The
roadmap is unambiguous about the guardrails:

- **No mandatory AI dependency.** Core knowledge retention — ingestion, storage,
  search, resurfacing, spaced repetition, export — must work fully offline and
  remain reproducible without any model (roadmap Q17, §7.1, Phase 8 cut line).
- **BYOK / BYOM only.** Support AI as *Bring Your Own Key* (cloud APIs) and
  *Bring Your Own Model* (local runtimes like Ollama or LM Studio). **pergamon
  does not operate a hosted inference service** (roadmap §7.1).
- **Privacy-bounded, opt-in.** "AI features are disabled by default or strictly
  privacy-bounded; no plaintext leaves the user's device without explicit opt-in"
  (Phase 8 acceptance criteria).
- **Derived artifacts with provenance; never authoritative.** AI output is a
  *suggestion* — a derived note, draft card, or proposed tag — recorded with
  provenance and an approval state, never a silent mutation of source content or
  metadata (roadmap §7.1 product rule).
- **No server-side embeddings by default. No hosted recommendation feed.** Both
  are on the Phase 8 cut line.

This sits on top of several accepted boundaries and must not weaken any of them:

- **ADR-001 / ADR-007** — `pergamon-core` is zero-I/O and platform-agnostic; all
  network and filesystem I/O lives in platform code outside the core. AI, which
  is inherently I/O (calling a local runtime or a remote API), cannot live in the
  core.
- **ADR-008** — Apache-2.0 for client crates; AGPL-3.0 only for server code.
- **ADR-022 / ADR-024 / ADR-026** — multi-device sync is a **blind relay** that
  stores only client-encrypted ciphertext. The relay and the `pergamon-server`
  have no plaintext and no ingestion pipeline; they are structurally incapable of
  running inference. AI must stay a client-side concern for the same reason
  capture does (ADR-027).
- **ADR-021 / ADR-027** — capture and ingestion already have one pipeline and one
  local-first posture; AI plugs into content *after* it is ingested locally.

The question this ADR answers: **what is the provider model, and exactly what
data may leave the device, under what consent?**

## Decision

### 1. AI lives behind a `pergamon-ai` crate, outside the zero-I/O core

All AI functionality is confined to a new client-side crate, **`pergamon-ai`**
(Apache-2.0, ADR-008), added by #136. `pergamon-core` stays zero-I/O (ADR-001)
and gains no model, network, or provider types. Prompt construction, redaction
policy, and provider dispatch live in `pergamon-ai`; the actual network/process
I/O to a provider lives at the platform edge (CLI/server/UniFFI), consistent with
ADR-007. The core may define pure, provider-agnostic value types for *storing* an
AI artifact (see §5), but never anything that performs or triggers inference.

### 2. Provider abstraction: local-first, BYOK remote — never pergamon-hosted

`pergamon-ai` exposes a single `Provider` trait (exact signature owned by #136)
with two concrete kinds:

| Kind | Examples | Data locality |
|------|----------|---------------|
| **On-device / local runtime** | Ollama, LM Studio, llama.cpp, a bundled small model | Content never leaves the machine |
| **BYOK remote** | User-supplied key to an OpenAI-compatible endpoint, Anthropic, etc. | Content is sent to the third party the user chose, per request |

Rules:

- **pergamon operates no inference service and ships no default API key or default
  remote endpoint.** There is no "pergamon AI" backend and no pergamon account
  involved in inference.
- **On-device is the default and recommended provider kind.** Where a local
  runtime is available, it is preferred; remote is an explicit user choice.
- The provider is **user-configured**: which kind, which endpoint/model, and (for
  remote) the user's own key, stored locally using the same key-handling posture
  as other secrets (encrypted-file / OS keychain, cf. ADR-024 / `pergamon-
  keystore`). Keys are never synced through the blind relay in plaintext and
  never logged.
- The same `Provider` abstraction is used by every platform (CLI, self-hosted
  web, iOS) so the privacy contract is identical everywhere.

### 3. Strict default: AI is disabled

AI is **off by default**. Until the user explicitly enables it *and* configures a
provider:

- No inference runs, local or remote.
- No implicit network calls are made — no "phone home," no model download without
  consent, no telemetry, no usage analytics.
- Core features behave exactly as they do today; nothing degrades or nags.

Enabling AI is a deliberate, discoverable action (a CLI command / settings
toggle) that names the provider being configured.

### 4. Data egress contract: nothing leaves the device without explicit opt-in

This is the crux of the boundary.

- **Local providers keep everything on-device.** Choosing an on-device runtime
  means content never crosses the machine boundary, and no separate egress
  consent is required beyond enabling the feature.
- **A BYOK remote provider is the *only* path that transmits content off-device**,
  and only for the **specific operation the user invoked** (e.g. "summarize *this*
  document," "suggest tags for *this* item"). There is:
  - **no background egress** — AI never runs on a schedule, on ingest, or on
    sync;
  - **no bulk egress** — the whole library is never shipped to a provider;
  - **no ambient indexing** — no automatic embedding/upload of content to a
    remote service (server-side embeddings are on the roadmap cut line).
- **Per-scope, informed opt-in.** Before content is sent to a remote provider,
  the surface makes it explicit *what* will be sent and *to whom* ("This will send
  the text of ‹title› to ‹provider›"). Consent is per remote provider, is
  revocable, and remote calls are auditable (a local record of which operation
  sent which item to which provider and when). A first remote call requires an
  explicit acknowledgement.
- **The sync path is not an AI path.** Content reaches a provider only through a
  user-invoked local AI operation, never as a side effect of sync. The relay and
  server never see plaintext and never call a provider (ADR-022/024/026).

### 5. Data minimization and redaction

For any egress to a remote provider, `pergamon-ai` minimizes and sanitizes:

- **Minimum necessary context.** Send only the passage/document/highlights the
  operation needs — not adjacent items, not collection structure, not unrelated
  metadata. Question-answering over the library retrieves and sends only the
  matched excerpts required to answer, not the corpus.
- **Redaction before egress.** Strip obviously sensitive material before sending:
  credential-shaped strings (API keys, tokens, passwords), and a configurable set
  of user-designated sensitive fields/tags/collections that are **excluded from
  remote AI entirely** (an allow/deny policy — e.g. a "private" collection never
  leaves the device even with AI enabled). Redaction is a no-op safety net for
  local providers but always applied uniformly.
- **No metadata leakage.** Do not attach account identifiers, device identifiers,
  file paths, or library-wide statistics to provider requests.
- **Truthful cost/size surfacing.** Because remote calls have cost and size, the
  surface can show what will be sent; nothing is silently expanded.

### 6. Provenance and non-authority of AI output

AI output is **never authoritative** and never silently mutates the archive:

- Every AI result is stored as a **derived artifact with provenance**: provider +
  model identifier, prompt-template version, generation timestamp, and an
  **approval state** (suggested → accepted/rejected). This mirrors roadmap §7.1
  and keeps AI reproducible and auditable.
- **Suggestions stay suggestions until the user accepts them**: suggested tags are
  proposals, suggested highlights are proposals, summaries are derived notes,
  generated flashcards are drafts. Source content and existing metadata are never
  overwritten by AI; acceptance is an explicit user act.
- Derived artifacts are **first-class, exportable, and deletable** — export
  (ADR-024 / export contracts) must not become worse because AI is involved, and
  a user can strip AI-derived content cleanly. This preserves the "makes export or
  reproducibility worse" prohibition from roadmap §7.1.

### 7. Server and sync boundary

- The AGPL `pergamon-server` and the `pergamon-sync-server` blind relay **never
  run inference and never see plaintext for AI purposes.** A self-hoster who
  enables AI on their own server instance is acting as their own client and
  configures their own provider/key there; the relay is still blind.
- AI is a **client-side, local-first** capability. There is no server-mediated AI,
  no hosted embedding index, and no cross-user AI feature.

## Consequences

### Positive

- **Trust story intact by construction.** AI is off by default, local-first, and
  BYOK/BYOM; no plaintext leaves the device without an explicit, per-scope opt-in,
  satisfying the Phase 8 acceptance criteria directly.
- **No new trusted third party and no pergamon inference cost.** Users bring their
  own model or key; pergamon runs no inference service and holds no keys.
- **Core stays clean.** `pergamon-core` remains zero-I/O; AI I/O is quarantined in
  `pergamon-ai` at the platform edge, so the core is still WASM-friendly and
  reproducible.
- **Reproducibility and ownership preserved.** AI output is derived, provenanced,
  non-authoritative, and exportable/deletable, so the archive never becomes an
  opaque AI-mutated blob and export never degrades.
- **Uniform privacy contract across platforms.** One `Provider` abstraction means
  CLI, web, and iOS enforce the same egress and consent rules.
- **Sync remains a blind relay.** AI never rides the sync path; the server never
  sees plaintext for inference.

### Negative

- **Setup friction.** BYOK/BYOM means the user must install a local runtime or
  paste a key before any AI works — less "magical out of the box" than incumbents
  with hosted AI. This is the deliberate price of the privacy boundary.
- **Local model quality/perf varies.** On-device inference on modest desktop or
  mobile hardware is weaker than frontier hosted models; users wanting top quality
  must opt into a remote provider and accept egress.
- **Redaction is best-effort.** Credential/secret stripping and sensitive-scope
  exclusion reduce but cannot fully guarantee that a remote provider never
  receives something the user considers private within the content they chose to
  send; the allow/deny policy and per-operation transparency mitigate this.
- **More surface to build and test.** Consent UI, provenance storage, redaction,
  and provider config add real work in #136 — but the policy here keeps that work
  bounded and optional.

### Neutral / follow-ups

- #136 owns the `pergamon-ai` crate, the `Provider` trait signature, the artifact
  schema/migrations, prompt-template versioning, and the per-surface consent UX.
- #140 (semantic clustering / topic views) must respect this boundary: any
  embeddings are computed locally by default; no ambient remote indexing.
- A future ADR may revisit an *optional*, explicitly-opt-in local semantic index
  and its portability across SQLite/WASM/iOS (roadmap §7.1 complexity drivers);
  it does not change this boundary.

## Rejected Alternatives

- **A pergamon-hosted inference service or a default cloud key.** Rejected: it
  introduces a trusted third party, an account, ongoing cost, and a data-egress
  path pergamon controls — the opposite of local-first ownership. BYOK/BYOM keeps
  the user in control of both the model and the data (roadmap §7.1).

- **AI on by default, or as a dependency of core features.** Rejected: core
  knowledge retention must work fully offline and reproducibly with no model
  (roadmap Q17, Phase 8 cut line). AI is assistive and optional; making it
  mandatory would weaken the trust story and break offline/reproducible use.

- **Ambient/background egress — auto-summarize on ingest, auto-embed the library
  to a remote service, server-side embeddings by default.** Rejected: it ships
  content off-device without a per-operation choice and lands squarely on the
  Phase 8 cut line ("no server-side embeddings by default"). Egress is only ever
  a user-invoked, per-scope action.

- **Sending whole-library or broad context to a remote provider "for better
  answers."** Rejected: it maximizes egress and cost and leaks structure/metadata.
  Minimization sends only the excerpts an operation needs.

- **Putting AI I/O inside `pergamon-core`.** Rejected: it violates the zero-I/O
  boundary (ADR-001/007), breaks WASM-friendliness, and couples the pure domain
  to networking. AI I/O belongs in `pergamon-ai` at the platform edge.

- **Storing AI output as authoritative edits to source content or metadata (silent
  rewriting, auto-applied tags).** Rejected: it destroys reproducibility and
  ownership and violates roadmap §7.1's product rule. AI output is a derived,
  provenanced suggestion until the user accepts it.

- **Running AI on the sync server / relay.** Rejected: the relay is blind
  (ciphertext only, ADR-022/024/026) and cannot see plaintext; server-side AI
  would require breaking end-to-end encryption. AI stays client-side.

- **A pergamon-run "sensitive data" scrubbing service in the cloud.** Rejected:
  redaction that itself requires shipping content to pergamon defeats the purpose;
  minimization and redaction run locally in `pergamon-ai` before any egress.
