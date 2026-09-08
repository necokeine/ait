import type { DesktopRun, NativeApproval } from "./types.js";

export type ApprovalAction = "approve" | "deny" | "cancel";
export type ApprovalScope = "one_shot" | "turn" | "session";

export function approvalAction(value: unknown): ApprovalAction {
  if (value === "approve" || value === "deny" || value === "cancel") return value;
  throw new Error("Approval action is invalid.");
}

export function approvalScope(value: unknown, action: ApprovalAction): ApprovalScope | undefined {
  if (action !== "approve") {
    if (value !== undefined) throw new Error("Only approval may carry an authorization scope.");
    return undefined;
  }
  if (value === "one_shot" || value === "turn" || value === "session") return value;
  throw new Error("Approval scope is invalid.");
}

export function isApprovalEvent(kind: string): boolean {
  return kind === "run.approval_requested"
    || kind === "run.approval_resolved"
    || kind === "run.approval_expired";
}

export function renderPendingApprovals(run: DesktopRun | undefined): string {
  if (!run) return "";
  return run.nativeApprovals
    .filter((approval) => approval.status === "pending")
    .map((approval) => renderApproval(run, approval))
    .join("");
}

function renderApproval(run: DesktopRun, approval: NativeApproval): string {
  const permissionDetail = approval.requestedPermissions
    ? `<pre>${escapeHtml(JSON.stringify(approval.requestedPermissions, null, 2))}</pre>`
    : "";
  const limitedScope = approval.kind === "permissions" ? "turn" : "one_shot";
  const limitedLabel = approval.kind === "permissions" ? "Allow for turn" : "Allow once";
  return `<section class="native-approval" data-run-id="${escapeAttribute(run.id)}" data-approval-id="${escapeAttribute(approval.id)}">
    <header><strong>${escapeHtml(kindLabel(approval.kind))}</strong><span>Approval required</span></header>
    <p>Codex requires an explicit decision before continuing. Provider arguments are not persisted here.</p>${permissionDetail}
    <dl>
      <div><dt>Thread</dt><dd><code>${escapeHtml(approval.threadId)}</code></dd></div>
      <div><dt>Turn</dt><dd><code>${escapeHtml(approval.turnId)}</code></dd></div>
      <div><dt>Request</dt><dd><code>${escapeHtml(String(approval.protocolRequestId))}</code></dd></div>
    </dl>
    <footer>
      <button type="button" data-approval-action="cancel">Cancel turn</button>
      <button type="button" data-approval-action="deny">Deny</button>
      <button type="button" data-approval-action="approve" data-approval-scope="${limitedScope}">${limitedLabel}</button>
      <button type="button" class="primary" data-approval-action="approve" data-approval-scope="session">Allow for session</button>
    </footer>
  </section>`;
}

function kindLabel(kind: NativeApproval["kind"]): string {
  switch (kind) {
    case "command_execution": return "Run command";
    case "file_change": return "Change files";
    case "permissions": return "Extend permissions";
    case "legacy_command": return "Run legacy command";
    case "legacy_patch": return "Apply legacy patch";
  }
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (character) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", "\"": "&quot;", "'": "&#39;",
  })[character]!);
}

function escapeAttribute(value: string): string {
  return escapeHtml(value);
}
