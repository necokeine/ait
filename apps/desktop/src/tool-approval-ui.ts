import type { DesktopRun } from "./types.js";
import { escapeCatalog as escape } from "./agent-settings.js";

const permissions = { read_only: "Readonly", workspace_write: "Workspace Write", full_access: "Full Access" };
export function renderToolApprovals(run: DesktopRun): string {
  return (run.toolApprovals ?? []).map(({ grant, status }) => {
    const target = grant.target;
    const live = status === "pending" && grant.expires_at > Date.now()
      && ["running", "waiting_approval"].includes(run.status);
    const visibleStatus = status === "pending" && !live ? "expired" : status;
    if (!target?.operation || !target.cwd || !permissions[target.requested]) {
      return '<section class="native-approval"><strong>Approval unavailable</strong><p>This request has no reviewable target.</p></section>';
    }
    const scope = target.tool_name === "bash"
      ? target.requested === "full_access" ? "This command and its child processes can access host files and the network without an OS sandbox."
        : "This command and its child processes can write in this workspace. The OS sandbox still restricts external files and network access."
      : "Only this file operation, within the workspace. File tools cannot follow symlinks or access hidden paths.";
    return `<section class="native-approval tool-approval" data-tool-approval="true" data-tool-expiry="${grant.expires_at}" data-run-id="${escape(run.id)}" data-approval-id="${escape(grant.request_id)}">
      <header><strong>${escape(target.tool_name)} · Permission request</strong><span class="tool-approval-status">${escape(visibleStatus)}</span></header>
      <p>${escape(run.providerName ?? "API Provider")} · ${escape(run.agentName ?? "Agent")}</p>
      <dl class="approval-target"><div><dt>${target.tool_name === "bash" ? "Command" : "Target"}</dt><dd><pre>${escape(target.operation)}</pre></dd></div>
      <div><dt>Working directory</dt><dd><code>${escape(target.cwd)}</code></dd></div>
      <div><dt>Reason</dt><dd>${escape(target.reason)}</dd></div>
      <div><dt>Access</dt><dd>${escape(permissions[target.current] ?? target.current)} → ${escape(permissions[target.requested])}</dd></div>
      <div><dt>Authorization scope</dt><dd>This operation only. ${scope}</dd></div>
      <div><dt>Expires</dt><dd><time datetime="${new Date(grant.expires_at).toISOString()}">${escape(new Date(grant.expires_at).toLocaleTimeString())}</time> · Waiting counts toward the Run timeout.</dd></div>
      <div><dt>Call</dt><dd><code>${escape(grant.call_id)}</code></dd></div></dl>
      ${live ? '<footer><button type="button" data-approval-action="cancel">Cancel Run</button><button type="button" data-approval-action="deny">Deny</button><button type="button" class="primary" data-approval-action="approve">Allow once</button></footer>' : ""}
    </section>`;
  }).join("");
}

/** Expiry remains visible even while the event connection is unavailable. */
export function expireToolApprovalCards(root: ParentNode, now = Date.now()): void {
  for (const card of root.querySelectorAll<HTMLElement>("[data-tool-expiry]")) {
    const status = card.querySelector(".tool-approval-status");
    if (Number(card.dataset.toolExpiry) <= now && status?.textContent === "pending") {
      status.textContent = "expired";
      card.querySelector("footer")?.remove();
    }
  }
}
