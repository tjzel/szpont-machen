import type { Metadata } from "next";

export const metadata: Metadata = { title: "TUI keys — szpont machen docs" };

export default function Keys() {
  return (
    <>
      <h1>TUI keys</h1>

      <table>
        <tbody>
          <tr>
            <th>Key</th>
            <th>Action</th>
          </tr>
          <tr>
            <td>
              <code>↑</code> / <code>↓</code>
            </td>
            <td>move selection</td>
          </tr>
          <tr>
            <td>
              <code>Home</code> / <code>End</code> / <code>G</code>
            </td>
            <td>jump to the first / last row</td>
          </tr>
          <tr>
            <td>
              <code>Enter</code>
            </td>
            <td>
              resume the selected session (szpont steps aside; inside tmux the
              session opens in a new window)
            </td>
          </tr>
          <tr>
            <td>
              <code>E</code>
            </td>
            <td>resume in this window and come back to szpont on exit</td>
          </tr>
          <tr>
            <td>
              <code>n</code>
            </td>
            <td>
              new session (tool picker; <code>1</code>–<code>9</code> picks a
              tool directly)
            </td>
          </tr>
          <tr>
            <td>
              <code>c</code>
            </td>
            <td>
              mark as completed: enters marking mode on the selected session;
              keep marking with <code>c</code>, move with <code>↑</code> /{" "}
              <code>↓</code>, confirm with <code>Enter</code>, cancel with{" "}
              <code>Esc</code> or <code>q</code>
            </td>
          </tr>
          <tr>
            <td>
              <code>u</code>
            </td>
            <td>reopen (archive screen)</td>
          </tr>
          <tr>
            <td>
              <code>d</code>
            </td>
            <td>
              mark for deletion (works on every session screen, including the
              live list): same marking flow with <code>d</code> /{" "}
              <code>Enter</code> / <code>Esc</code> or <code>q</code>. Deletion
              removes the session from szpont <strong>and</strong> from the
              tool&apos;s own storage (<code>codex delete</code>; file removal
              for Claude Code and Kimi, which have no delete command).
              Sessions still open in a tool are skipped.
            </td>
          </tr>
          <tr>
            <td>
              <code>v</code>
            </td>
            <td>
              tree view — sessions nested by directory with branch lines; works
              from monitor, repo scope and archive; <code>v</code> or{" "}
              <code>Esc</code> returns
            </td>
          </tr>
          <tr>
            <td>
              <code>p</code>
            </td>
            <td>
              toggle scope — global (all sessions) vs local (the current git
              repository, or the current directory when it is not in one; the
              project instead when the directory belongs to one)
            </td>
          </tr>
          <tr>
            <td>
              <code>o</code>
            </td>
            <td>
              options screen — set the context window per model (e.g.{" "}
              <code>200K</code>, <code>1M</code>); persisted, used for the CTX
              bar when the tool does not report a window (Claude Code)
            </td>
          </tr>
          <tr>
            <td>
              <code>a</code>
            </td>
            <td>archive screen</td>
          </tr>
          <tr>
            <td>
              <code>m</code>
            </td>
            <td>
              rename the selected session (szpont-side custom title; survives
              rescans)
            </td>
          </tr>
          <tr>
            <td>
              <code>s</code>
            </td>
            <td>
              assign the selected session&apos;s directory to a project (shown
              in the <code>PROJECT</code> column); several directories can share
              one project; an empty name clears it
            </td>
          </tr>
          <tr>
            <td>
              <code>Esc</code>
            </td>
            <td>
              back from archive or tree view · clear marks · clear filters —
              one step per press
            </td>
          </tr>
          <tr>
            <td>
              <code>l</code>
            </td>
            <td>limit usage popup (any key closes it)</td>
          </tr>
          <tr>
            <td>
              <code>i</code>
            </td>
            <td>session detail popup (any key closes it)</td>
          </tr>
          <tr>
            <td>
              <code>f</code>
            </td>
            <td>
              find (fzf-style fuzzy search: subsequence match over title,
              prompt, path and tool; filters live while typing, best matches
              first; works on every session view)
            </td>
          </tr>
          <tr>
            <td>
              <code>Tab</code>
            </td>
            <td>
              cycle tool filter (<code>Shift+Tab</code> goes in reverse)
            </td>
          </tr>
          <tr>
            <td>
              <code>r</code>
            </td>
            <td>
              force rescan (min 3s between scans; idle tick every 15s; file
              changes trigger scans automatically)
            </td>
          </tr>
          <tr>
            <td>
              double <code>Ctrl+C</code>
            </td>
            <td>quit</td>
          </tr>
        </tbody>
      </table>

      <h2>Reading the display</h2>
      <p>The STATUS column shows:</p>
      <table>
        <tbody>
          <tr>
            <th>Status</th>
            <th>Meaning</th>
          </tr>
          <tr>
            <td>
              <code>RUNNING</code> (green)
            </td>
            <td>the tool is working</td>
          </tr>
          <tr>
            <td>
              <code>BLOCKED</code> (yellow)
            </td>
            <td>
              waiting for your input (reported for Claude Code only)
            </td>
          </tr>
          <tr>
            <td>
              <code>IDLE</code> (cyan)
            </td>
            <td>
              open at the prompt — for Codex and Kimi this means a tool process
              has the session&apos;s directory open
            </td>
          </tr>
          <tr>
            <td>(empty)</td>
            <td>session not loaded in any process</td>
          </tr>
        </tbody>
      </table>
      <p>
        The list always sits in a tinted frame that names the mode: neutral
        &quot; sessions &quot;, magenta &quot; archive &quot;, yellow
        &quot; marking as completed &quot;, red &quot; marking for deletion
        &quot;, green &quot; new session &quot;, cyan &quot; options &quot;.
      </p>
      <p>
        Background rescans never reorder the list under you: updates that keep
        the visible row order (token counts, status changes) apply live, while
        updates that would add, remove or reorder rows are held — the top bar
        shows &quot;⟳ list changed — press r to refresh&quot;. <code>r</code>{" "}
        applies the held update instantly and rescans; your own actions
        (resume, complete, delete) always apply immediately too.
      </p>
      <p>
        The <code>CTX</code> column shows context-window usage as{" "}
        <code>used/window</code> token counts with a fill bar when the window
        size is known (Codex reports it in its rollout; Kimi&apos;s comes from{" "}
        <code>~/.kimi-code/config.toml</code>), otherwise the raw token count
        of the last request (Claude Code does not record its window size
        locally — set it once per model in the <code>o</code> options screen).
      </p>
    </>
  );
}
