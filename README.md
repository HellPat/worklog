# Worklog

A native macOS GPUI prototype for a daily speaking timeline, transcript comments, and labeled time spans. Blue is Me, yellow is Them, lime is overlap; empty time remains visible. The footer provides recording simulation, Start/End span, and Label/Save span. Subscreens have Back, and refinement keeps the timeline visible beside the transcript workflow.

**Prototype limits:** recording is simulated in the GPUI app. Real local ASR can prepare an audio fixture for the desktop tests. The activity timeline is synthetic and does not derive from that WAV. Active spans are in memory until ended. The older live-capture Team-up app is preserved in the root Rust package; see [its documentation](packaging/LEGACY-TEAM-UP.md).

## Homebrew

```sh
brew tap hellpat/worklog https://github.com/HellPat/worklog
brew install hellpat/worklog/worklog
worklog
```

This is a custom tap in the source repository, so its explicit URL is required. Homebrew installs `sqldef/sqldef/sqlite3def` as a runtime dependency and builds the GPUI app from the pinned source revision. No Go toolchain is required by users. The initial distribution is a command that opens the native app, not a notarized `.app` bundle. Apple silicon macOS is the currently validated platform.

The default database is `~/Library/Application Support/worklog/worklog.sqlite`. Set `TEAM_UP_SPIKE_DB` for an isolated database. Existing spike fixture databases are not moved or removed.

## Schema and storage

`spikes/gpui-recorder/schema.sql` is the desired schema, embedded in the executable. On opening a database, `sqlite3def` plans the difference; Rust applies it transactionally and checks foreign keys before committing. Destructive changes are not enabled. Planning failures stop startup with an error. `WORKLOG_SQLITE3DEF` can specify the tool; packaged builds compile in Homebrew's stable dependency path so launching outside a shell also works.

Transcript messages, spans, and replies are stored in distinct tables. `span_messages` explicitly couples spans to their included transcript messages, without making transcript lifetime depend on spans.

## Development and hooks

```sh
brew install sqldef/sqldef/sqlite3def
./scripts/install-hooks.sh
./scripts/check.sh
```

Tracked `hooks/pre-commit` runs formatting checks, a release build of the GPUI app, the legacy package's automated tests, and the GPUI core/storage tests. `.envrc` configures `core.hooksPath=hooks` when enabled with `direnv allow`; direnv is optional. `.git/hooks` itself cannot be tracked. New clones need either the installer or direnv because Git intentionally does not automatically enable repository-supplied hooks.

Desktop Cucumber tests require a logged-in macOS desktop, Accessibility/Screen Recording permissions for agent-desktop, Node/npm, and the installed local Nemotron runtime/model described in the legacy documentation. Close other `team-up-gpui-spike` processes first.

```sh
./scripts/test-desktop.sh
# Or include those checks in the pre-commit gate:
WORKLOG_DESKTOP_TESTS=1 ./scripts/check.sh
```

Desktop tests use isolated fixture databases and preserve evidence under the ignored `spikes/gpui-recorder/evidence/` directory. They use the existing agent-desktop driver, without an LLM in the test loop. Tests exercise the simulation, navigation, exact message boundaries, comments, optional span details, and local-ASR fixture import and persistence. Real live-device tests in the legacy package remain explicitly ignored unless individually invoked.

## Licensing

Worklog source is MIT licensed. GPUI Kit is Apache-2.0. SQLdef's main code is MIT and its parser is Apache-2.0; SQLdef is installed separately by Homebrew. Preserve upstream notices if redistributing it. Third-party Rust dependencies retain their licenses; see Cargo.lock and [third-party notices](THIRD_PARTY_NOTICES.md). Model/runtime downloads are separate and governed by their own licenses.
