# Design choices

Decisions about szpont that are not features in themselves: scope, approach, rules and trade-offs. Add a row when a decision
is made, and change its status when it is reversed. Features are in [FEATURES.md](FEATURES.md); deferred ideas are in
[FUTURE_IDEAS.md](FUTURE_IDEAS.md).

Dates before 2026-10-29 are taken from the commit that carries the decision and can be a day off. "Not recorded" means the
reason was not written down.

## Scope

| Date | Decision | Why | Not chosen |
| --- | --- | --- | --- |
| 2026-08-10 | szpont reads other tools' stores and does not write to them. The exceptions go through the tool where it can: `codex delete`, `codex unarchive`; Claude Code and Kimi have no delete command, so their session files are removed | A tool may be appending to a session at the same moment; Codex keeps a live WAL database | Writing titles or archive flags into the tools' stores |
| 2026-08-13 | Renames are szpont-side titles only (`sessions_meta`) | See the rule above; propagating them is in [FUTURE_IDEAS.md](FUTURE_IDEAS.md) | Writing the title into each tool |
| 2026-08-13 | Agent integration is tool-agnostic: the MCP initialize result carries the instructions, and `report-start` works from any harness that can run a command | Claude Code, Codex and Kimi must all work without per-user setup | A Claude Code-only skill |
| 2026-08-13 | Marking a session completed over MCP is user-triggered only; an exact `session_id` is preferred over the newest-session-in-cwd fallback | A session archived early disappears from the list the user watches; the fallback can pick the wrong session | Letting agents archive when work looks finished |
| 2026-08-18 | Local scope does not need a git repository; a plain directory is scoped by path | Umbrella directories hold several repositories | Git-only scope |
| 2026-08-18 | `--global` picks only the first screen; it does not drop the local scope | `p` must still work after `--global` | `--global` discarding the scope |

## Launching

| Date | Decision | Why | Not chosen |
| --- | --- | --- | --- |
| 2026-08-11 | Inside tmux, a launch opens a new tmux window (`tmux new-window -c <cwd>`); if that fails, szpont runs the tool in its own window | szpont stays open in its window | Always suspending szpont |
| 2026-10-29 | `Enter` and new sessions replace the szpont process (outside tmux), so leaving the tool returns to the shell. `E` runs the tool in this window and returns to szpont. This swaps the two keys | Not recorded | `Enter` suspending szpont and `E` replacing it |
| 2026-10-29 | `codex delete` runs with `--force` | szpont runs it without a terminal, so Codex cannot ask for confirmation; szpont asks first | Prompting through Codex |
| 2026-10-29 | The session directory is the last `cwd` in the Claude transcript, and a stored directory that no longer exists is replaced | A session that moves to another directory showed the old one, also after the old one was deleted | Keeping the first `cwd` |

## Storage and concurrency

| Date | Decision | Why | Not chosen |
| --- | --- | --- | --- |
| 2026-08-11 | One SQLite database for every szpont process; scan checkpoints use `BEGIN IMMEDIATE` and commit per session; busy timeout 10 s | Several TUIs and MCP servers run at once; deferred read-to-write upgrades failed with `SQLITE_BUSY` | Deferred transactions |
| 2026-08-13 | The limits cache is shared in the database (3 minutes for Claude) and records the szpont version that wrote it | An older binary refreshed the cache with its own parser and a newer one lost the model-scoped windows | A cache without a version |
| 2026-08-31 | An existing database file is opened only by SQLite; szpont creates the file with `create_new` and sets permissions after SQLite opens it | A plain `close()` on POSIX cancels every advisory lock the process holds on the file ("How To Corrupt An SQLite Database File", section 2.2) | Opening the file to set permissions first |

## Screen and input

| Date | Decision | Why | Not chosen |
| --- | --- | --- | --- |
| 2026-08-13 | The kitty keyboard protocol is enabled when the terminal supports it | Cmd/Ctrl+Backspace must be told apart from Backspace | Plain key events only |
| 2026-08-13 | Limits have their own header row | Not recorded | Limits in the status line |
| 2026-08-20 | The footer shows every key the screen binds; a test checks that each bound key is advertised | Keys were clipped or missing from the footer | A short hint list |
| 2026-08-20 | Ctrl, Alt and Cmd combinations do not act as the plain letter outside text inputs | `Ctrl+D` opened the delete prompt | Ignoring modifiers |

## Release and docs

| Date | Decision | Why | Not chosen |
| --- | --- | --- | --- |
| 2026-08-11 | Releases use cargo-dist (GitHub Release, shell installer with `szpont-update`, Homebrew tap) and publish to crates.io; see [RELEASING.md](RELEASING.md) | Several install channels from one tag | Manual builds |
| 2026-08-11 | The docs site is a Next.js static export in `docs/`, deployed to Cloudflare Pages | — | — |
| 2026-10-29 | `FEATURES.md`, `DECISIONS.md` and `FUTURE_IDEAS.md` at the repository root, in the format of the structured-agent project; `TODO.md` became `FUTURE_IDEAS.md` | `docs/` is the website | Files in `docs/` |
| 2026-10-29 | The docs pages are formatted with Prettier, pinned to an exact version (3.9.10) in `docs/package.json`, with default options. `pnpm --dir docs format` formats; the docs workflow runs `format:check` before it deploys | `npx prettier` used whatever version was on the machine, and the pages had drifted from its style | Unpinned `npx prettier`; no check |
