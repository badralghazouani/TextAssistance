# TextAssistance

A Grammarly-style writing assistant with a Rust core. It checks your text for
spelling, grammar and style problems and rewrites it on request, using
**Claude** or **OpenAI**, switchable in settings.

The plan ([docs/PLAN.md](docs/PLAN.md)):

1. **Now: personal use on macOS.** No server. The app calls the provider
   directly with your own API key, kept in the macOS Keychain.
2. **Later: SaaS.** A Rust gateway holds the keys and handles accounts,
   billing and quotas. Clients swap one `LlmProvider` implementation to use it.

## Status

| Milestone | State |
|---|---|
| Core engine: paragraph diffing, UTF-16 spans, rebasing, debounce | ✅ |
| Privacy: emails, phones, cards (Luhn), IBANs redacted before sending | ✅ |
| Claude + OpenAI adapters, price table, usage log, monthly cap, cache | ✅ |
| `writer` CLI (test harness for the pipeline) | ✅ |
| macOS menu-bar app: shortcut → suggestions popup → Replace | ✅ |
| Underlines while you type (no shortcut needed) | next |
| Offline spell-check | next |
| SaaS gateway, Windows, browser extension, mobile keyboards | later |

## Workspace

```
crates/protocol   wire types shared by clients and the future server
crates/core       paragraphs, diffs, span rebasing, debounce, quote locating
crates/privacy    PII redaction and mapping answers back to the original
crates/llm        LlmProvider trait, Claude and OpenAI adapters, pricing
crates/store      config file, SQLite usage log and result cache
crates/engine     check/rewrite pipeline and API keys, shared by both apps
apps/cli          the `writer` command
apps/desktop      the menu-bar app (Tauri): shortcut, popup, replace
```

## The menu-bar app

TextAssistance runs in the menu bar. While writing anywhere (Teams, Chrome,
Brave, Mail, Slack, Notes…):

1. Click in the text (or select part of it) and press **⌃⌥G** (Control+Option+G).
   Or select text, right-click, and choose **Services → Check with
   TextAssistance**.
2. A popup shows the corrections in place. Click any one to skip it, or try
   **Clearer / Shorter / Formal / Friendly** for a rewrite you can edit.
3. Press **Enter** (or click **Replace**) and your text is swapped for the
   corrected version. **Esc** closes without changing anything.

It reads and writes your text through the clipboard (copy, select-all,
paste) and puts your clipboard back afterwards, so it works in any app.

### Install

Download **TextAssistance-macOS** from the latest successful
[CI run](../../actions/workflows/ci.yml) (or a [release](../../releases)),
unzip it, open `TextAssistance.dmg` and drag the app to Applications.

The app isn't signed with an Apple developer certificate yet, so macOS blocks
it the first time. Run this once, then open it normally:

```sh
xattr -dr com.apple.quarantine /Applications/TextAssistance.app
```

Or build it yourself (needs Rust and Node.js):

```sh
cd apps/desktop
npx @tauri-apps/cli@2 build --bundles app
open ../../target/release/bundle/macos/TextAssistance.app
```

### First run

1. **Accessibility:** macOS asks to let TextAssistance control your computer.
   Allow it in System Settings → Privacy & Security → Accessibility. It is
   needed to send copy and paste to the app you're writing in.
2. **API key:** the first time you press ⌃⌥G the popup asks for your Claude or
   OpenAI API key and stores it in the Keychain. (Keys saved with
   `writer key set` work too.)
3. Optional: menu bar icon → **Start at login**.
4. If **Check with TextAssistance** doesn't show under right-click →
   Services, refresh macOS's list of services, then reopen the app you're in:
   `/System/Library/CoreServices/pbs -update`. It can also be switched on, or
   given its own shortcut, in System Settings → Keyboard → Keyboard Shortcuts
   → Services → Text.

The menu bar icon also switches between Claude and ChatGPT, shows this month's
spend, and opens the settings file (change `hotkey` there, then restart).

## The command-line tool: set up on macOS

```sh
xcode-select --install          # C compiler for bundled SQLite and TLS crypto
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust
cargo test --workspace          # runs offline, no API key needed
cargo build --release
./target/release/writer key set claude    # stores your key in the Keychain
cp target/release/writer /usr/local/bin/  # optional: put it on your PATH
```

## Try it

```sh
cargo build --release

# Key: environment variable, or the macOS Keychain
export ANTHROPIC_API_KEY=...          # or OPENAI_API_KEY=...
./target/release/writer key set claude   # macOS: saves to Keychain instead

echo "Me and him goes to the store yesterday." | ./target/release/writer check
./target/release/writer check notes.txt --apply > fixed.txt
./target/release/writer rewrite email.txt --mode formal --app email
./target/release/writer check draft.txt --provider openai

./target/release/writer usage                 # this month's spend per model
./target/release/writer config set monthly_cap_usd 5
./target/release/writer config set provider openai
```

Suggestions and rewrites go to stdout; cost and monthly spend go to stderr,
so output can be piped.

## Models and cost

| | Claude | OpenAI |
|---|---|---|
| Checks (default) | `claude-haiku-5-5` · $0.10 / $0.50 | `gpt-6-luna` · $0.10 / $0.50 |
| Rewrites (default) | `claude-sonnet-5-5` · $2 / $10 | `gpt-6.1-sol` · $2 / $10 |
| Premium rewrites | `claude-opus-5-5` · $4 / $20 | `gpt-6-astra` · $10 / $50 |

USD per million input / output tokens, as published on 2026-10-22
(`crates/llm/src/pricing.rs`). Heavy personal use (about 400 checks and 20
rewrites a day) comes to roughly **$5–7 a month** with the defaults.

Each paragraph is sent once: unchanged text is served from the local cache,
and the monthly cap (default $10) stops API calls once reached.

## How a check works

1. Text is split into paragraphs; only new or changed ones are checked.
2. Emails, phone numbers, card numbers and IBANs become placeholders such as
   `⟦EMAIL_1⟧`. The originals never leave the machine.
3. The model returns JSON (structured output) with the exact text it is
   quoting, not character offsets, because models count characters badly.
4. The quote is located in the paragraph, mapped back around placeholders, and
   turned into a UTF-16 span, the unit macOS and the other platforms use.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

The adapters are tested against recorded response shapes and a local mock
server, so the test suite needs no API key and costs nothing.
