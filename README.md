# Worklog

A native macOS GPUI prototype for a daily speaking timeline, live local transcription, notes, breakpoints, and labeled spans. The centered transcript ruler separates your microphone transcript on the right from other speakers on the left. Blue is Me, yellow is Them, lime is overlap; empty time remains visible.

The left sidebar starts visible and the app opens on today. A slim tick minimap on the right shows the selected day, with blue/yellow/lime speech, teal computer activity, purple notes, and red breakpoints. Longer ticks indicate the visible passage or marked range. Empty time remains neutral. Click to scroll, drag to mark a range (Option-drag to scroll), or use its hover “+” for time-specific annotations. Today extends from midnight to the current time and advances during silence; historical days retain their complete day, including daylight-saving transitions.

Hover the centered transcript ruler for a “+” menu with Comment, Breakpoint, and Start/End range actions. These work on past days and without audio recording. A range is a named connection between two persistent breakpoints; removing its connection preserves the breakpoints and comments. Click a red breakpoint time pill to edit its optional label. Purple notes are centered on their timestamps. Streaming and finalized speech share chronological ordering around notes and breakpoints. Hovering or editing a breakpoint pauses live following and anchors its screen position during transcript updates.

Breakpoint and Record are centered together above the note composer. The record control has a fixed width, shows a spinner until both audio sources and the transcription session are ready, then plays a short on-air cue and displays the live voice meter. Speaker and time appear together on both dimmed streaming drafts and final bubbles. Nemotron supports live partials; Whisper and Parakeet TDT v3 produce completed eight-second chunks (and flush the final remainder on stop). Settings offers all three engines and remembers the choice. Parakeet uses the same local NeMo-Speech runtime with its complete-audio endpoint. Subscreens have Back, and appearance follows macOS.

When microphone and system audio are both selected, Sonora 0.3.0 removes playback echo from the microphone before recognition. The cleaned microphone also drives the voice meter; system audio remains the Them track. Capture preprocessing runs independently of inference, synchronizes device timestamps, and flushes the final partial frame. Raw source recordings remain separate from cleaned recognition input. Microphone-only capture bypasses echo cancellation.

Background echo regression checks use known speech fixtures mixed with a delayed playback copy and a room reflection. They test echo suppression, microphone-only preservation, double talk, frame boundaries, raw retention, and an explicitly invoked local ASR comparison. They do not establish live-device performance for every speaker/microphone combination.

**Prototype limits:** live recording requires the installed local speech runtime/model described in [the capture documentation](packaging/LEGACY-TEAM-UP.md). The initial sample conversation and its activity chart remain fixtures; actual captured speech is stored independently and drives its own activity intervals. Computer activity capture runs locally alongside recording: app/window changes, clicks, browser navigation, and accessibility notifications appear in the Me lane. Settings provides an explicit Accessibility permission action, opening macOS Privacy & Security for the signed `Worklog Activity.app` helper. Enable Worklog Activity there; status updates automatically. The helper keeps a stable identifier across rebuilds. Available native attributes, full URLs, unknown fields, and OS error codes are retained in separate `activity_events` rows without truncation; only the timeline display groups events. Details opens each source payload. Fixture-audio mode does not start desktop activity capture.

## Homebrew

```sh
brew tap hellpat/worklog https://github.com/HellPat/worklog
brew install hellpat/worklog/worklog
worklog
```

This is a custom tap in the source repository, so its explicit URL is required. Homebrew installs `sqldef/sqldef/sqlite3def` as a runtime dependency and builds the GPUI app from the pinned source revision. No Go toolchain is required by users. The initial distribution is a command that opens the native app, not a notarized `.app` bundle. Apple silicon macOS is the currently validated platform.

The default database is `~/Library/Application Support/worklog/worklog.sqlite`. Set `TEAM_UP_SPIKE_DB` for an isolated database. Existing spike fixture databases are not moved or removed.

## Agent CLI

`worklog skill` prints a self-contained `SKILL.md` with command examples, cursor handling, streaming, and output interpretation. Its source is [skills/worklog/SKILL.md](skills/worklog/SKILL.md), embedded in the executable; printing it does not require a database. Redirect stdout to a skill folder's `SKILL.md` to use it with an agent.

Install the skill for both Codex and Claude Code (all projects), using the latest `worklog` on your PATH:

```sh
mkdir -p "$HOME/.agents/skills/worklog" "$HOME/.claude/skills/worklog" &&
worklog skill > "$HOME/.agents/skills/worklog/SKILL.md" &&
cp "$HOME/.agents/skills/worklog/SKILL.md" "$HOME/.claude/skills/worklog/SKILL.md"
```

These are the personal skill locations documented by [Codex](https://learn.chatgpt.com/docs/build-skills#where-codex-loads-local-skills) and [Claude Code](https://code.claude.com/docs/en/skills#choose-where-skills-load).

For the latest source build, run this from the repository root instead:

```sh
cargo build --locked --release --manifest-path spikes/gpui-recorder/Cargo.toml &&
mkdir -p "$HOME/.agents/skills/worklog" "$HOME/.claude/skills/worklog" &&
./spikes/gpui-recorder/target/release/team-up-gpui-spike skill > "$HOME/.agents/skills/worklog/SKILL.md" &&
cp "$HOME/.agents/skills/worklog/SKILL.md" "$HOME/.claude/skills/worklog/SKILL.md"
```

Start a new chat and supply the absolute executable path if `worklog` is not on the agent's PATH. For example:

- Codex: `$worklog Summarize what happened after the breakpoint labeled "since then".`
- Claude Code: `/worklog Summarize what happened after the breakpoint labeled "since then".`

For subsequent reads, ask for changes since the last read; the skill explains how to resume with a cursor. For a bounded live read, ask it to follow for two minutes and summarize afterward. Read transcript and notes first, group activity by app/window/URL, and request full raw activity details only when needed to keep agent context manageable.

Quick check:

```sh
worklog timeline --today --limit 5
worklog timeline --today --follow --duration 30
```

While the second command runs, select today in the app and add a note named `skill-check`. It should appear as a new batch. Speech appears after its utterance is finalized.

Then start a new agent chat and ask `$worklog Find my skill-check note from today and include its ID` in Codex, or `/worklog Find my skill-check note from today and include its ID` in Claude Code. If using a source build instead of a PATH-installed command, supply the executable's absolute path in that prompt.

The same binary provides headless, read-only access for coding agents. `worklog` opens the UI when given no arguments. In a source checkout, use `spikes/gpui-recorder/target/release/team-up-gpui-spike` in place of `worklog`.

```sh
worklog timeline --today
worklog timeline --date 2026-11-10 --include transcript,activity
worklog timeline --after message:42
worklog timeline --after entry:7
worklog timeline --range span:3
worklog timeline --from 2026-11-10T09:00:00+01:00 --to 2026-11-10T10:00:00+01:00
worklog days
worklog activity 'activity:EVENT_UUID'
```

GCF generic format is the default, encoded with the official pinned Rust `gcf` crate. `--json` (or `--format json`) provides the same data as JSON. `activity` returns the complete raw OS payload; ordinary timeline reads return compact app/action/window/URL summaries with an ID for fetching details. Values are not truncated. GCF rejects integers outside its signed 64-bit domain; JSON remains available for such raw payloads.

`timeline` is the main command: `--after` chooses a starting message/breakpoint, `--resume` continues a previous read, `--follow` keeps printing updates, and `--wait SECONDS` waits for one new batch. `--follow` works both with a fresh timeline query and with a resume token. `--date`, `--range`, and `--include` select the initial scope; a resume token retains that scope.

Every timeline batch returns `next_cursor`, `has_more`, and stable namespaced IDs. A first `timeline` query is a consistent snapshot of finalized, persisted data. If `has_more` is true, call `timeline --resume TOKEN` with its cursor to continue that snapshot. Once complete, the same cursor API returns only subsequent changes. Upserts replace the cached object with the same `id`; deletes remove it. Changes for one object are coalesced within a batch, though subsequent pages may update that object again. Streaming drafts are not persisted or exposed by this reader.

```sh
worklog timeline --resume 'CURSOR_FROM_PREVIOUS_BATCH'
# Agent-friendly long poll: returns the next batch or an empty batch on timeout.
worklog timeline --resume 'CURSOR_FROM_PREVIOUS_BATCH' --wait 30
# Continuous, resumable output; Ctrl-C stops it.
worklog timeline --resume 'CURSOR_FROM_PREVIOUS_BATCH' --follow
# Or a bounded stream:
worklog timeline --resume 'CURSOR_FROM_PREVIOUS_BATCH' --follow --duration 60
```

Each streamed GCF document starts with `GCF profile=generic`, includes its own cursor, and is flushed immediately. JSON streaming emits one compact JSON object per line. Save the last fully received cursor to reconnect with `timeline --resume TOKEN`, adding `--follow` for streaming; retrying a cursor replays the same changes rather than consuming a shared queue. Background command support lets an agent read or poll the output; a running stream alone does not guarantee that a client automatically wakes the model for each batch. See [Codex terminal tools](https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide) and [Claude Code background commands](https://code.claude.com/docs/en/interactive-mode#background-bash-commands).

Cursors retain the database identity, selected kinds, and time scope. A `--today` cursor remains scoped to that Berlin calendar day; start a new read for a new day. `--after message:ID` and `--after entry:ID` continue chronologically from that anchor without an upper day limit. Timestamp ranges use an inclusive `--from` and exclusive `--to`; output timestamps are UTC RFC3339. The limit is 200 by default, at most 2000.

The SQLite change feed records writes transactionally in ingestion order, so late finalized speech is delivered even if its speech timestamp predates the last read. Snapshots remain stable while new recording data arrives. Edits and deletions—including entries moved outside the selected range—are delivered explicitly. The cursor survives app/CLI restarts; a cursor from another database or from ahead of a restored backup is rejected.

CLI queries open the database read-only and never start the GUI, audio capture, or sample-data seeding. Open the latest app once to initialize the feed on an older database. Alternatively, explicitly reconcile its schema without opening a window:

```sh
TEAM_UP_SPIKE_DB="$HOME/Library/Application Support/worklog/worklog.sqlite" worklog --check-db
```

`--database PATH` or `TEAM_UP_SPIKE_DB` selects an isolated database. Missing databases are errors rather than new empty files.

The previous `read`, `updates --cursor`, `watch --stream`, `--day`, `--span`, `--kinds`, and `--after-message` spellings remain accepted as compatibility aliases; help shows the simpler names.

## Schema and storage

`spikes/gpui-recorder/schema.sql` is the desired schema, embedded in the executable. On opening a database, `sqlite3def` plans the difference; Rust applies it transactionally and checks foreign keys before committing. Destructive changes are not enabled. Planning failures stop startup with an error. `WORKLOG_SQLITE3DEF` can specify the tool; packaged builds compile in Homebrew's stable dependency path so launching outside a shell also works.

Transcript messages, ranges, replies, and timeline notes/breakpoints are stored in distinct tables. `span_breakpoints` connects each range to its endpoint records; existing saved spans are backfilled transactionally without removing their content. `span_messages` explicitly couples spans to their included transcript messages, without making transcript lifetime depend on spans. Native activity payloads are stored independently in `activity_events`, ordered by timestamp.

## Development and hooks

```sh
brew install sqldef/sqldef/sqlite3def
./scripts/install-hooks.sh
./scripts/check.sh
```

Tracked `hooks/pre-commit` runs formatting checks, a release build of the GPUI app, the legacy package's automated tests, and the GPUI core/storage, reader, and headless CLI tests. `.envrc` configures `core.hooksPath=hooks` when enabled with `direnv allow`; direnv is optional. `.git/hooks` itself cannot be tracked. New clones need either the installer or direnv because Git intentionally does not automatically enable repository-supplied hooks.

Background build, state, storage, protocol, and fixture-audio tests do not require desktop automation. Interactive desktop Cucumber tests take focus and move the pointer; keep them paused while using the computer. They require a logged-in macOS desktop, Accessibility/Screen Recording permissions for agent-desktop, Node/npm, and the installed local Nemotron runtime/model described in the legacy documentation. Close other `team-up-gpui-spike` processes first.

```sh
./scripts/test-desktop.sh
# Or include those checks in the pre-commit gate:
WORKLOG_DESKTOP_TESTS=1 ./scripts/check.sh
```

Desktop tests use isolated fixture databases and preserve evidence under the ignored `spikes/gpui-recorder/evidence/` directory. They use the existing agent-desktop driver, without an LLM in the test loop. The current focused scenarios cover the centered driver timeline, sidebar defaults, independent notes/breakpoints, and real streaming ASR draft/final persistence. The feature file also retains older flows that need updating as the prototype changes; do not treat a focused run as full-suite coverage. Real live-device tests in the legacy package remain explicitly ignored unless individually invoked.

## Licensing

Worklog source is MIT licensed. GPUI Kit is Apache-2.0. SQLdef's main code is MIT and its parser is Apache-2.0; SQLdef is installed separately by Homebrew. Preserve upstream notices if redistributing it. Third-party Rust dependencies retain their licenses; see Cargo.lock and [third-party notices](THIRD_PARTY_NOTICES.md). Model/runtime downloads are separate and governed by their own licenses.
