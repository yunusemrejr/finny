# Finny — Local Finance Assistant (Native Linux Desktop)

A compact, **local-first** personal finance assistant for Ubuntu / Linux.
**Native desktop GUI** built in Rust with [eframe/egui](https://github.com/emilk/egui)
— one binary, one SQLite database, one `run.sh` entry point. **No web server, no
browser, no cloud, no LLM, no telemetry.**

It looks like a 90s / early-2000s desktop app on the outside, and is built to be
fast, robust, and modern on the inside.

```bash
git clone https://github.com/yunusemrejr/finny.git
cd finny
./run.sh
```

First run resolves dependencies and builds the release binary, then opens a
native window. Chat history persists to `./data/finny.db`. No accounts, no
internet required for the core experience.

---

## Features

- **Conversational chat** for finance & economics — rates, inflation, GDP, yields,
  FX, calculators, definitions, trends, and forecasts. Finny greets you, answers
  small talk, and explains what it can do; finance answers read like a normal
  reply, not a raw report. Under the hood this is **fast local machine learning**
  (a Naive-Bayes intent/chitchat classifier + fuzzy, typo-tolerant slot matching)
  — **not** a language model. Scrollable log with markdown rendering, Ctrl/Cmd+Enter
  to send, Enter for newline, auto-scroll, per-turn persistence.
- **Sessions** — create, switch, archive; sidebar list; history reload.
- **Currency converter** — 10 currencies (USD EUR GBP JPY CHF CAD AUD CNY INR TRY),
  from/to, swap. Uses **live reference rates** (frankfurter.app / ECB, no API key)
  when the network is on, with a built-in approximate matrix as fallback.
- **Trend charts** — metric × region → ASCII chart from the deterministic engine.
- **Live network (on by default)** — automatic retrieval from a fixed set of
  keyless public APIs (World Bank, BLS, frankfurter.app) and central-bank /
  statistics-agency pages. Fast JSON APIs are tried first, slow page scrapes last.
  HTTPS only, SSRF-guarded, with full provenance. Disable via the toggle or
  `FINNY_NETWORK=0`.
- **Clear all data** — one button (with a confirm step) wipes every chat and
  cached source, leaving a fresh session. The database file is never deleted.
- **Responsive layout** — the sidebar collapses into a top navigation strip on
  narrow windows, like a browser going mobile.
- **Settings** — text-scale slider, export chat history to Markdown.
- **Status bar** — session, network state, evidence count, live clock, version.

## Query classes

| Class | Example |
|---|---|
| Direct lookup | `latest US CPI` |
| Comparison | `compare policy rate across US UK Japan` |
| Calculation | `calculate bond duration D=7 and 25bp` |
| Causal scenario | `how could yen appreciation affect carry trades` |
| Definition | `define duration` |
| Trend / Forecast | `US CPI over the last 5 years` · `forecast US CPI next year` |
| Data-availability challenge | `exact volume of stop-loss orders at 152.50` → firm refusal |

## Calculators (deterministic)

Bond duration, FX %Δ, CAGR, mortgage/loan, amortization schedule, present value,
inflation adjustment, P/E, debt-to-equity, current ratio, ROE, dividend yield,
Black-Scholes, Sharpe ratio, portfolio variance, break-even, WACC, compound
interest, currency conversion.

---

## Requirements

- **Linux** (developed on Ubuntu; other distros supported — `run.sh` detects
  Debian/Ubuntu, Fedora, and Arch family package managers).
- **Rust** stable toolchain (`cargo`, `rustc`). `run.sh` installs it via rustup
  if missing (prompts first).
- **Runtime graphics libs** for the native window: OpenGL (mesa `libgl`),
  `libxkbcommon`, and X11 or Wayland client libs. `run.sh` checks for these and
  offers to install any that are missing.
- A **C compiler** (`gcc`/`cc`) to build the bundled SQLite and `ring`.

`run.sh` resolves all of the above automatically and only ever asks before
running a privileged (`sudo`) install. It never touches your data.

## Verify

```bash
./verify.sh
```

Runs: release build, unit tests, a headless GUI self-test (via Xvfb + software GL
when available), and confirms the database is created.

## Environment variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `FINNY_DATA_DIR` | `./data` | SQLite location |
| `FINNY_NETWORK` | `true` | Enable live retrieval at startup (`0`/`false` to disable) |
| `FINNY_SELFTEST` | `false` | Render a few frames, print `SELFTEST_OK`, exit (headless check) |

## Architecture

```
finny/
  run.sh          # entry point: resolves deps, builds if needed, launches the window
  verify.sh       # E2E smoke test: build, tests, headless GUI, DB check
  Cargo.toml      # eframe/egui + image + finance-engine deps (no HTTP server)
  src/
    main.rs       # eframe App: window, panels, worker-thread chat, logo, styling, markdown
    engine.rs     # finance engine: classify -> plan -> [live fetch -> provenance] -> persist
    nlu.rs        # local ML: Naive-Bayes intent/chitchat + fuzzy (typo-tolerant) slots
    dialogue.rs   # conversational NLG: chitchat replies + natural answer wrapping
    intent.rs     # rule-based intent classifier + slot bindings
    planner.rs    # deterministic answer engine (calculators, charts, definitions)
    db.rs         # SQLite, per-call Connection (Send-safe), sessions/messages/sources
    retrieval.rs  # SSRF-guarded live retriever + keyless public APIs (WB/BLS/FX)
  assets/logo.png # Finny mascot logo (shown in header)
  data/           # SQLite lives here (FINNY_DATA_DIR) — not committed
```

The finance engine (`intent` / `planner` / `db` / `retrieval`) is UI-agnostic.
egui's `update()` runs on the main thread, so every chat send spawns a **worker
thread** that runs the engine and returns the reply over an `mpsc` channel — live
network fetches never freeze the UI (a "thinking" indicator shows meanwhile).

### Live data sources

The retrieval pipeline prefers fast, keyless JSON APIs and falls back gracefully:

1. **World Bank API** — inflation (CPI), GDP growth, unemployment, real interest
   rate, government debt, GDP per capita for 15+ countries/eurozone. No API key.
2. **BLS Public API** — US series: CPI-U (CUUR0000SA0), Fed Funds Effective
   (FEDFUNDS), unemployment (UNRATE). No API key.
3. **Central-bank / statistics-agency pages** — Federal Reserve (H15), ECB, BoE,
   ONS, Eurostat, TÜİK, BEA, BoJ (scraped, excerpted around keywords). Tried last.
4. **frankfurter.app** — live FX reference rates (ECB-sourced) for the converter.

Each phase runs only if the previous produced nothing, so fast local matches
avoid unnecessary network calls. All strategies are SSRF-guarded and every live
retrieval is persisted with full provenance (URL + SHA-256 + timestamp).

## Design principles

1. **Don't fabricate** — hidden order volumes / private positions → firm refusal.
2. **Show your work** — every formula exposes inputs, computation, assumptions.
3. **Local ownership** — no cloud, no telemetry, no silent uploads.
4. **Native, not browser** — a real desktop window, not a localhost web page.
5. **Auditability** — every live retrieval is persisted with full provenance.

## Tests

```bash
cargo test --release    # engine unit tests (SSRF guards, HTML→text, intent, JSON parsing)
./verify.sh             # full E2E: build + tests + headless GUI + DB init
```

## License

Not yet specified. All rights reserved until the author chooses a license.
