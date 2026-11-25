# Paparan AI agent loop

The harness Paparan AI uses inside Kantor to make decks, documents and workbooks: a model and
Kantor's tools take turns until the model calls `finish`. There is no chat UI yet; `kantor agent
run` drives it from a terminal, and a GPUI chat panel will render the same events.

| Part | Where |
|---|---|
| The loop: steps, budgets, nudges, `finish`, events, conversation pruning | `crates/kantor_ai/src/agent/` |
| Skill prompts (local folder, or served by Paparan AI) | `kantor_ai/src/agent/skill.rs`, `kantor_ai/src/paparan.rs` |
| The document tools, confined to one work folder | `crates/kantor/src/harness/` |
| `kantor agent run` | `crates/kantor/src/cli/agent.rs` |

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
  fetches Paparan AI's own skill (below); `--skill DIR` still reads a local folder.

```sh
kantor agent run --provider paparan --skill sheet --dir job "Check rekap.xlsx and fix the totals"
```
- Exit status 0 only when the model called `finish` and it was accepted.
- Ctrl+C stops after the current step; twice quits.

## Rules the loop enforces

These come from running the Paparan skills with a cheap model through a general agent harness.

- **Only `finish` ends a job.** A turn with text and no tool call gets a nudge (at most
  `max_nudges`, default 3), worded more firmly when the text announces work ("Let me
  restructure…"). The summary the user reads is `finish`'s `summary`, so a job can't end on a plan.
- **`finish` is checked.** Every declared output must exist in the work folder, and `.pptx`,
  `.docx` and `.xlsx` outputs must pass `check` without error-level findings; otherwise `finish`
  is refused with the errors and the model goes on.
- **Budgets.** `max_steps` (default 40), `max_tokens` and `max_cost` (with prices). The loop stops
  before a step that would go over, counting the next step as at least as large as the last.
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
- Tool results over `max_result_chars` (12,000) are cut with a note on how to page. Results four
  steps old are cut to about 1,500 characters, and strings over 1,500 characters in older tool-call
  arguments (a whole `deck.json` sent to `write_file`) are replaced by a note ("read the file"). Old
  messages are changed only once, so the provider's prompt cache keeps the prefix up to them.

## Tools

All confined to the work folder (no `..`, no absolute paths outside it, no symlinks out, picture
paths in build and edit JSON too). Files in the folder when the job starts are the user's: they
can be read but not overwritten.

| Tool | Does |
|---|---|
| `list_files`, `read_file`, `write_file`, `replace_in_file` | files; PNG/JPEG come back as pictures |
| `build` | JSON → .pptx/.docx/.xlsx, then `check` and the contact sheet (the skills' `build.sh`) |
| `check`, `list`, `edit`, `schema` | as `kantor slides|docs|sheets …`; `edit` takes the ops inline |
| `look` | contact sheet or pages as pictures (at most 4), saved under `shots/` |
| `convert` | `kantor convert` (PDF copies, CSV ↔ xlsx, .doc → .docx, …) |
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

## Tests

`kantor_ai::agent::scripted::Scripted` plays back a script of replies (no network). The tests
cover the nudge and `finish` rules, refusals for missing outputs and check errors, picture
pruning, truncation and shortening of old steps, budgets, step limits, cancellation and skill
files (`crates/kantor_ai/src/agent/tests.rs`, `crates/kantor/src/harness/tests.rs`). Served
skills are tested against a scripted Paparan server (`crates/kantor_ai/src/paparan.rs`): one
fetch, reads from memory, path forms, and the signed-out, no-credit, unknown-skill and offline
errors.
