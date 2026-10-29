import type { Metadata } from "next";

export const metadata: Metadata = { title: "MCP — szpont docs" };

export default function Mcp() {
  return (
    <>
      <h1>MCP integration</h1>
      <p>
        <code>szpont install-mcp</code> registers <code>szpont mcp</code> with
        all three tools (<code>claude mcp add</code>, <code>codex mcp add</code>
        , <code>~/.kimi-code/mcp.json</code>). Agents can then call:
      </p>
      <table>
        <tbody>
          <tr>
            <th>Tool</th>
            <th>Purpose</th>
          </tr>
          <tr>
            <td>
              <code>report_session_start</code>
            </td>
            <td>track a session as soon as it starts</td>
          </tr>
          <tr>
            <td>
              <code>mark_session_completed</code>
            </td>
            <td>
              move a finished session to the archive (works with just{" "}
              <code>cwd</code> if the agent does not know its session id)
            </td>
          </tr>
          <tr>
            <td>
              <code>reopen_session</code>
            </td>
            <td>undo a completed mark</td>
          </tr>
        </tbody>
      </table>
      <p>
        There is deliberately no <code>list_sessions</code> tool: connected
        agents can report and complete sessions, but they cannot enumerate
        sessions from other repos (titles and paths of unrelated private work
        stay private).
      </p>
      <p>
        The MCP server writes to the shared database; the TUI picks changes up
        on its next refresh tick (15s default, <code>--refresh-secs</code>).
        Every call is recorded in a local audit table (<code>mcp_events</code>,
        arguments JSON capped at 4KB, newest 1000 rows kept).
      </p>
      <p>
        Install details: Claude Code is registered with{" "}
        <code>--scope user</code>; for Kimi the entry is written to{" "}
        <code>~/.kimi-code/mcp.json</code> and a previous file is kept at{" "}
        <code>mcp.json.bak</code>.
      </p>
    </>
  );
}
