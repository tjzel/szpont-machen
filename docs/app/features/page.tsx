import type { Metadata } from "next";

export const metadata: Metadata = { title: "Features — szpont machen docs" };

export default function Features() {
  return (
    <>
      <h1>Features</h1>

      <h2>Session tracking</h2>
      <ul>
        <li>
          Tracks sessions of <strong>Claude Code</strong>,{" "}
          <strong>Codex CLI</strong> and <strong>Kimi Code</strong> in one
          list, scanned read-only from each tool&apos;s native store.
        </li>
        <li>
          Live status per session: <code>RUNNING</code>, <code>BLOCKED</code>,{" "}
          <code>IDLE</code>.
        </li>
        <li>
          Resume any session with the right tool via <code>Enter</code> —
          szpont steps aside, so leaving the tool drops you back at the shell.
        </li>
        <li>
          Incremental parsing: after the first scan, refreshes read only
          appended data.
        </li>
      </ul>

      <h2>Usage and limits</h2>
      <ul>
        <li>Token usage per session, subagents included.</li>
        <li>
          Context-window usage per session (<code>used/window</code> with a
          fill bar in the <code>CTX</code> column when the window size is
          known).
        </li>
        <li>
          Rate-limit state per tool, with exact values from the
          providers&apos; usage endpoints.
        </li>
      </ul>

      <h2>Organization</h2>
      <ul>
        <li>Archive completed sessions; reopen them anytime.</li>
        <li>Custom session titles that survive rescans.</li>
        <li>
          Deletion that removes the session from szpont and the tool&apos;s own
          storage.
        </li>
        <li>
          Tree view, fuzzy find, tool filters, and repo/global scope toggle.
        </li>
      </ul>

      <h2>Integration</h2>
      <ul>
        <li>
          Headless CLI with <code>--json</code> output for scripting.
        </li>
        <li>Shell completions for bash, zsh, fish and more.</li>
        <li>
          MCP server so agents can report and complete sessions themselves.
        </li>
      </ul>
    </>
  );
}
