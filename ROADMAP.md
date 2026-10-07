# chatterg: Hardening, Flexibility, Reporting and a General-Purpose Bot

*A deterministic (no AI) bot-to-bot questionnaire client. This document lists proposed improvements, how to build each, and what a general-purpose version would take.*

**How to read the estimates.** Effort is in *dev-days*: focused working days for one developer who knows this codebase, including tests. They are rough ranges, not commitments. Anything marked **(external)** depends on a third party's behaviour and can overrun.

---

## 1. Where the project stands today

### 1.1 What exists

| Area | Current behaviour |
|---|---|
| Input | `.txt` (one question per line) and `.yaml`/`.yml` (bare strings and/or full question objects, mixable). |
| Question model | `id`, `question`, `required`, `type` (`string`, `text`, `integer`, `list`, `enum`), `values`, `max_followups`, `on_failure`, `reject_if_contains`, `followup`. |
| Engine | Pure state machine. Position and a sent-message counter live inside `Conversation`, so they persist with the answers. |
| Persistence | SQLite, one row (`id = 1`) holding the whole conversation as JSON. A rerun with the same `--store` resumes. |
| Transport | `Transport` trait (`discover`, `send`). A2A JSON-RPC (`message/send`) implemented and tested; a mock transport for tests; a minimal `http.rs`. |
| Resilience | Cooldown and resend on 429 / 5xx / timeout / network failure / rate-limit-looking replies. `--cooldown`, `--max-waits`, `--delay`, `--timeout`. |
| Errors | Typed errors per layer (`DomainError`, `StorageError`, `TransportError`, `ApplicationError`). |
| Tests | ~100 tests: unit, Mockito-based transport tests, CLI end-to-end tests with a mock agent. |

### 1.2 Known limitations (the starting point for the roadmap)

> **Status update:** milestone **M1** is done (items 1, 2, 4 and the schema-version part of 3 below, plus exit codes and CI). Those entries are struck through. Everything else is still open.

1. ~~**No timestamps or latency.** Attempts record request, response and validation only. Reports cannot show *when* or *how long*.~~ *Done (R1).*
2. ~~**Resume trusts the saved position.** Editing or reordering the questions file mid-run silently misaligns answers.~~ *Done (H1).*
3. **One conversation per database.** *(Run metadata and a schema version with migrations are done (R1); multiple runs per database is F3.)*
4. ~~**No locking.** Two processes on one `.db` can interleave.~~ *Done (H2).*
5. **Fixed waits.** The cooldown is a constant. `Retry-After` and `RateLimit-*` headers are ignored.
6. **Stateless messages.** No `contextId`/`taskId` is sent, so the agent cannot connect one question to the next.
7. **A2A tasks that are not finished** (`working`, `input-required`) are treated as malformed.
8. **Output is console-only** (latest answer per question). There is no report.
9. **`on_failure: unknown` ends the whole run**, the same as `abort`. This surprises people.
10. **Ctrl-C is not handled.** State is safe (saved after every answer) but there is no summary and no graceful stop.

---

## 2. Roadmap at a glance

| ID | Item | Value | Effort (days) | Depends on |
|---|---|---|---|---|
| **R1** ✅ | Timestamps, latency, schema version | High | 1 | none |
| **R2** | `report` command (Markdown, HTML, CSV, JSON) | High | 3 to 4 | R1 |
| **H1** ✅ | Questions-file fingerprint on resume | High | 0.5 | R1 |
| **H2** ✅ | Database locking, WAL, busy timeout | High | 0.5 | none |
| **H3** | Graceful Ctrl-C and run summary | Medium | 1 | none |
| **H4** ✅ | Exit codes | Medium | 0.5 | none |
| **B1** | `Retry-After` / `RateLimit-*` support | High | 1 to 1.5 | none |
| **B2** | Client-side rate limiter and backoff | High | 1 | B1 |
| **F1** | Sections in the questions file | Medium | 1 | R2 for use |
| **F2** | Run control (`--only`, `--from`, `--limit`, `--dry-run`) | Medium | 1 | none |
| **C1** | Structured logging (`tracing`), `--verbose`, log file | Medium | 1 | none |
| **C2** | `status`, `validate`, `probe` commands | Medium | 2.5 | R1 |
| **D1** | Config file and environment variables | Medium | 1.5 | none |
| **D2** | Authentication (bearer token, API key header) | Medium | 1 | D1 |
| **E1** | A2A task states, `contextId`, polling | Medium | 3 to 4 | none |
| **F3** | Multiple named runs per database | Medium | 3 | R1 |
| **F4** | Templating and per-section settings | Low | 2 | F1 |
| **F5** | CSV and Markdown input | Low | 1 | none |
| **F6** | Stronger answer checks (regex, length, refusal) | Medium | 1.5 | none |
| **R3** | `compare` two runs | Low | 1.5 | R2, F3 |
| **F7** | One questionnaire against several agents | Low | 3 | F3 |
| **I1** ✅ | CI, `cargo audit`, MSRV pin | Medium | 1 | none |

**Core track (R1, R2, H1 to H4, B1, B2, F1, F2, C1, I1): about 12 to 14 days.** Everything in the table: about 30 to 35 days.

---

## 3. Reporting

### R1. Timestamps, latency and schema version  ✅ *done*

> **As built:** every attempt stores `sent_at`, `received_at` and `latency_ms` (UTC; latency is never negative). The conversation stores `started_at`, `finished_at`, the agent (name, target, endpoint, protocol), the chatterg version and a cooldown log (`at`, `reason`, `seconds`). `schema_version` is 2; `storage::migrate` upgrades version-1 data on load and refuses data from a newer version. Data older than version 1 (before positions were stored) is still rejected rather than guessed. Time is read through an injectable clock, so tests are deterministic.

**Problem.** Attempts record request, response and validation only. Without times, a report cannot show duration, cooldown impact or pace, and every later feature needs the data.

**Proposal.**
- Add to each attempt: `sent_at`, `received_at` (UTC, RFC 3339), `latency_ms`.
- Add to the conversation: `started_at`, `finished_at`, `cooldowns` (a list of `{at, reason, seconds}`), `agent` (`name`, `url`, `protocol`), `chatterg_version`, `questionnaire_fingerprint`.
- Add `schema_version: u32` to the stored JSON and a `migrate(old) -> new` function chain.

**Implementation notes.**
- Take timestamps in `application.rs` around `transport.send`, and cooldown events inside `with_cooldown`. Keep the engine pure by passing the timing in, not reading the clock in domain code.
- Use `chrono` or `time` for dates.
- Previously stored data lacks `schema_version`. Treat a missing version as version 1 and migrate. (The current loader rejects old data on purpose; migration replaces that.)

**Acceptance tests.**
- A run against the mock agent stores increasing timestamps and non-negative latencies.
- A v1 database migrates to v2 and resumes correctly.
- A round trip through JSON preserves every field.

### R2. `report` command

**Problem.** Console output shows only ids and the latest answer. You want something you can hand to a colleague.

**Proposal.** New subcommand:

```bash
chatterg report --store run.db --format md  --out report.md
chatterg report --store run.db --format html --out report.html
chatterg report --store run.db --format csv  --out answers.csv
chatterg report --store run.db --format json --out run.json
```

*Report contents (Markdown/HTML):*
1. **Header:** agent name and URL, protocol, start/finish time, duration, chatterg version, questions file name and fingerprint.
2. **Summary table:** total questions, answered, rejected/evasive, skipped, retries used, cooldown count and total pause time, average and p95 latency.
3. **Answers by section** (uses F1): question, final answer, number of attempts.
4. **Needs attention:** questions that were rejected, evasive (matched `reject_if_contains`), skipped or empty, each with the reason.
5. **Cooldown log:** when and why the agent was unavailable.
6. **Appendix: full transcript** with every attempt, timestamp and validation result.

**Implementation notes.**
- A pure function `render(&Conversation, &Questionnaire, Format) -> String` in a new `output/report.rs`, with one small renderer per format. No I/O inside, which makes golden-file tests easy.
- HTML: a single self-contained file (inline CSS, no external requests), responsive, light/dark aware, with an escaped answer body. **Always HTML-escape answers**; they are untrusted bot output.
- CSV: one row per question, quoted correctly, with a header row; columns `id, section, question, answer, status, attempts, latency_ms`.
- The questionnaire is needed to recover question text and sections. This ties in with H1: refuse to report with a different questions file than the run used.

**Acceptance tests.** Golden-file tests per format against a fixed fixture conversation; an answer containing `<script>`, quotes, commas and newlines renders safely in HTML and CSV.

### R3. `compare` command

Diff two runs of the same questionnaire (for example the same bot on two days, or two bots): per question show "same / changed / newly answered / newly missing", plus a similarity score using a deterministic text measure (token Jaccard or normalized Levenshtein). Output as Markdown. **Effort 1.5 days**, after R2 and F3. This is deterministic string comparison, not semantic judgement. Say so in the output so nobody reads "changed" as "different meaning".

---

## 4. Production hardening

### H1. Detect a changed questions file  ✅ *done*

> **As built:** a SHA-256 fingerprint plus a snapshot of each question's id, text, type and allowed values is stored at the start of the run. Retry and dodge-word settings are deliberately not part of it. On resume, any change is refused with a summary (`1 added (q003)`, `order changed`, ...). `--force-resume` continues only if every question already asked (and the one in progress) is unchanged; reordering, removing or editing those is refused even with the flag. `--restart` copies the old run to `<store>.<timestamp>.bak` (`VACUUM INTO`) and starts over. Runs saved before fingerprints existed adopt the current questions once.

**Problem.** The saved position is trusted. If you insert a question at position 5 after answering 10, the engine resumes at 10 against shifted questions. `Engine::resume` only checks that stored answer ids exist.

**Proposal.** Store `questionnaire_fingerprint` (SHA-256 over a canonical form: ordered `(id, question text, type, values)`). On resume:
- identical: continue;
- different: stop with a clear error showing a short diff summary (added, removed, reordered, edited), and offer `--force-resume` (only valid if every already-answered id still exists and kept its order) or `--restart` (archive the old run, start fresh).

**Notes.** Plain-text ids are positional (`q001`...), so inserting a line renumbers everything. Document this, and make `validate` (C2) warn about it. Consider a content-derived id option for plain text later (a hash of the question text), so reordering does not break resume.

**Acceptance tests.** Edited, reordered, appended and unchanged files each produce the intended outcome; `--force-resume` refuses a reorder of answered questions.

### H2. Database locking and robustness  ✅ *done*

> **As built:** an exclusive advisory lock on `<store>.lock` (via the `fs2` crate) is taken when the store is opened and released when it is dropped or the process dies, so a crash never leaves a stale lock. The database uses WAL with `synchronous=NORMAL` and a 5 s busy timeout. Saves are a single atomic upsert. Not done: a periodic `integrity_check` (planned for `status`, C2).

**Problem.** Two processes using one database can interleave writes. A killed process can leave a journal.

**Proposal.**
- Open SQLite with `PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA synchronous=NORMAL`.
- Take an exclusive advisory lock on `<store>.lock` (e.g. the `fs2` or `fd-lock` crate) for the lifetime of a run. A second run fails fast: "another chatterg is using this store (pid N)".
- Wrap the save in an explicit transaction. Add a periodic `PRAGMA integrity_check` in `validate`/`status`.

**Acceptance tests.** Spawn two runs on one store: the second exits non-zero without touching data. Kill -9 mid-run, then resume.

### H3. Graceful Ctrl-C

**Problem.** Ctrl-C works only because state is saved after each answer. There is no confirmation and no summary, and a cooldown sleep ignores the signal until it ends.

**Proposal.** Use `tokio::signal::ctrl_c` (the feature is already enabled) with `tokio::select!` around sleeps and sends. On the first signal: finish or abandon the in-flight request, save, print a summary (`Stopped at 37/120. Resume with the same command.`) and exit with a dedicated code. A second signal exits immediately.

**Acceptance tests.** Send SIGINT to the binary during a mock cooldown and confirm it exits quickly with state saved.

### H4. Exit codes  ✅ *done*

> **As built:** 0, 1, 2 and 3 as listed below; they are shown in `--help`. The 130 (interrupted) code arrives with H3 (graceful Ctrl-C), which is not done yet.

| Code | Meaning |
|---|---|
| 0 | Completed (all questions processed) |
| 1 | Error (bad arguments, file, network, protocol) |
| 2 | Run ended early by policy (`abort`/`unknown`) |
| 3 | Gave up after `--max-waits` cooldowns (agent unavailable) |
| 130 | Interrupted by the user |

This lets cron and CI react correctly. Document it in `--help`.

### I1. CI and supply chain  ✅ *done*

> **As built:** `.github/workflows/ci.yml` (fmt, clippy `-D warnings`, tests on stable and on the declared minimum Rust, 1.88), `.github/workflows/audit.yml` (`rustsec/audit-check`, weekly and on lockfile changes), `.github/dependabot.yml`, and `rust-version = "1.88"` in `Cargo.toml`. Not done: `cargo deny` and release binaries. The workflows have not been run on GitHub yet.

- GitHub Actions on push/PR: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --all-targets`, on stable and the pinned MSRV.
- `cargo audit` and `cargo deny` (licences, banned crates) on a schedule.
- Commit `Cargo.lock`, pin `rust-version` in `Cargo.toml`, add Dependabot or Renovate.
- Release workflow producing static binaries for Linux, macOS and Windows.
- Effort: **1 day**.

---

## 5. Rate limiting and politeness

The agent you tested limited by public IP, with the message "Rate limit exceeded for this caller". The right response is to behave well, not to disguise yourself. Do not build identity or IP rotation. The following make chatterg a considerate client.

### B1. Honour server hints

On HTTP 429/503, read `Retry-After` (seconds or an HTTP date) and common headers (`RateLimit-Reset`, `X-RateLimit-Reset`, `X-RateLimit-Remaining`). Use `max(cooldown, hint)` for the wait, with a cap (`--max-cooldown`, default 1 hour). Keep the fixed cooldown as the fallback for servers that send no hint (as this one appears not to; it returned a JSON-RPC error).

**Implementation.** Extend `TransportError::Http` with `retry_after: Option<Duration>`; have `with_cooldown` use it. **Effort 1 to 1.5 days.**

### B2. Client-side limiter and adaptive backoff

- `--rate "30/hour"` or `--min-interval 60s`: a token bucket in the application layer, applied before every send, so you stay under the limit instead of tripping it.
- **Adaptive:** after each cooldown, multiply the inter-question delay (say x1.5, capped), and decay it slowly after a streak of successes. Persist the learned interval in the run metadata so a restart does not forget it.
- **Jitter** (+/- 10%) so waits are not perfectly periodic.
- Treat rejected retries as costly: some servers count them. Prefer longer waits over frequent retries.

**Effort 1 day.**

### B3. Operational guidance (documentation)

A short "Being a good client" section in the README: identify the tool honestly with a `User-Agent: chatterg/<version> (contact: ...)`, run slowly, ask the operator for a quota or key, and run in batches (progress is saved).

---

## 6. Observability and operator tooling

### C1. Logging

Use `tracing` (already a dependency). Levels: `error`/`warn` always, `info` per question, `debug` for request/response bodies. Flags: `-v/-vv`, `--log-file`, `--log-format json`. **Never log secrets** (tokens). Keep the human progress lines (`[12/120] q012`) on stderr. **Effort 1 day.**

### C2. Utility subcommands

| Command | Purpose | Notes |
|---|---|---|
| `chatterg status --store run.db` | Position, answered/rejected counts, average latency, ETA from the current pace, last cooldown | Read-only; safe during a run (WAL) |
| `chatterg validate questions.txt` | Lint: duplicates, empty, very long, ids that depend on position, suspicious reject phrases | Exit non-zero on errors |
| `chatterg probe <url>` | Fetch the agent card, show protocol and interfaces, send one harmless message, print the status and rate-limit headers | Replaces the manual `curl` commands; sends exactly one request |

**Effort 2.5 days total.**

---

## 7. Configuration and authentication

### D1. Config file and environment

Flags get long. Support `chatterg.toml` (and `CHATTERG_*` environment variables), with precedence *flags > env > file > defaults*:

```toml
[agent]
url = "https://example.com/agent"
timeout_secs = 120

[run]
store = "runs/zeolite.db"
questions = "questions/zeolite_membranes.txt"

[pacing]
delay_secs = 60
cooldown_secs = 600
max_waits = 30
rate = "30/hour"

[defaults]            # applied to plain-text questions
retries = 2
on_failure = "continue"
reject_phrases = ["meeting", "schedule a call", "calendly"]
```

Use `serde` + `toml` and a `clap` override layer. **Effort 1.5 days.**

### D2. Authentication

Add `[auth]` to config: `type = "bearer" | "header" | "basic"`, with the **secret read from an environment variable or a file, never from the config itself**. Redact secrets in logs and in reports. If the agent card declares `securitySchemes`, `probe` should report what is required. **Effort 1 day.**

---

## 8. Protocol completeness (A2A)

### E1. Tasks, context and streaming

Today only a reply that carries a message (`result.message`, a flat task with `status.message`, or `result.task.status.message`) is understood. Proposed handling:

- **Context.** Capture `contextId` (and `taskId` when relevant) from the first response and send it on later messages (config `conversation = "stateful"`), so an agent that tracks context can answer follow-ups. Keep `stateless` as the default because it is what works today.
- **Task states.** `submitted`/`working`: poll `tasks/get` with backoff until `completed`, `failed`, `canceled`, `rejected`, or a timeout. `input-required`: treat as the agent asking something. Record it and, by policy, answer, skip or stop. `failed`/`rejected`: a protocol error with the task's message.
- **Streaming.** Optional `message/stream` (server-sent events) for agents that stream; assemble the final text. Do last; lowest value.
- **Multi-part replies.** Currently text parts are joined with newlines. Keep non-text parts (files, data) as references in the stored attempt rather than dropping them.
- **Agent card extras.** Honour `capabilities`, `defaultInputModes`, and warn when the card advertises no JSON-RPC binding.

**Effort 3 to 4 days (external:** agents differ in how strictly they follow the spec).

---

## 9. Flexibility: questions and runs

### F1. Sections

Plain text: a line starting with `## ` begins a section (`#` stays a comment). YAML: a `section:` key on an entry, or a `sections:` list containing `questions`. Store `section` on each question and surface it in reports and `--only`. **Effort 1 day.**

```text
## Zeolite basics
What is a zeolite, and why are zeolites useful for membrane separations?
How does the pore size of a zeolite affect molecular separation?

## Nxtbrane
What is Nxtbrane?
```

### F2. Run control

- `--only q001-q020,q050` or `--only "section:Nxtbrane"`
- `--from q040`, `--limit 10`
- `--dry-run`: parse, validate, print what would be sent (ids, text, settings) and exit without any network traffic
- `--shuffle --seed N`: a reproducible order, to test whether order affects answers

Partial runs interact with resume (position semantics). Define position over the *selected* list and include the selection in the fingerprint (H1). **Effort 1 day.**

### F3. Multiple named runs per database

**Problem.** The `conversations` table holds exactly one row (`id = 1`). Repeat runs need separate files.

**Proposed schema (migration from v2):**

```sql
CREATE TABLE runs (
  id            INTEGER PRIMARY KEY,
  name          TEXT NOT NULL UNIQUE,
  agent_url     TEXT NOT NULL,
  fingerprint   TEXT NOT NULL,
  started_at    TEXT NOT NULL,
  finished_at   TEXT,
  state         TEXT NOT NULL,          -- running | complete | aborted | gave_up
  schema_version INTEGER NOT NULL
);

CREATE TABLE attempts (
  run_id        INTEGER NOT NULL REFERENCES runs(id),
  question_id   TEXT NOT NULL,
  attempt_no    INTEGER NOT NULL,
  message_id    TEXT NOT NULL,
  request       TEXT NOT NULL,
  response      TEXT,
  validation    TEXT NOT NULL,          -- accepted | rejected | evasive | ...
  sent_at       TEXT NOT NULL,
  received_at   TEXT,
  latency_ms    INTEGER,
  PRIMARY KEY (run_id, question_id, attempt_no)
);

CREATE TABLE events (                   -- cooldowns, interrupts, errors
  run_id INTEGER NOT NULL REFERENCES runs(id),
  at TEXT NOT NULL, kind TEXT NOT NULL, detail TEXT
);
```

Row-per-attempt (instead of one JSON blob) makes reporting, status and partial inspection straightforward SQL, and removes the "rewrite the whole blob on every save" cost. CLI: `--run zeolite-2026-10-05`; `chatterg runs list`. **Effort 3 days**, including the migration and rewriting `Store`.

### F4. Templating and per-section settings

Variables from the config (`{company}`, `{product}`) substituted into question text; per-section defaults for `retries`, `on_failure`, `reject_phrases`. Optional simple conditionals (`ask_if: q003 contains "yes"`) kept deliberately small and rule-based. **Effort 2 days.** Skip until a real need shows up; conditionals grow quickly into a scripting language (see section 11).

### F5. More input formats

CSV (`id,section,question,type,retries,...`) and Markdown (headings become sections, list items become questions). **Effort 1 day.** Useful for non-programmers who keep questions in a spreadsheet.

### F6. Stronger answer checks (still deterministic)

- `min_length`, `max_length`, `must_contain`, `must_match: "<regex>"`
- A built-in refusal/deflection list (not just meetings): "I can't share", "as an AI", "contact sales"
- Detect **identical repeated replies** across different questions (a bot stuck on a canned message) and flag them rather than recording 40 copies of the same text
- Count the evasion cause separately in the report (matched phrase X)

Cap regex execution (use the `regex` crate, which is linear-time) to avoid pathological patterns. **Effort 1.5 days.**

### F7. One questionnaire, several agents

Run the same questions against a list of agent URLs sequentially (each with its own limiter, because limits are per target), producing one comparison report. **Effort 3 days**, after F3 and R3.

---

## 10. Suggested sequencing

| Milestone | Contents | Effort | Outcome |
|---|---|---|---|
| **M1: Trustworthy results** ✅ *done* | R1, H1, H2, H4, I1 | ~3.5 days | Safe resume, no data confusion, CI in place |
| **M2: Report** | R2, F1 | ~4.5 days | A shareable report grouped by section |
| **M3: Polite and robust** | B1, B2, H3, C1 | ~4.5 days | Respects server limits, clean stop, logs |
| **M4: Operator comfort** | F2, C2, D1, D2 | ~6 days | Dry-run, status, probe, config, auth |
| **M5: Depth** | F3, R3, F6, E1 | ~10 days | Multi-run, comparison, stricter checks, richer A2A |
| **Optional** | F4, F5, F7 | ~6 days | Templating, CSV, multi-agent |

M1 and M2 alone (about 8 days) turn the current tool into something you can run and present with confidence.

---

## 11. Going general-purpose: talking to any bot

### 11.1 Is it possible?

**Yes, within a clear limit.** chatterg is already structured for it: the engine, validation and storage know nothing about A2A. They only see "send a text, get a text". All protocol knowledge is behind the `Transport` trait. A general-purpose version means adding more transports and a way to describe a target without writing Rust each time.

**The limit (no AI).** A deterministic tool can *send* anything and *capture* the reply as raw text. It cannot *understand* a free-form reply. What it can do is apply rules the operator wrote ahead of time:

| Possible without AI | Not possible without AI |
|---|---|
| Send a fixed list of questions | Decide what to ask next based on the meaning of a reply |
| Capture the full reply text | Judge whether an answer is *correct* or *good* |
| Match keywords, regexes, JSON paths, numbers | Extract a fact from an arbitrary rambling reply |
| Walk a **pre-scripted** menu or decision tree | Navigate an unfamiliar menu on its own |
| Detect "rate limit", "no", "contact sales" via phrase lists | Reliably detect evasion phrased in new ways |

So "get the required answers" really means: *the operator defines how to reach the bot, what to send, and how to pull each answer out of the reply (usually "the whole reply"), and chatterg executes that reliably.* Bots that answer free-form questions in plain text are the easy case (and what we do now). Menu- and button-driven bots need a script.

### 11.2 Target adapters (the work list)

| Adapter | How it talks | Fit | Effort (days) |
|---|---|---|---|
| **A2A** | JSON-RPC over HTTP | Done | 0 |
| **Generic HTTP/JSON** | POST/GET with a configurable URL, headers, body template and a JSON-pointer for the reply | Covers many custom bots and webhooks | 3 to 4 |
| **OpenAI-compatible chat** | `POST /v1/chat/completions`; the client keeps and resends message history | A very common shape; many bots and gateways copy it | 1.5 to 2 |
| **Command / stdin-stdout** | Spawn a process, write a line, read a line | Local bots, scripts, CLIs | 1.5 |
| **WebSocket** | Persistent socket, JSON or text frames | Many web chat backends | 3 |
| **SSE / streaming HTTP** | Accumulate a streamed reply | Streaming APIs | 2 |
| **MCP client** | Call a tool exposed by a Model Context Protocol server | Tool-style agents | 3 to 4 |
| **Chat platforms** (Slack, Telegram, Discord, Teams) | Official bot APIs: post a message, wait for the reply event | Only where *you own or are authorised on* the bot and workspace | 3 to 5 each **(external)** |
| **Email** | SMTP send, IMAP poll | Slow, asynchronous bots | 4 to 5 |
| **Browser automation** | Drive a web chat widget in headless Chromium | Last resort: fragile, often against the site's terms | 7 to 10 (not recommended) |

**Guardrail.** Only talk to bots you operate or have permission to test. Respect terms of service, authentication and rate limits. Do not build features whose purpose is to impersonate a person, evade a limit or bypass a login (identity or IP rotation, CAPTCHA solving, scraping chat widgets that forbid it). The platform adapters in particular require the platform's official, authorised bot access.

### 11.3 Architecture changes needed

**1. Target profiles.** Describe a target in a file instead of code:

```toml
# targets/acme-support.toml
name = "Acme support bot"
adapter = "http-json"
endpoint = "https://bot.acme.example/api/chat"
conversation = "stateful"          # stateless | stateful | history

[auth]
type = "bearer"
token_env = "ACME_TOKEN"           # secret comes from the environment

[request]
method = "POST"
body = '''{"session": "{{session_id}}", "text": "{{question}}"}'''
headers = { "Content-Type" = "application/json" }

[response]
text_path = "/reply/text"          # JSON pointer; or `regex`, or `whole_body = true`
done_path = "/reply/final"         # optional: wait until this is true

[limits]
rate = "20/hour"
timeout_secs = 60
```

**2. A richer transport contract.** Today: `send(text) -> text`. A general version needs, as options on that contract:
- **Session lifecycle:** `open()` (create a session, perform a greeting or consent step) and `close()`.
- **Conversation modes:** *stateless* (each message independent), *stateful* (a session or context id carried along), *history* (the client resends prior turns, as chat-completions APIs expect).
- **Reply collection:** a single response; several messages that arrive over time (collect until a *quiet period*, or until an end marker); or polling until done.

**3. Extraction rules per question** (deterministic):

```yaml
- id: price
  question: "What does the Pro plan cost?"
  extract:
    regex: '\$\s?(\d+(?:\.\d{2})?)'    # capture group 1 becomes the answer
    fallback: whole_reply              # otherwise keep the full text
  type: text
```

Supported extractors: whole reply, regex capture, JSON pointer, first number, enum match against a list of allowed values, line N. The full reply is always stored alongside the extracted value, so nothing is lost.

**4. Scripted flows** for bots that start with menus, buttons or a form:

```yaml
flow:
  - expect: "(?i)welcome|how can i help"     # wait for the greeting
  - send: "1"                                  # menu option
  - expect: "(?i)select a topic"
  - send: "Billing"
  - then: questions                            # now run the questionnaire
```

This is a small state machine: `send`, `expect` (regex or timeout), `choose` (pick a button by label), `then`. Deliberately limited: it replays a path the operator already knows. It does not explore. Keep it small, or it becomes a general scripting language.

**5. Per-target limits.** The rate limiter (B2), cooldown (B1) and timeouts must be configured per target, since each bot has different limits.

**6. A fake-bot test harness.** One scriptable fake server per adapter (HTTP, WebSocket, OpenAI-style, menu bot) so adapters are tested without real services. This is the main reason adapters are not "3 days" each in practice.

### 11.4 Effort

| Package | Contents | Effort (days) |
|---|---|---|
| **Foundation** | Target profiles, adapter registry, session lifecycle, conversation modes, per-target limits | 4 to 5 |
| **Extraction** | Extractors (regex, JSON pointer, number, enum), storing both raw and extracted | 3 |
| **First adapters** | Generic HTTP/JSON, OpenAI-compatible, command/stdin | 6 to 8 |
| **Async replies** | Quiet-period collection, polling, end markers | 2 to 3 |
| **Scripted flows** | `send` / `expect` / `choose` state machine plus tests | 5 to 7 |
| **Test harness** | Fake servers per adapter | 3 to 4 |
| **Docs and examples** | Target profile guide, sample profiles | 2 |
| **MVP total** (HTTP-JSON, OpenAI-style, command, A2A, extraction, no scripted flows) | | **about 15 to 20 days** |
| **Plus scripted flows and async** | | **+7 to 10 days** |
| **Plus WebSocket, SSE, MCP** | | **+8 to 11 days** |
| **Plus each chat platform (Slack etc.)** | | **+3 to 5 days each** |

**Realistic range:** a solid general-purpose tool for HTTP-style bots in **4 to 5 weeks**; broad coverage including websockets, MCP and a couple of chat platforms in **8 to 12 weeks**. Treat any adapter that depends on a third party's quirks or authentication as the likely source of overruns.

### 11.5 Main risks

- **Every bot is different.** The long tail of replies (multi-message, typing indicators, buttons, markdown cards, attachments) never ends. The profile format must stay expressive enough without becoming a programming language.
- **Statefulness.** Many bots only make sense with context. Getting session handling right per adapter is where most debugging time goes.
- **Terms of service and authorisation.** Platform bots and web widgets often forbid automated access. The adapter list is bounded by what you are permitted to use.
- **Brittle extraction.** Regexes on free-form text break when the bot's wording changes. Always store the raw reply and flag extraction failures instead of guessing.
- **No semantic checking.** The tool cannot say an answer is *right*. Reports must label results "captured", not "verified".

### 11.6 Suggested path to get there

1. Finish **M1 and M2** (trustworthy storage, reports) because every adapter benefits from them.
2. Introduce the **target profile + adapter registry** while keeping A2A as the first adapter. No behaviour change, only restructuring.
3. Add **Generic HTTP/JSON** and **OpenAI-compatible** adapters with the **extraction rules**. This covers most "ask a question, get an answer" bots.
4. Add **scripted flows** only once a concrete menu-driven bot needs them.
5. Add WebSocket / MCP / platform adapters one at a time, each driven by a real target you are authorised to test.

---

## 12. Appendix

### A. Proposed CLI surface

```text
chatterg run      <TARGET|URL> <QUESTIONS> [--run NAME] [--only ...] [--dry-run]
                  [--delay S] [--cooldown S] [--max-waits N] [--rate 30/hour]
                  [--config chatterg.toml] [--store PATH] [-v|-vv]
chatterg report   --store PATH [--run NAME] --format md|html|csv|json --out FILE
chatterg status   --store PATH [--run NAME]
chatterg validate <QUESTIONS>
chatterg probe    <URL|TARGET>
chatterg compare  --store PATH <RUN_A> <RUN_B>
chatterg runs     list|show|delete --store PATH
```

(The current form, `chatterg <URL> <QUESTIONS> --store PATH`, would stay as a shortcut for `run`.)

### B. Report skeleton (Markdown)

```markdown
# Questionnaire report: <agent name>

| | |
|---|---|
| Agent | https://example.com/agent (A2A) |
| Run | zeolite-2026-10-05 |
| Started / finished | 2026-10-05 09:12 / 2026-10-05 11:48 |
| Questions file | zeolite_membranes.txt (sha256 3fa9...) |

## Summary
| Total | Answered | Rejected | Skipped | Retries | Cooldowns | Avg latency |
|---|---|---|---|---|---|---|
| 120 | 108 | 9 | 3 | 21 | 4 (14 min) | 3.2 s |

## Zeolite basics
### q001: What is a zeolite, and why are zeolites useful ...?
*Accepted, 1 attempt, 2.9 s*
> <answer text>

## Needs attention
| Question | Status | Reason |
|---|---|---|
| q047 | Rejected after 3 attempts | matched "schedule a call" |

## Cooldown log
| Time | Reason | Waited |
|---|---|---|

## Appendix: full transcript
```

### C. Definition of done for each item

- Tests added first or alongside (unit + integration + CLI where relevant).
- `cargo fmt`, `cargo clippy -D warnings`, `cargo test` pass.
- No new `unwrap`/`expect` in non-test code; typed errors with the cause preserved.
- Documented in `--help` and the README; secrets never logged or reported.
- Stored-data changes ship with a migration and a test that loads the previous version.

### D. Things deliberately out of scope

- Concurrent requests to one target (they work against rate limits).
- Any form of IP, identity or fingerprint rotation, or other limit evasion.
- Judging answer quality or meaning (that would need a language model; the premise here is none).
