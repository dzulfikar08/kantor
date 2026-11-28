# Paparan AI agent loop

The harness Paparan AI uses inside Kantor to make decks, documents and workbooks: a model and
Kantor's tools take turns until the model calls `finish`. `kantor agent run` drives it from a
terminal; the chat panel (below) renders the same events in the app.

| Part | Where |
|---|---|
| The loop: steps, budgets, nudges, `finish`, events, conversation pruning | `crates/kantor_ai/src/agent/` |
| Skill prompts (local folder, or served by Paparan AI) | `kantor_ai/src/agent/skill.rs`, `kantor_ai/src/paparan.rs` |
| The document tools, confined to one work folder | `crates/kantor/src/harness/` |
| `kantor agent run` | `crates/kantor/src/cli/agent.rs` |
| The chat panel | `crates/kantor/src/ai/chat/` (docked by `workspace/chat.rs`) |

## Running it

```sh
export CF_TOKEN=…   # any OpenAI-compatible API with tool calls (and vision, for `look`)
kantor agent run --skill ~/.claude/skills/paparan-deck --dir job \
  --model @cf/deepseek-ai/deepseek-v4.1-flash-1017 \
  --base-url https://api.cloudflare.com/client/v4/accounts/ACCOUNT/ai/v1 \
  --api-key-env CF_TOKEN \
  --price-input 0.30 --price-cached 0.006 --price-output 1.20 --max-cost 0.50 \
  --json "A 6-slide deck for …"
```

- `--json` prints one event per line (`step_start`, `text`, `reasoning`, `tool_call`, `tool_result`,
  `images_dropped`, `nudge`, `usage`, `done`, `stopped`, `error`), then a `usage_total` line with
  input, cached input and output tokens (and the cost when prices are given). Without `--json` the
  same events are printed for people, ending with a `usage:` line.
- `--provider paparan` uses the Paparan AI sign-in saved by Kantor (credits) instead of a key,
  with the `paparan-agent` model unless `--model` says otherwise. `--skill deck|doc|sheet` then
  fetches Paparan AI's own skill (below); `--skill DIR` still reads a local folder. Costs are
  counted in credits with Paparan AI's price list (`paparan::AGENT_PRICES`, 1,500 / 30 / 6,000
  credits per million input / cached / output tokens; the server's count is what's charged), the
  budget is `--max-credits N`, and `reasoning_effort` is `low` (`--reasoning` changes it).

```sh
kantor agent run --provider paparan --skill sheet --dir job "Check rekap.xlsx and fix the totals"
```
- Exit status 0 when the job finished: the model's accepted `finish`, or the documents it made
  when it reached a limit (`usage_total` then has `"limit": "steps"` or `"budget"`).
- Ctrl+C stops after the current step; twice quits.

## The chat panel

Paparan AI Chat (command palette, or the sparkle button in the tab strip) docks on the right.

- The user types a request and attaches source files (paper clip, or drop them on the panel). The
  kind (deck, document, workbook) is guessed from the words and the files' types, and can be
  picked instead; it chooses the skill (`paparan-deck|doc|sheet`). Skills come through a loader
  (`job::SkillLoader`, a `PromptSource` per kind): with Paparan AI as the provider, its served
  skill (`paparan::load_skill`, fetched per job, in memory only) and the `paparan-agent` model;
  with other providers (BYOK, Ollama), local folders under `~/.claude/skills`. `ai.skills_dir` in
  settings.toml, when set, wins for every provider (development).
- Failures say what to do: signed out offers Sign in, no credits Top up (the account's link),
  offline or a server error Try again (the same request, no repeat in the list); an unknown skill
  names the ones there are.
- Each job has a work folder: new under `<data dir>/paparan/jobs/<time>-<words>`, or one the user
  picks before the first request. Attachments are copied in and count as the user's files (never
  overwritten).
- The provider is the one in AI Settings: Paparan AI (credits shown), any OpenAI-compatible API,
  or Ollama (through its `/v1` endpoint). Anthropic's own API can't run jobs yet.
- With Paparan AI a request may spend `job::MAX_CREDITS` (500; a follow-up gets as many again),
  with short reasoning (`job::limits`). The usage line shows about how many credits it took.
- The full skills are Paparan AI's paid part and stay on its server. With another provider and no
  local folder for the job's skill (`job::local_or_paparan`), the panel says that decks, documents
  and workbooks need Paparan AI and offers Sign in (AI Settings), instead of an unknown-skill
  error. A folder set in `ai.skills_dir` reads as before (development).
- The harness runs on a worker thread; events come back in batches (one redraw per batch). The
  list shows the model's text (Markdown), a row per tool call (name, the files it works on;
  click for the result), `look` pictures as thumbnails (decoded on the worker; click to
  enlarge), nudges as quiet lines, a usage line, then the `finish` summary with the output files
  (Open in a tab, Reveal in the file manager). Stop cancels after the current step.
- Follow-ups continue the job: same folder, same tools (files the job made stay its own to
  change), and the conversation goes on (`agent::resume`; pictures from earlier runs become
  notes, calls a stopped run left unanswered get a result).
- `KANTOR_CHAT_SCRIPT=script.json` replaces the model with a script of turns (`ai/chat/script.rs`)
  while the tools stay real: for demos and screenshots without a provider.

## Rules the loop enforces

These come from running the Paparan skills with a cheap model through a general agent harness.

- **Only `finish` ends a job.** A turn with text and no tool call gets a nudge (at most
  `max_nudges`, default 3), worded more firmly when the text announces work ("Let me
  restructure…"). The summary the user reads is `finish`'s `summary`, so a job can't end on a plan.
- **`finish` is checked.** Every declared output must exist in the work folder, and `.pptx`,
  `.docx` and `.xlsx` outputs must pass `check` without error-level findings; otherwise `finish`
  is refused with the errors and the model goes on. Warnings never block it: the ones left are
  appended to the summary ("Check warnings left (n)", in Indonesian for Indonesian requests).
  `build` and `check` results say so too ("No errors: the file can be handed over…"), because
  cheap models otherwise loop on warnings until the step limit.
- **Steps.** `max_steps` defaults to 24 (the jobs that finish well take 10 to 20). From a quarter
  of the steps before the end the model is told how many are left and to wrap up with what
  exists; the last step runs only `finish` (other calls aren't run; it is offered the same tools,
  since a different tool list would end the prompt cache and re-read the conversation at full
  price).
- **Budgets.** `max_tokens` and `max_cost` (with prices). Before each step the loop estimates
  its input (the last step's input plus what was added since, a picture as about 1,000 tokens,
  the cached prefix priced as cached) and gives the reply what is left after it: the step is
  sent with `max_tokens` set to that (when under 16k), so a step can't run far over the budget,
  and a step that couldn't afford 1,500 output tokens isn't started. At 70% of the budget, or
  when less than three typical steps are left, the model is told once to wrap up.
- **Ending at a limit hands over what exists.** At the step limit, the budget or too many turns
  without `finish`, the loop asks the tools for the documents the job made that pass their check
  (`Tools::made_outputs`: new .pptx/.docx/.xlsx outside `shots/`, 0 errors) and, when there are
  any, ends `Finished` with them: a summary saying the limit was reached, the model's last note
  and the warnings left. `Event::Done` and `Outcome` carry the limit. Nothing made: `Stopped`.
- **Language.** The rules ask for notes and the summary in the request's language, and the
  system prompt ends with an explicit note when the request reads as Indonesian or English
  (`request_language`, word lists): the skills are in English and cheap models narrate in them,
  and with Indonesian data an English request got an Indonesian deck.
- **Reasoning.** `Limits::reasoning` goes to every step as `reasoning_effort`. Thinking models'
  reasoning is paid as output (4 times input's price, 200 times cached input's) and was two
  thirds of a job's cost; `low` about halves it. Each step's reasoning goes back with its turn
  as `reasoning_content` (`Limits::keep_reasoning`), so the model goes on from its plan instead
  of working it out again on the next step; it is re-read from the cache at 2% of the price of
  writing it. A follow-up request starts without the earlier reasoning.
- **Batched steps.** The rules ask for a plan of about four steps: read the inputs and the skill
  files the job needs in one step, write the JSON and `build` it in one reply, fix errors with
  small `replace_in_file` patches plus `build` (never by rewriting the file), then `finish` with
  a short summary. `build` already returns the check and the contact sheet.
- **A failed build lists every problem.** The builders stop at their first error, so a model
  paid a step and a rebuild per wrong key. When a build fails, the source is also checked
  against the app's JSON Schema (`harness/validate.rs`: `$ref`, types, unknown and missing
  keys, enums, `anyOf`) and every problem is listed with its JSON path. It only explains a
  failed build, so a difference between schema and builder never refuses a good one.
- **Cancellation** between steps and between streamed pieces.

## Keeping the conversation small

Every step resends the whole conversation, which dominates the cost.

- The system prompt is the harness rules, the tools' primer and the skill's `SKILL.md` only.
  `references/` and `examples/` are listed with sizes and read on demand with `read_skill_file`
  (paths as the skill writes them, `~/.claude/skills/NAME/…`, work too, sibling skills included).
- The primer maps the skill's commands to tools (`build.sh` → `build`, `kantor … edit --ops` →
  `edit`, `--to png` → `look`) and gives every schema definition's keys up front (`stat_cards:
  name, x, y, …, items*[]{value*, label, detail, highlight}`), about 1k tokens per app, only for the
  skill's apps. The model doesn't explore the schema one call at a time.
- Pictures go to the model as `image_url` parts of a user message after the tool results. A new
  picture of a page drops older pictures of that page, a picture of a changed file drops every
  older picture of it, and at most `max_images` (default 4) stay; dropped ones become a one-line
  note.
- Tool results over `max_result_chars` (12,000) are cut with a note on how to page. Once the
  conversation is over `old_results_over` (160,000 characters, about 40k tokens), every fourth
  step, results at least four steps old are cut to about 1,500 characters. Not earlier and not
  every step: each cut ends the provider's prompt cache there, and cached input costs 2% of
  fresh input, so in a job of normal size a cut cost more (the rest re-read fresh) than it
  saved. Past the same size, strings over 1,500 characters in older tool-call arguments (a
  whole `deck.json` sent to `write_file`) are replaced by a note saying the call ran and
  `read_file` shows the file. Not earlier: with the note in a short job, DeepSeek took its own
  call for a failed write and wrote the whole file again, two or three times (40 credits each
  for a workbook). Old
  messages are changed only once, so the provider's prompt cache keeps the prefix up to them.

## Tools

All confined to the work folder (no `..`, no absolute paths outside it, no symlinks out, picture
paths in build and edit JSON too). Files in the folder when the job starts are the user's: they
can be read but not overwritten.

| Tool | Does |
|---|---|
| `list_files`, `read_file`, `write_file`, `replace_in_file` | files; PNG/JPEG come back as pictures |
| `build` | JSON → .pptx/.docx/.xlsx, then `check` and the contact sheet (the skills' `build.sh`); a sheet's `csv` key copies a CSV's rows, so the model doesn't retype the user's data |
| `check`, `list`, `edit`, `schema` | as `kantor slides|docs|sheets …`; `edit` takes the ops inline |
| `macros` | a workbook's VBA macros, as `kantor sheets macros`: modules, procedures, code on request (read, never run) |
| `look` | contact sheet or pages as pictures (at most 4), saved under `shots/` |
| `convert` | `kantor convert` (PDF copies, CSV ↔ xlsx, .doc → .docx, …) |
| `pls` | PLS-SEM as `kantor pls run` / `check` on a data file in the folder: a compact summary with rule-of-thumb flags, optionally a .docx report and a results .xlsx ([pls-json.md](pls-json.md)) |
| `read_skill_file`, `finish` | the loop's own |

The tools call the same Rust functions as the CLI, in process.

## Prompts

`PromptSource` is pluggable. `LocalSkill` reads a skill folder (BYOK and development).
`ServedSkill` holds a skill served by the Paparan backend: the skill text is Paparan AI's paid
product, so it is never compiled into Kantor or kept on disk.

- `kantor_ai::paparan::fetch_skill` checks the account (`GET me`: signed in, credits left), then
  fetches the whole skill once with `GET skills/NAME` (Bearer sign-in token; `GET skills` lists
  them). The answer is `{name, version, files: {path: text}}`, text files only; the job keeps it
  in memory and `read_skill_file` reads from there, never going back to the server.
- Errors say what to do: signed out or revoked (sign in again), no credits (the top-up link),
  offline (can't reach Paparan AI), unknown name (the skills there are).
- Paths: the served copy names files relative to the skill (`references/charts.md`, `build.sh`).
  A file of another skill it points at comes in the same answer under that skill's name
  (`paparan-deck/references/charts.md` in the sheet skill), so a skill is one fetch. Paths as
  local skills write them (`~/.claude/skills/NAME/…`, `NAME/…`, `./…`) resolve the same way.
- The backend keeps its copy in sync with the local skills with `scripts/sync-skills.sh` (in the
  Paparan repo), which rewrites the absolute paths to these forms.

## Where a job's credits go

From the 0.25.0 jobs (DeepSeek V4.1 Flash on Workers AI, Nov 2026):

- Output, mostly hidden reasoning, was about two thirds of the cost: a 13-step workbook job read
  126k input tokens (107k of them cached, 3 credits) and wrote 10.8k output tokens (65 credits);
  a step that only read files could think for 4,000 to 6,000 tokens.
- Write steps are 5 to 10 times the size of the steps before them (a 12 KB `deck.json` plus its
  reasoning: 15k output tokens, 96 credits), which is why "the next step costs what the last did"
  let a budget overshoot by a whole step.
- Fresh input came from the last few steps' results, re-sent uncached because old results were
  cut every step.
- Wasted steps: a false `sum_misses_rows` error (a date label beside the amounts) sent a
  workbook job round in circles for 40 steps; another finished all errors and then fixed
  warnings until the step limit.

From the cheaper-jobs round (Nov 2026; GLM-5.3 Flash on Z.ai for iteration, DeepSeek to confirm):

- A model that saw its own `write_file` call with a placeholder in place of the content took
  it for a failed write and wrote the whole file again, two or three times: 40 credits per
  rewrite for a workbook, the largest single waste found. Long arguments now stay whole until
  the conversation is long.
- Without its earlier reasoning, DeepSeek planned the deck again on every step (5,000 to 8,000
  reasoning tokens a step while reading references). Sent back, later steps think for a few
  hundred tokens.
- Builders stop at their first error; GLM took up to four builds (two steps each) to find
  four wrong keys. The schema check lists them together.
- The CSV's rows were retyped into `rows` (96 rows: about 5,000 output tokens, 30 credits);
  the `csv` key copies them.
- A finish-only tool list on the last step re-read the whole conversation uncached.
- Budgets under about 100 credits stop a deck before it is written: the loop keeps room for
  three typical steps, and a deck's write step is the largest.

Measured on Paparan (DeepSeek V4.1 Flash) after the round, one run per job with
`--max-credits 200`, credits from the account balance before and after each job:

| Job | 0.25.1 | After | Steps, input / cached / output tokens |
|---|---|---|---|
| English board deck, 6 slides | 163 credits, 8 steps | 123 credits | 7, 137.7k / 117.6k / 15.0k |
| Indonesian invitation letter | 69, 14 | 102 | 19, 305.8k / 282.4k / 9.7k |
| Budget recap, 54 rows | 179, 17 | 107 | 19, 296.1k / 271.4k / 10.3k |
| Monthly sales per city with line chart | 162, 14 | 96 | 7, 119.2k / 101.9k / 11.3k |
| Total | 573 | 428 (-25%) | |

All four finished with 0 errors. The letter cost more because the model took the contact
sheet's tile number under the page for a page number in the letter and spent 12 steps (about
40 credits) changing the footer to remove it; 0.25.1's contact sheet has the same label.

## Tests

`kantor_ai::agent::scripted::Scripted` plays back a script of replies (no network) and records
each step's messages, tools and options. The tests cover the nudge and `finish` rules, refusals
for missing outputs and check errors, warnings in the summary, picture pruning, truncation and
shortening of old steps, budgets (the capped output, the wrap-up note, handing over what was
made), the finish-only last step, the language note, reasoning effort, cancellation and skill
files (`crates/kantor_ai/src/agent/tests.rs`, `crates/kantor/src/harness/tests.rs`). Served
skills are tested against a scripted Paparan server (`crates/kantor_ai/src/paparan.rs`): one
fetch, reads from memory, path forms, and the signed-out, no-credit, unknown-skill and offline
errors.
