# chatterg 🤖💬

**chatterg is a polite robot that asks another robot a list of questions and writes down the answers.**

That's it. No magic. No AI inside chatterg. It just asks, waits, writes down what it hears, and moves to the next question.

---

## The story in 30 seconds

Imagine you have a list of questions on paper:

```
What is a zeolite?
What is a membrane?
How does water get through?
```

And there is another robot (a "bot") on the internet that knows the answers.

chatterg is like a little postman:

1. 📝 It reads your list of questions.
2. 🚪 It knocks on the other bot's door.
3. 🗣️ It asks question number 1 and waits.
4. ✍️ It writes the answer in a notebook (a small file on your computer).
5. ➡️ It asks question number 2... and so on, until the list is finished.

If the other bot says "Too many questions! Slow down!", chatterg **takes a nap for 2 minutes** and then asks again. If you turn chatterg off and on again, it opens its notebook, sees where it stopped, and **carries on from there**.

---

## What you need first

| Thing | Why |
|---|---|
| A computer with a terminal (the black window where you type commands) | That's where you tell chatterg what to do |
| **Rust 1.88 or newer** | chatterg is written in Rust. Get it at <https://rustup.rs> |
| The **web address (URL)** of the bot you want to ask | So chatterg knows which door to knock on |
| A **file of questions** | So chatterg knows what to ask |

To check Rust is installed, type this. You should see a version number:

```bash
rustc --version
```

---

## Step 1: Build it (do this once)

```bash
git clone <your-repo-url> chatterg
cd chatterg
cargo build --release
```

This makes the program. It lives here: `target/release/chatterg`

> 💡 `--release` makes it fast. You only need to build again if you change the code.

---

## Step 2: Make your list of questions

The easiest way: make a text file. **One question on each line.** That's all.

Save this as `my_questions.txt`:

```text
# Lines that start with a # are notes for humans. chatterg skips them.

What is a zeolite?
What is a membrane?

How does water get through?
```

Rules (very easy):

- One question per line.
- Empty lines are fine. chatterg skips them.
- Lines starting with `#` are comments. chatterg skips them.
- Things like `- `, `* ` or `1. ` at the start of a line, and quote marks around the question, are removed for you.
- A line starting with `##` is a **heading**. It starts a new group (a "section") for the questions below it. The report uses these as chapter titles. A line with just `##` ends the group.

```text
## Zeolite basics
What is a zeolite?
What is a membrane?

## Nxtbrane
What is Nxtbrane?
```

chatterg gives each question a name: `q001`, `q002`, `q003`... (that's how you'll find them in the answers).

### Already have a big list?

This repo comes with a ready-made list in `questions/zeolite_membranes.txt` (120 questions about zeolite membranes).

---

## Step 3: Run it!

```bash
./target/release/chatterg  https://the-bots-address.example/agent  my_questions.txt
```

Let's read that like a sentence:

| Part | What it means |
|---|---|
| `./target/release/chatterg` | "Hey chatterg, wake up!" |
| `https://the-bots-address.example/agent` | "Go and knock on THIS door." |
| `my_questions.txt` | "Ask THESE questions." |

While it works you will see little messages like this:

```
[1/3] q001
[2/3] q002
[3/3] q003
```

That means "I'm asking question 1 of 3", and so on. 🎉

When **every question has been processed**, chatterg also prints a note on the screen:

```
Done: 3 questions over 2m 10s: 3 answered.
```

(It counts questions, how long it took, how many were answered, rejected or never asked, and how many naps it took.)

When it is **finished**, chatterg prints all the answers, like this:

```
q001
  A zeolite is a crystalline material with tiny pores.
  Accepted
q002
  A membrane is a thin barrier that lets some things through.
  Accepted
```

Each answer shows: the question name, what the bot said, and whether chatterg was happy with it (`Accepted`) or not (`Rejected`).

### Want to keep the answers in a file?

The progress messages go to one place (stderr) and the answers go to another (stdout). So you can save just the answers:

```bash
./target/release/chatterg  https://the-bots-address.example/agent  my_questions.txt  > answers.txt
```

> 🔎 The answers are only printed when chatterg is **finished**. If you stop it early, make a report from the notebook. See "Making a report" below.

---

## The notebook (how chatterg remembers)

chatterg writes everything into a small file called **`chatterg.db`** in the folder where you ran it. We call it "the notebook".

- ✅ After every answer, chatterg writes it down.
- ✅ If your computer goes to sleep, or you press **Ctrl+C**, nothing is lost.
- ✅ Run **the exact same command again** and chatterg picks up where it stopped. It does not ask the same questions twice.
- 🕒 It also writes down **when** each question was asked, **how long** the bot took, and every time it had to take a nap.
- 🔒 The notebook is **locked** while chatterg is using it. If you start a second chatterg on the same notebook, it politely refuses instead of messing up the first one.
- 📎 You may see extra files next to the notebook (`chatterg.db-wal`, `chatterg.db-shm`, `chatterg.db.lock`). They are normal helpers. Don't delete them while chatterg is running.
- 🔄 Older notebooks (made before times were saved) are upgraded automatically when you open them.

Want a different notebook? Use `--store`:

```bash
./target/release/chatterg  https://the-bots-address.example/agent  my_questions.txt  --store monday.db
```

**Important:** one notebook = one conversation.

- Want to start over? Use `--restart` (it keeps a backup of the old run), use a new notebook name, or delete the old `.db` file.
- Using the same notebook after the run is **finished** just prints the old answers again. It does not ask anything.
- If a run was **stopped on purpose** (see `on_failure: abort` below), it stays stopped. Use a new notebook to try again.

---

## All the switches (flags) 🎛️

Switches go at the end of the command. Every switch is **optional**. If you don't use one, chatterg uses the "normal" value.

```bash
chatterg <AGENT_URL> <QUESTIONS_FILE> [switches]
```

| Switch | Normal value | What it does, in simple words |
|---|---|---|
| `--store <FILE>` | `chatterg.db` | **Which notebook to use.** Same notebook = carry on where you stopped. |
| `--force-resume` | off | **Carry on even though you edited the questions file**, as long as every question already asked is unchanged. Edits to later questions, or new questions added at the end, are fine. |
| `--restart` | off | **Start over.** Copies the old notebook entry into a backup file (`<notebook>.<date-time>.bak`) and begins again at question 1. |
| `--report <FILE>` | none | **Write a report when the run finishes.** The kind of file comes from its ending: `.md`, `.html`, `.csv` or `.json`. Use it several times to get several. Missing folders are created. |
| `--overwrite` | off | **Allow `--report` to replace a file that already exists.** (Without it, chatterg refuses before it starts, so you don't lose an hour first.) |
| `--retries <N>` | `2` | **If the answer is bad, ask again this many extra times.** `2` means up to 3 tries in total. Only for plain questions (see below). |
| `--reject-phrase <WORDS>` | a built-in list | **Words that mean "the bot is dodging."** If the answer contains one, it counts as a bad answer and chatterg asks again. You can use this switch many times. Using it **replaces** the built-in list. |
| `--cooldown <SECONDS>` | `120` | **How long to nap** when the bot stops answering (too many questions, error, no reply). 120 seconds = 2 minutes. |
| `--max-waits <N>` | `30` | **How many naps in a row before giving up.** 30 naps of 2 minutes is about 1 hour. |
| `--delay <SECONDS>` | `0` | **A pause between questions**, so you don't ask too fast. Great for strict bots. |
| `--timeout <SECONDS>` | `120` | **How long to wait for ONE answer** before saying "no reply" and taking a nap. |
| `--rate-limit-phrase <WORDS>` | a built-in list | **Words that mean "slow down!"** in a short reply. You can use this switch many times. Using it **replaces** the built-in list. |
| `--help` | | Shows the help message. |

### Examples

**Ask slowly (one question a minute) so a strict bot stays happy:**

```bash
./target/release/chatterg  https://bot.example/agent  questions.txt  --delay 60
```

**Nap for 10 minutes when the bot says "too many", and be very patient:**

```bash
./target/release/chatterg  https://bot.example/agent  questions.txt  --cooldown 600 --max-waits 100
```

**Ask each question up to 5 times if the bot dodges, and use my own "dodge" words:**

```bash
./target/release/chatterg  https://bot.example/agent  questions.txt \
  --retries 4 --reject-phrase "meeting" --reject-phrase "call me"
```

**Use a named notebook:**

```bash
./target/release/chatterg  https://bot.example/agent  questions.txt  --store zeolite_run.db
```

---

## What if I change my questions file halfway? ✏️

chatterg remembers a "fingerprint" of your questions from when the run started. If you edit the file and run again with the same notebook, it **checks** instead of guessing:

| What you changed | What chatterg does |
|---|---|
| Nothing (or only retry/dodge-word settings) | Carries on. |
| Added questions at the end, or reworded questions that were **not asked yet** | Stops and tells you what changed. Add `--force-resume` to carry on with the new list. |
| Reworded, moved or removed a question that was **already asked** (or is being asked right now) | Refuses, even with `--force-resume`, because the answers would end up next to the wrong questions. Use `--restart` or a new notebook. |

> ⚠️ Plain-text questions are named by their position (`q001`, `q002`, ...). If you insert a line in the **middle**, every question after it gets a new name, so chatterg will see that as a big change. Add new questions at the **end** if you want to keep going.

---

## How chatterg tells you how it went (exit codes) 🚦

When chatterg stops, it leaves a little number behind, so scripts can tell what happened:

| Number | Meaning |
|---|---|
| `0` | All questions were processed. (Some answers may still say `Rejected`. Look at the answers.) |
| `1` | Something went wrong: bad file, bad address, network or storage problem, or the questions file changed. |
| `2` | The run **ended early on purpose** because a question failed and its `on_failure` was `abort` (or `unknown`). |
| `3` | **Gave up:** the bot stayed unavailable for `--max-waits` naps in a row. Run the same command later to carry on. |

In a terminal you can see the number with `echo $?` right after chatterg ends.

---

## What does chatterg do when things go wrong?

### 1. The bot gives a dodgy answer 🙈

Example: you ask "What is your company name?" and the bot says "Let's schedule a meeting!"

- chatterg sees the word **"meeting"** (it's on the dodge list), so it says "hmm, that's not an answer."
- It asks again with friendlier words ("Please answer directly and concisely, without proposing a meeting or call: ...").
- It does this up to `--retries` extra times.
- If the bot keeps dodging, chatterg writes the answer down as `Rejected` and **moves to the next question**. It does not get stuck.

The built-in dodge words: `meeting`, `schedule a call`, `book a call`, `book a demo`, `calendly`, `get in touch`, `contact us`.

> ⚠️ A good answer that happens to contain one of these words would also be counted as dodgy. Check any `Rejected` answers yourself.

### 2. The bot says "slow down!" (rate limit) 😴

Some bots only let you ask a few questions per hour. chatterg notices when:

- the bot answers with a "try later" error (HTTP 429, or an error in the 5xx family, or 408/425),
- the bot can't be reached, or doesn't answer within `--timeout`,
- the bot's error message contains words like *rate limit*,
- the bot's **short** reply (under 400 letters) says something like *rate limit*, *too many requests*, *try again later* or *slow down*.

Then chatterg:

1. Says so on screen:
   ```
   agent unavailable: agent returned a protocol error (-32003): Rate limit exceeded for this caller
   pausing 120s, then resuming (cooldown 1/30)
   ```
2. Naps for `--cooldown` seconds.
3. Asks **the very same question** again. The question that was never answered is not counted or lost.
4. If it naps `--max-waits` times in a row and the bot still won't talk, it stops with an error. Your notebook is safe: run the same command later and it continues.

Long, real answers that happen to mention "the rate-limiting step" are **not** mistaken for a rate limit.

> 🧠 Tip: the limit is usually kept by the bot's server, not by chatterg. Turning chatterg off and on does not reset it. Using `--delay` so you ask slowly is the best way to avoid it.

### 3. A real mistake happens 🚫

Things like "wrong address" (HTTP 404), "this isn't the kind of bot I know how to talk to," or a broken reply. chatterg does **not** nap for these. It stops straight away and tells you what went wrong on screen (see "Help! Something went wrong" below).

---

## Fancy questions (YAML file)

Plain text is enough for most people. But if you want **different rules for different questions**, make a file ending in `.yaml` or `.yml`.

Files ending in `.yaml` / `.yml` are read as YAML. **Any other ending** (like `.txt`) is read as "one question per line".

### The simple YAML

Just a list of questions:

```yaml
questions:
  - "What is a zeolite?"
  - "What is a membrane?"
```

(A plain list without the `questions:` line works too.)

### The fancy YAML (full question cards)

You can mix simple questions and full "question cards":

```yaml
questions:
  - "What is Nxtbrane?"                       # simple one, gets normal rules

  - id: employees                             # a full card
    question: "How many people work there?"
    required: true
    type: integer
    max_followups: 3
    on_failure: continue

  - id: stage
    question: "Which stage are you at? Reply with one word."
    followup: "Please reply with exactly one of: prototype, pilot, commercial."
    required: true
    type: enum
    values: [prototype, pilot, commercial]
    max_followups: 2
    on_failure: skip
```

### What goes on a question card

| Field | What it means | Needed? |
|---|---|---|
| `id` | A short, unique name for the question (no two cards can share one) | Yes |
| `question` | The words chatterg says to the bot | Yes |
| `required` | `true` = an empty answer is not OK. `false` = an empty answer is OK | Yes |
| `type` | What kind of answer you expect (table below) | Yes |
| `values` | The allowed answers, for `type: enum` | Only for `enum` |
| `max_followups` | How many extra times to ask if the answer is bad. `0` = ask just once. | No (normal: `0`) |
| `on_failure` | What to do if the answer is still bad after all tries (table below) | No (normal: `abort`) |
| `reject_if_contains` | A list of "dodge" words for this question | No |
| `followup` | The words to use when asking again (instead of repeating the question) | No |

**Answer types (`type`):**

| `type` | The answer is OK when... |
|---|---|
| `string` | it is any non-empty text (good for short answers) |
| `text` | it is any non-empty text (good for long answers) |
| `integer` | the whole reply is a whole number, like `42` |
| `list` | it has at least one item in a comma-separated list, like `India, Singapore` |
| `enum` | the whole reply is exactly one of the `values` (capital letters don't matter) |

**What to do when it still goes wrong (`on_failure`):**

| `on_failure` | What happens |
|---|---|
| `abort` | **Stop the whole run.** (This is the normal choice for question cards.) |
| `skip` | Give up on this question and go to the next one. |
| `continue` | Same as `skip`. Write it down and go to the next one. |
| `unknown` | Stop the whole run (like `abort`). |

> Simple (plain) questions automatically get: type `text`, required, 3 tries in total, `on_failure: continue`, the built-in dodge words, and a friendly follow-up. The `--retries` and `--reject-phrase` switches change these for **simple questions only**. Full question cards keep their own rules.

---

## Making a report 📄

chatterg can turn the notebook into a tidy document, even while a run is still going (it only **reads** the notebook, so it never gets in the way):

```bash
# Print a Markdown report on the screen
./target/release/chatterg report --store chatterg.db

# Save it as a web page (the format is guessed from the file name: .md .html .csv .json)
./target/release/chatterg report --store chatterg.db --out report.html

# Spreadsheet or program friendly
./target/release/chatterg report --store chatterg.db --out answers.csv
./target/release/chatterg report --store chatterg.db --format json
```

| Switch | What it does |
|---|---|
| `--store <FILE>` | **Which notebook to read** (normal: `chatterg.db`). |
| `--format md\|html\|csv\|json` | **What kind of document.** If you leave it out, chatterg looks at the ending of `--out` (`.html`, `.csv`, ...). Otherwise it uses Markdown. |
| `--out <FILE>` | **Save to a file** instead of printing. chatterg will **not** replace a file that already exists. |
| `--overwrite` | **Allow replacing** the `--out` file. |
| `--questions <FILE>` | **The questions file you used.** Adds the section headings and lists questions that were never asked. It must be the same questions as the run used, or chatterg refuses. |

What's inside a report:

1. **The facts:** which bot, when it started and finished, how long it took.
2. **A summary:** how many questions, answered, rejected, not asked, retries, naps, and how fast the bot replied.
3. **All answers**, grouped by section.
4. **Needs attention:** every question that was rejected or never asked, with the reason (for example `contains the evasive phrase "meeting"`).
5. **The nap log:** every time chatterg had to wait, and why.
6. **A full transcript** of every attempt, with times.

### Get the report automatically when the run finishes ⚙️

Add `--report` to the normal run. When the last question is processed, chatterg writes the file for you:

```bash
./target/release/chatterg https://the-bots-address.example/agent questions.txt \
  --store zeolite.db --report report.html --report answers.csv
```

```
Done: 89 questions over 3h 12m 05s: 87 answered, 2 rejected. Paused 3 times for 30m 00s.
Report: report.html
Report: answers.csv
```

- The report **has the section headings** from your questions file automatically.
- It is only written when the run **finishes**. If the run ends early (exit code 2) or chatterg gives up (exit code 3), no report is written. You can still ask for one by hand with `chatterg report`.
- chatterg checks your `--report` names **before it asks anything**. If a name has no known ending, or the file already exists (and you didn't add `--overwrite`), it stops straight away. It will never replace your questions file or your notebook.
- If the report can't be written at the end (for example, the disk is full), your run is **still saved**. chatterg tells you the `chatterg report --store ...` command to try again.
- Running the same command again on a finished notebook asks nothing and prints the note again. With `--report` and an existing file it stops, unless you add `--overwrite`.

Safety notes:

- Everything the bot said is treated as **untrusted text**. In the HTML report it is escaped, so a bot can't put code in your page. In the CSV report, cells that start with `=`, `+`, `-` or `@` get a `'` in front so a spreadsheet will not run them as formulas.
- The HTML report is **one file** with no outside links. You can email it or open it offline.
- The same notebook always gives the same report. No clock is read.

---

## Reading the notebook yourself (getting answers out of `chatterg.db`)

The report above is the easy way. If you want to look inside by hand, you can, any time, even when chatterg is stopped.

This needs Python 3 (nothing to install):

```bash
python3 - chatterg.db <<'EOF'
import json, sqlite3, sys

c = json.loads(sqlite3.connect(sys.argv[1]).execute("select data from conversations").fetchone()[0])
print(f"state: {c['state']} | questions finished: {c['position']} | messages sent: {c['messages_sent']}")
print(f"started: {c.get('started_at')} | finished: {c.get('finished_at')} | naps: {len(c.get('cooldowns', []))}\n")

for record in c["answers"]:
    last = record["attempts"][-1]
    print(f"## {record['question_id']}: {last['request']}")
    took = f", {last['latency_ms']} ms" if last.get("latency_ms") is not None else ""
    print(f"({last['validation']}, tries: {len(record['attempts'])}{took})")
    print(last["response"] + "\n")
EOF
```

Save it as a Markdown file by adding `> results.md` at the end of the first line:

```bash
python3 - chatterg.db > results.md <<'EOF'
...same script as above...
EOF
```

What the words mean:

- `state`: `Complete` = finished. `Aborted` = stopped on purpose. `Ready` or `{"Waiting": ...}` = still in the middle.
- `validation`: `accepted` = good answer. `rejected` = chatterg wasn't happy with it.
- `request`: what chatterg actually said last time (might be the follow-up wording).

---

## Which bots can chatterg talk to?

Today: bots that speak **A2A** (the "Agent2Agent" language) over the web, using **JSON-RPC** with the `message/send` method.

How chatterg finds out:

1. You give it an address, like `https://flowmarket.social/nxtbrane`.
2. It looks for the bot's business card at `<address>/.well-known/agent-card.json`.
3. The card tells it where to send messages. chatterg needs a **JSONRPC** entry in the card's `supportedInterfaces` (protocol version `1.0`).

If a bot doesn't have a card like that, chatterg can't talk to it yet.

---

## Help! Something went wrong 🆘

chatterg tells you what happened on the screen, starting with `error:`. Here is what the common ones mean:

| What you see | What it means | What to do |
|---|---|---|
| `no such notebook` (from `report`) | The `--store` file isn't there. `report` never creates one | Check the notebook name |
| `the notebook <file> does not contain a run yet` | The notebook exists but chatterg never asked anything | Run chatterg first |
| `<file> already exists; choose another name or add --overwrite` | `report --out` would replace a file | Pick a new name, or add `--overwrite` |
| `the questions file does not match the one this run used` | `report --questions` was given different questions | Use the original file, or leave `--questions` out |
| `cannot read questionnaire <file>` | The questions file isn't where you said | Check the file name and folder |
| `invalid questionnaire <file>` | A `.yaml` file has a spelling or spacing mistake | Check the YAML (spaces matter!) |
| `invalid question #N in <file>` | Question card number N is missing a field or has a wrong value | Look at that card |
| `questionnaire contains no questions` | The file is empty (or only comments) | Add some questions |
| `duplicate question id: X` | Two cards have the same `id` | Give each a different `id` |
| `cannot open database <file>` | The notebook's folder doesn't exist, or you can't write there | Check the `--store` path |
| `stored conversation is corrupt` | The notebook is broken or from an older version of chatterg | Use a new notebook name |
| `question not found: X` or `stored position ... is beyond` | You are using an old notebook with a **different** questions file | Use a new notebook |
| `HTTP request failed with status 404` | The address is wrong | Check the URL |
| `malformed agent card` | The address doesn't point at an A2A bot | Check the URL |
| `unsupported protocol` | The bot doesn't offer JSON-RPC version 1.0 | chatterg can't talk to this bot (yet) |
| `another chatterg is already using <file>` | A second chatterg is running on the same notebook | Wait for it to finish, or use a different `--store` |
| `the questions file changed since this run started` | You edited the questions after starting | Read the message. Use `--force-resume` (safe edits only), `--restart`, or a new notebook |
| `stored conversation uses format version N` | The notebook was made by a **newer** chatterg | Update chatterg |
| `cannot tell the report format from <file>` | A `--report` name doesn't end in `.md`, `.html`, `.csv` or `.json` | Rename it |
| `the report file <file> already exists` | `--report` would replace a file | Pick a new name, or add `--overwrite` |
| `--report <file> would overwrite your questions file` (or notebook) | You pointed `--report` at an important file | Choose a different name |
| `the run finished and is saved in <db>, but a report could not be written` | The run is fine, only the report file failed | Fix the folder or disk, then run the `chatterg report --store <db>` command it shows |
| `agent still unavailable after N cooldowns` | The bot kept saying "slow down" or never answered, and chatterg ran out of naps | Wait, then run the **same** command again. Or use a bigger `--delay`, `--cooldown` or `--max-waits` |

If chatterg ends but some answers say `Rejected`, that is **not** a crash. It means the bot gave answers chatterg didn't like, and chatterg moved on.

---

## Good manners 🙏

- Only ask bots that **you own** or that you have **permission** to ask.
- Respect rate limits. Use `--delay` and long `--cooldown` values. Don't try to dodge a bot's limits.
- If you need to ask many questions, ask the bot's owner for permission or a higher limit. It is faster and friendlier.

---

## Checking that chatterg itself works (for people who change the code)

```bash
cargo test                  # runs all the tests
./scripts/gate.sh           # formats the code, runs the tests, and runs the code checker (clippy)
./scripts/test-nxtbrane.sh  # (optional) real test against the NxTBrane bot. Needs internet.
```

Tests use pretend bots, so they don't need the internet and never bother a real bot.

### Where things live

```
chatterg/
├── Cargo.toml                 the shopping list for Rust
├── ROADMAP.md                 what we plan to build next
├── questions.yaml             a small example questions file
├── questions/
│   └── zeolite_membranes.txt  120 ready-made questions
├── scripts/                   helper scripts (gate.sh, test-nxtbrane.sh)
├── .github/                   automatic checks on GitHub (tests, lints, security audit)
├── src/
│   ├── main.rs                the front door: reads your switches
│   ├── application.rs         the boss: asks, waits, naps, saves
│   ├── domain/                the rules: questions, answers, checking answers
│   ├── storage/               the notebook (SQLite): saving, locking, upgrading old notebooks
│   ├── transport/             how to talk to bots (A2A) + a pretend bot for tests
│   └── output/                how answers are shown: on screen (human) and reports (report.rs)
└── tests/                     the checks that prove it works
```

---

## What chatterg can't do (yet)

- It **doesn't understand** answers. It can't tell if an answer is *correct*. It only checks simple things (empty? a number? one of the allowed words? a dodge word?).
- Each question is sent on its own. The bot is not told about the earlier questions.
- One notebook holds **one** conversation.
- It only talks to A2A bots for now.
- It only writes a report by itself when you add `--report`. There is no default report yet, and it doesn't empty your questions file afterwards (both are planned, see the roadmap).

Want to know what we plan to build next (reports, safer resume, smarter waiting, talking to more kinds of bots)? Read **[ROADMAP.md](ROADMAP.md)**.

---

## Cheat sheet 🧾

```bash
# Build once
cargo build --release

# Ask questions
./target/release/chatterg  <BOT_URL>  <QUESTIONS_FILE>

# Ask slowly, and nap 5 minutes if the bot says "slow down"
./target/release/chatterg  <BOT_URL>  <QUESTIONS_FILE>  --delay 30 --cooldown 300

# Stopped halfway? Run the SAME command again. It carries on.

# Edited the questions file halfway (only later questions)? Carry on with the new list
./target/release/chatterg  <BOT_URL>  <QUESTIONS_FILE>  --force-resume

# Start over but keep a backup of the old run
./target/release/chatterg  <BOT_URL>  <QUESTIONS_FILE>  --restart

# How did it go? (0 = done, 1 = error, 2 = ended early, 3 = bot unavailable)
echo $?

# Start over: use a new notebook
./target/release/chatterg  <BOT_URL>  <QUESTIONS_FILE>  --store new_run.db

# Get a report automatically when the run finishes
./target/release/chatterg  <BOT_URL>  <QUESTIONS_FILE>  --report report.html

# Make a report from a notebook yourself (Markdown on screen, or a file in the format of its ending)
./target/release/chatterg  report  --store chatterg.db
./target/release/chatterg  report  --store chatterg.db  --out report.html

# Save the answers to a file
./target/release/chatterg  <BOT_URL>  <QUESTIONS_FILE>  > answers.txt

# See all the switches
./target/release/chatterg --help
```
