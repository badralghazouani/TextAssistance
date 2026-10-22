# Plan: Rust writing assistant, personal use on macOS first, then SaaS

## Context
You want a Grammarly-like writing assistant. It starts as a tool for your own use and later becomes a SaaS product. Decisions so far:
- **First platform:** macOS.
- **Language:** Rust core, so the code can go cross-platform later.
- **LLM providers:** Claude and OpenAI, switchable in settings.
- **Model tier:** budget (cheap model for checks, mid-tier model for rewrites).

The architecture is in two stages:
1. **Personal stage:** no server. The app calls the LLM API directly with your own API key.
2. **SaaS stage:** add a Rust backend that holds the API keys, handles accounts and billing, and tracks each user's usage.

The client is built so that moving to stage 2 means swapping one component, not rewriting the app.

This repository (`writer`) holds the app.

---

## 1. Stage 1: personal macOS app (no backend)

```
macOS app (Rust + Tauri 2 menu-bar)
 ├─ platform-macos: AXUIElement observer → focused text + caret rect
 ├─ core: paragraph hashing, debounce (400 ms / max 2 s), anchor rebasing
 ├─ spell: offline spell-check (spellbook + Hunspell en/fr dictionaries) → instant
 ├─ privacy: PII redaction (emails, phones, IBAN, cards) before any network call
 ├─ llm: trait LlmProvider { check(), rewrite() }
 │    ├─ ClaudeProvider  (reqwest → POST api.anthropic.com/v1/messages)
 │    └─ OpenAiProvider  (reqwest → POST api.openai.com/v1/responses)
 ├─ store: SQLite cache (paragraph hash → suggestions), dictionary, usage log
 └─ overlay: non-activating NSPanel with underlines + suggestion card
API keys live in the macOS Keychain (`keyring` crate) and are entered in Settings.
```

### Cargo workspace (new repo)
```
writer/
├─ crates/core        # doc model, diff, scheduler, rebasing (pure Rust, unit-tested)
├─ crates/protocol    # Suggestion, CheckRequest types (serde) — reused by the server later
├─ crates/privacy     # PII redaction + rehydration
├─ crates/spell       # offline spelling
├─ crates/llm         # LlmProvider trait + Claude/OpenAI adapters + cost meter
├─ crates/store       # rusqlite cache, settings, usage table
├─ crates/platform-macos  # objc2 / accessibility bindings, NSPanel overlay
└─ apps/desktop       # Tauri 2: tray icon, Settings (provider, model, key, monthly cap), usage page
```

### LLM layer (`crates/llm`)
- **Common trait.** Both adapters implement:
  `async fn check(&self, para: &str, ctx: &AppContext) -> Result<(Vec<Suggestion>, Usage)>`
  `async fn rewrite(&self, text: &str, mode: RewriteMode) -> Stream<Chunk>`.
- **HTTP client.** Neither Anthropic nor OpenAI publishes an official Rust SDK, so use `reqwest` + `serde` against each REST API directly. Stream with SSE (`eventsource-stream`).
- **Structured output.** Both adapters ask for JSON that matches a schema: `{suggestions:[{start,end,kind,replacement,explanation}]}`.
  - Claude: `output_config.format` (JSON schema).
  - OpenAI: `text.format` with `json_schema` in the Responses API.
  - Offsets are counted in UTF-16 and checked against the paragraph text. Spans that don't match are dropped.
- **Claude settings:**
  - Checks use `claude-haiku-5-5`: `output_config.effort: "low"`, `thinking: {type:"disabled"}` (allowed on Haiku 5.5 at effort high or below), and keep `max_tokens` small (~1024).
  - Rewrites use `claude-sonnet-5-5`, streamed, effort `low`/`medium`, plus the server-side `fallbacks: "default"` beta for refusals.
  - Don't send `temperature`. Sampling parameters now return a 400.
- **OpenAI settings:** checks use `gpt-6-luna`, rewrites use `gpt-6.1-sol`.
- **Prompt caching.** Keep the system prompt fixed, byte for byte, so it is cached; put the paragraph last. Haiku has a minimum prompt length for caching, so a short prompt may not cache. That's acceptable because cost is negligible either way.
- **Cost meter.** Read `usage` from every response, multiply by the price table in `crates/llm/pricing.rs`, and store the result in SQLite. A **monthly spend cap** set in Settings blocks LLM calls once it's reached; spell-check keeps working. Also set a spending limit in each provider's console.

### Personal-stage milestones
1. Core, spell-check and privacy crates with unit tests (no UI yet).
2. A CLI test harness: `writer check file.txt --provider claude|openai`, which shows suggestions and their cost.
3. macOS accessibility capture plus the overlay in TextEdit, Notes, Mail, Slack and Chrome.
4. Apply suggestions (select range → set `AXSelectedText`; fallback to pasteboard + Cmd+V). Check the text's hash before applying.
5. Tauri tray and settings: choose provider and model, enter API key (stored in Keychain), monthly cap, per-app on/off, usage page.

---

## 2. API prices (checked 2026-10-22; confirm on the vendors' pricing pages before relying on them)

All prices are **USD per 1 million tokens**.

| Use | Claude | In / Out | OpenAI | In / Out |
|---|---|---|---|---|
| Grammar checks (cheap) | Haiku 5.5 | $0.10 / $0.50 | GPT-6 Luna | $0.10 / $0.50 (cached in $0.01) |
| Rewrites (mid) | Sonnet 5.5 | $2 / $10 (cache read $0.20) | GPT-6.1 Sol | $2 / $10 (cached in $0.10) |
| Premium rewrites | Opus 5.5 | $4 / $20 | GPT-6 Astra | $10 / $50 |

Batch APIs cost 50% less, but they aren't real-time, so they only suit offline jobs.

### Your personal monthly cost (heavy use)
Assumptions:
- **Checks:** 400 per day = 12,000 per month. Each one is about 600 system-prompt tokens (cached), 150 paragraph tokens and about 150 output tokens.
- **Rewrites:** 20 per day = 600 per month. Each one is about 800 input tokens and about 600 output tokens.

| Tier | Checks | Rewrites | **Total / month** |
|---|---|---|---|
| **Budget (chosen):** Haiku 5.5 / Luna + Sonnet 5.5 / Sol | ~$1.20 | ~$4.50 | **≈ $5–7** |
| All mid-tier (Sonnet 5.5 / Sol for everything) | ~$25 | ~$4.50 | ≈ $30 |
| Cheap checks + Opus 5.5 rewrites | ~$1.20 | ~$8 | ≈ $10 |
| Cheap checks + GPT-6 Astra rewrites | ~$1.20 | ~$19 | ≈ $20 |

Rewrites cost the most because output tokens are 5× more expensive than input tokens. Ways to keep the cost down:
- Only send paragraphs that changed (paragraph hashing).
- Look up the local cache first.
- Run spell-check locally.
- Never send a paragraph again while it's unchanged.

---

## 3. Stage 2: SaaS (once the personal version works)

Changes:
- **Swap the client backend.** A new `GatewayProvider` implements the same `LlmProvider` trait. It calls *your* server over WebSocket instead of calling Claude or OpenAI directly. Users never see an API key.
- **New `server/` folder (Rust):**
  - Axum gateway (REST + WebSocket, rustls)
  - Postgres (`sqlx`): users, devices, refresh_tokens, dictionary_entries, settings, subscriptions, usage_daily
  - Redis: rate limits, cache
  - The same `crates/llm` adapters, now running on the server
- **Auth:** email + password (argon2) or OAuth sign-in, JWT access tokens + rotating refresh tokens.
- **Billing:** Stripe Checkout + webhooks, using the `async-stripe` crate.
- **Metering:** count tokens per user per day. The plan's quota is enforced at the gateway.
- **Pricing model (example):**
  - **Free:** local spell-check + 50 AI checks per day.
  - **Pro, about $12/month:** unlimited checks + 50 rewrites per day.

  At the budget tier a typical active user costs about $1–3 a month in API fees, which leaves a healthy margin. Hosting at the start is one small VPS plus managed Postgres, about $20–40 a month.
- **Before launch:**
  - Privacy policy.
  - Opt-in consent before any text is stored.
  - Review each provider's data-retention and usage terms.
  - Notarize the macOS app (it must ship outside the Mac App Store, because the accessibility APIs need it).
- **Then add platforms:** Windows (UI Automation), the browser extension (WASM core), Android IME, iOS keyboard. All of them reuse the same Rust crates.

---

## Verification
- `cargo test --workspace`: core diff/rebase (including fuzz tests with `proptest`), PII redaction, UTF-16 offset conversion, JSON schema parsing for both providers.
- CLI harness: run the same fixed text sample through both providers. Confirm valid suggestions come back and the cost logged in SQLite matches each provider's usage dashboard.
- Manual macOS run: type in TextEdit, Notes, Mail and Slack. Check that underlines line up with the text, that applying keeps the caret in place, that secure fields are ignored, and that the monthly cap blocks calls once reached.
- Latency check: spell-check under 20 ms, LLM checks around 0.5–1.5 s after the debounce (fine for background suggestions), first rewrite token under 1 s.
