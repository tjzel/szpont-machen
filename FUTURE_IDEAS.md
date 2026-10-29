# Future ideas

Ideas that are deferred on purpose. They are not current work. Move an idea into a todo only when it is picked up.
Features are in [FEATURES.md](FEATURES.md); decisions are in [DECISIONS.md](DECISIONS.md).

## Propagate renames to the tools themselves (2026-08-13)

Today `m` sets a szpont-local title in `sessions_meta`; the tool's own session name is
untouched. Each tool does store a writable title:

- Claude — appends `{"type":"custom-title","customTitle":"…","sessionId":"…"}` to the session
  JSONL. szpont already parses this event, so a rename means appending one line.
- Codex — `threads.title` (and a separate `name` column) in `~/.codex/state_*.sqlite`.
- Kimi — `title` + `isCustomTitle` in `state.json`.

Blocked on the rule that szpont only reads other tools' stores. Writing to a session the tool
is actively appending to is the risky case, Codex most of all (live WAL database owned by
another process). If this is built, keep it Claude-only at first and expose it as a separate
action so a szpont label stays distinguishable from a real rename.
