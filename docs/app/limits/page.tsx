import type { Metadata } from "next";

export const metadata: Metadata = { title: "Limits — szpont docs" };

export default function Limits() {
  return (
    <>
      <h1>Limits</h1>
      <ul>
        <li>
          <strong>Codex</strong>: exact values (<code>used_percent</code>,
          window, reset time, plan) parsed from <code>rate_limits</code> events
          in the newest rollout file.
        </li>
        <li>
          <strong>Claude Code</strong>: token totals per rolling 5h / 7d window
          computed from local logs. Percentages are relative to the largest
          window ever observed and are labeled <code>est</code> in the limits
          popup (<code>~</code> in the top bar). The exact utilization comes
          from the OAuth usage endpoint — every window it reports is shown (5h,
          week, per-model weeks, extra usage).
        </li>
        <li>
          <strong>Kimi Code</strong>: no limit data on disk; the quota cycle and
          every windowed limit (e.g. the 5h window) come from Kimi&apos;s{" "}
          <code>/usages</code> endpoint, with the membership level as the plan.
        </li>
      </ul>
      <p>
        The online endpoints are unofficial and degrade silently: when they are
        unreachable, szpont falls back to what is on disk.
      </p>
    </>
  );
}
