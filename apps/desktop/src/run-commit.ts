import { escapeCatalog as escape } from "./agent-settings.js";
import type { DesktopRun } from "./types.js";

/** Render the Run's Git receipt independently of immutable conversation messages. */
export function renderRunCommit(run: DesktopRun, allowRetry = true): string {
  const commit = run.gitCommit;
  if (!commit) return "";
  const label = {
    pending: "Preparing Git commit",
    prepared: "Saving Git commit",
    committed: "Git commit saved",
    skipped: "Git commit skipped",
    failed: "Git commit failed",
  }[commit.status];
  const retry = allowRetry && commit.status === "failed" && run.status === "completed";
  return `<section class="run-git-commit" aria-label="Git commit">
    <strong>${escape(label)}</strong>
    ${commit.commitId ? `<code>${escape(commit.commitId)}</code>` : ""}
    ${commit.reason ? `<p>${escape(commit.reason)}</p>` : ""}
    ${retry ? `<button class="secondary-button" type="button" data-retry-commit="${escape(run.id)}">Retry Git commit</button>` : ""}
  </section>`;
}
