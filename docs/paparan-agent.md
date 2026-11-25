# Paparan AI agent loop

The harness Paparan AI uses inside Kantor to make decks, documents and workbooks: a model and
Kantor's tools take turns until the model calls `finish`. `kantor agent run` drives it from a
terminal; the chat panel (below) renders the same events in the app.

| Part | Where |
|---|---|
| The loop: steps, budgets, nudges, `finish`, events, conversation pruning | `crates/kantor_ai/src/agent/` |
| Skill prompts (local folder, hook for served prompts) | `kantor_ai/src/agent/skill.rs` |
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
- `--provider paparan` uses the Paparan AI sign-in saved by Kantor (credits) instead of a key.
- Exit status 0 only when the model called `finish` and it was accepted.
- Ctrl+C stops after the current step; twice quits.

## The chat panel

Paparan AI Chat (command palette, or the sparkle button in the tab strip) docks on the right.

- The user types a request and attaches source files (paper clip, or drop them on the panel). The
  kind (deck, document, workbook) is guessed from the words and the files' types, and can be
  picked instead; it chooses the skill (`paparan-deck|doc|sheet`). Skills come through a loader
  (`job::SkillLoader`, a `PromptSource` per kind): today local folders under `ai.skills_dir` in
  settings.toml (default `~/.claude/skills`); served skills plug in as another loader.
- Each job has a work folder: new under `<data dir>/paparan/jobs/<time>-<words>`, or one the user
  picks before the first request. Attachments are copied in and count as the user's files (never
  overwritten).
- The provider is the one in AI Settings: Paparan AI (credits shown), any OpenAI-compatible API,
  or Ollama (through its `/v1` endpoint). Anthropic's own API can't run jobs yet.
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
`ServedSkill` takes a fetch function: the hook for prompts served by the Paparan backend, which
keeps the paid prompts on the server; the skill text is never compiled into Kantor.
TODO: the backend has no skills endpoint yet, so `--skill paparan:NAME` is refused for now.

## Tests

`kantor_ai::agent::scripted::Scripted` plays back a script of replies (no network). The tests
cover the nudge and `finish` rules, refusals for missing outputs and check errors, picture
pruning, truncation and shortening of old steps, budgets, step limits, cancellation and skill
files (`crates/kantor_ai/src/agent/tests.rs`, `crates/kantor/src/harness/tests.rs`).
