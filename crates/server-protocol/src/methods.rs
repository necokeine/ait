//! Canonical WebSocket method names and their Paseo source names.
//!
//! Source names are retained for review and fixture generation only. The independent server
//! accepts canonical names on the wire and does not silently alias legacy names.

/// Functional area that owns a WebSocket operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodGroup {
    /// Daemon lifecycle, configuration, and diagnostics.
    Daemon,
    /// Project records and project-scoped configuration.
    Project,
    /// Workspace records, labels, setup, and worktrees.
    Workspace,
    /// Agent creation, lifecycle, configuration, and timelines.
    Agent,
    /// Provider discovery and diagnostics.
    Provider,
    /// Agent skill selection and installation state.
    Skills,
    /// Git checkout, branch, stash, forge, and pull-request operations.
    Git,
    /// Filesystem browsing and mutation.
    Files,
    /// Terminal creation, streams, and workspace scripts.
    Terminal,
    /// Legacy desktop editor compatibility.
    Editor,
    /// Scheduled work.
    Schedule,
    /// Voice audio and dictation streams.
    Voice,
    /// Connection subscriptions, creation subscriptions, and heartbeats.
    Session,
    /// Push notification token registration.
    Push,
    /// Browser host registration and execution callbacks.
    Browser,
}

/// Direction and correlation behavior of an inbound method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundKind {
    /// A correlated request that receives a response or protocol error.
    Request,
    /// An uncorrelated client event.
    Event,
    /// A client response to work initiated by the server.
    Response,
}

/// One requested Paseo operation and its canonical independent-server name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodSpec {
    /// Name in the pinned Paseo protocol.
    pub paseo_name: &'static str,
    /// Name accepted by the independent server.
    pub canonical_name: &'static str,
    /// Owning functional area.
    pub group: MethodGroup,
    /// Inbound message behavior.
    pub kind: InboundKind,
}

macro_rules! request {
    ($group:ident, $source:literal, $canonical:expr) => {
        MethodSpec {
            paseo_name: $source,
            canonical_name: $canonical,
            group: MethodGroup::$group,
            kind: InboundKind::Request,
        }
    };
}

macro_rules! event {
    ($group:ident, $source:literal, $canonical:expr) => {
        MethodSpec {
            paseo_name: $source,
            canonical_name: $canonical,
            group: MethodGroup::$group,
            kind: InboundKind::Event,
        }
    };
}

macro_rules! response {
    ($group:ident, $source:literal, $canonical:expr) => {
        MethodSpec {
            paseo_name: $source,
            canonical_name: $canonical,
            group: MethodGroup::$group,
            kind: InboundKind::Response,
        }
    };
}

/// Operations requested for the independent server, including merged legacy Paseo aliases.
pub const PASEO_METHODS: &[MethodSpec] = &[
    request!(
        Daemon,
        "daemon.get_status.request",
        "daemon.get_status.request"
    ),
    request!(
        Daemon,
        "daemon.get_pairing_offer.request",
        "daemon.get_pairing_offer.request"
    ),
    request!(
        Daemon,
        "daemon.config.reload.request",
        "daemon.config.reload.request"
    ),
    request!(Daemon, "daemon.update.request", "daemon.update.request"),
    request!(Daemon, "diagnostics.request", "diagnostics.request"),
    request!(
        Daemon,
        "get_daemon_config_request",
        "daemon.config.get.request"
    ),
    request!(
        Daemon,
        "set_daemon_config_request",
        "daemon.config.set.request"
    ),
    request!(Daemon, "restart_server_request", "server.restart.request"),
    request!(Daemon, "shutdown_server_request", "server.shutdown.request"),
    request!(Project, "project.list.request", "project.list.request"),
    request!(Project, "project.add.request", "project.add.request"),
    request!(
        Project,
        "project.create_directory.request",
        "project.create_directory.request"
    ),
    request!(
        Project,
        "project.github.clone.request",
        "project.github.clone.request"
    ),
    request!(Project, "project.rename.request", "project.rename.request"),
    request!(Project, "project.remove.request", "project.remove.request"),
    request!(
        Project,
        "project.icon.set.request",
        "project.icon.set.request"
    ),
    request!(
        Project,
        "project.icon.get.request",
        "project.icon.get.request"
    ),
    request!(Project, "project_icon_request", "project.icon.get.request"),
    request!(
        Project,
        "read_project_config_request",
        "project.config.read.request"
    ),
    request!(
        Project,
        "write_project_config_request",
        "project.config.write.request"
    ),
    request!(
        Workspace,
        "fetch_workspaces_request",
        "workspace.list.request"
    ),
    request!(Workspace, "open_project_request", "workspace.open.request"),
    request!(
        Workspace,
        "workspace.create.request",
        "workspace.create.request"
    ),
    request!(
        Workspace,
        "archive_workspace_request",
        "workspace.archive.request"
    ),
    request!(
        Workspace,
        "workspace.title.set.request",
        "workspace.title.set.request"
    ),
    request!(
        Workspace,
        "workspace.pin.set.request",
        "workspace.pin.set.request"
    ),
    request!(
        Workspace,
        "workspace.clear_attention.request",
        "workspace.clear_attention.request"
    ),
    request!(
        Workspace,
        "workspace.mark_unread.request",
        "workspace.mark_unread.request"
    ),
    request!(
        Workspace,
        "workspace.recovery.inspect.request",
        "workspace.recovery.inspect.request"
    ),
    request!(
        Workspace,
        "workspace.recovery.restore.request",
        "workspace.recovery.restore.request"
    ),
    request!(
        Workspace,
        "workspace.github.search_repositories.request",
        "workspace.github.search_repositories.request"
    ),
    request!(
        Workspace,
        "workspace.label.list.request",
        "workspace.label.list.request"
    ),
    request!(
        Workspace,
        "workspace.label.assignment.set.request",
        "workspace.label.assignment.set.request"
    ),
    request!(
        Workspace,
        "workspace.label.update.request",
        "workspace.label.update.request"
    ),
    request!(
        Workspace,
        "workspace.label.delete.inspect.request",
        "workspace.label.delete.inspect.request"
    ),
    request!(
        Workspace,
        "workspace.label.delete.request",
        "workspace.label.delete.request"
    ),
    request!(
        Workspace,
        "paseo_worktree_list_request",
        "workspace.worktree.list.request"
    ),
    request!(
        Workspace,
        "create_paseo_worktree_request",
        "workspace.worktree.create.request"
    ),
    request!(
        Workspace,
        "paseo_worktree_archive_request",
        "workspace.worktree.archive.request"
    ),
    request!(
        Workspace,
        "workspace_setup_status_request",
        "workspace.setup.status.request"
    ),
    request!(
        Workspace,
        "workspace.setup.run.request",
        "workspace.setup.run.request"
    ),
    request!(
        Workspace,
        "workspace.script.list.request",
        "workspace.script.list.request"
    ),
    request!(
        Workspace,
        "workspace.script.start.request",
        "workspace.script.start.request"
    ),
    request!(
        Workspace,
        "workspace.script.stop.request",
        "workspace.script.stop.request"
    ),
    request!(
        Workspace,
        "start_workspace_script_request",
        "workspace.script.start.request"
    ),
    request!(Agent, "fetch_agents_request", "agent.list.request"),
    request!(Agent, "fetch_agent_request", "agent.get.request"),
    request!(
        Agent,
        "fetch_agent_history_request",
        "agent.history.get.request"
    ),
    request!(Agent, "agent.create.request", "agent.create.request"),
    request!(Agent, "create_agent_request", "agent.create.request"),
    request!(Agent, "resume_agent_request", "agent.resume.request"),
    request!(Agent, "import_agent_request", "agent.import.request"),
    request!(Agent, "refresh_agent_request", "agent.refresh.request"),
    request!(Agent, "update_agent_request", "agent.update.request"),
    request!(
        Agent,
        "send_agent_message_request",
        "agent.message.send.request"
    ),
    request!(
        Agent,
        "wait_for_finish_request",
        "agent.finish.wait.request"
    ),
    request!(Agent, "cancel_agent_request", "agent.cancel.request"),
    request!(Agent, "archive_agent_request", "agent.archive.request"),
    request!(Agent, "delete_agent_request", "agent.delete.request"),
    request!(Agent, "agent.detach.request", "agent.detach.request"),
    request!(Agent, "agent.rewind.request", "agent.rewind.request"),
    request!(
        Agent,
        "clear_agent_attention",
        "agent.attention.clear.request"
    ),
    request!(Agent, "close_items_request", "agent.items.close.request"),
    request!(
        Agent,
        "list_commands_request",
        "agent.commands.list.request"
    ),
    request!(Agent, "set_agent_mode_request", "agent.mode.set.request"),
    request!(Agent, "set_agent_model_request", "agent.model.set.request"),
    request!(
        Agent,
        "set_agent_thinking_request",
        "agent.thinking.set.request"
    ),
    request!(
        Agent,
        "set_agent_feature_request",
        "agent.feature.set.request"
    ),
    request!(
        Agent,
        "agent.config.apply.request",
        "agent.config.apply.request"
    ),
    request!(
        Agent,
        "agent_permission_response",
        "agent.permission.resolve.request"
    ),
    request!(
        Agent,
        "fetch_agent_timeline_request",
        "agent.timeline.get.request"
    ),
    request!(
        Agent,
        "agent.timeline.search.request",
        "agent.timeline.search.request"
    ),
    request!(
        Agent,
        "agent.timeline.list_prompts.request",
        "agent.timeline.list_prompts.request"
    ),
    request!(
        Agent,
        "agent.timeline.append.request",
        "agent.timeline.append.request"
    ),
    request!(
        Agent,
        "agent.timeline.set_subscription.request",
        "agent.timeline.set_subscription.request"
    ),
    request!(
        Agent,
        "agent.fork_context.request",
        "agent.fork_context.request"
    ),
    request!(
        Agent,
        "agent.provider_subagents.list.request",
        "agent.provider_subagents.list.request"
    ),
    request!(
        Agent,
        "agent.provider_subagents.timeline.get.request",
        "agent.provider_subagents.timeline.get.request"
    ),
    request!(
        Provider,
        "list_available_providers_request",
        "provider.available.list.request"
    ),
    request!(
        Provider,
        "list_provider_models_request",
        "provider.models.list.request"
    ),
    request!(
        Provider,
        "list_provider_modes_request",
        "provider.modes.list.request"
    ),
    request!(
        Provider,
        "list_provider_features_request",
        "provider.features.list.request"
    ),
    request!(
        Provider,
        "get_providers_snapshot_request",
        "provider.snapshot.get.request"
    ),
    request!(
        Provider,
        "refresh_providers_snapshot_request",
        "provider.snapshot.refresh.request"
    ),
    request!(
        Provider,
        "provider_diagnostic_request",
        "provider.diagnostic.request"
    ),
    request!(
        Provider,
        "provider.usage.list.request",
        "provider.usage.list.request"
    ),
    request!(
        Provider,
        "fetch_recent_provider_sessions_request",
        "provider.sessions.recent.list.request"
    ),
    request!(
        Skills,
        "agent.skills.get_status.request",
        "agent.skills.get_status.request"
    ),
    request!(
        Skills,
        "agent.skills.reconcile.request",
        "agent.skills.reconcile.request"
    ),
    request!(
        Skills,
        "agent.skills.uninstall.request",
        "agent.skills.uninstall.request"
    ),
    request!(
        Skills,
        "agent.skills.save_selection.request",
        "agent.skills.save_selection.request"
    ),
    request!(
        Skills,
        "agent.skills.import_legacy_selection.request",
        "agent.skills.import_legacy_selection.request"
    ),
    request!(
        Git,
        "checkout_status_request",
        "checkout.status.get.request"
    ),
    request!(Git, "checkout.refresh.request", "checkout.refresh.request"),
    request!(
        Git,
        "checkout.diff.get.request",
        "checkout.diff.get.request"
    ),
    request!(
        Git,
        "subscribe_checkout_diff_request",
        "checkout.diff.subscribe.request"
    ),
    request!(
        Git,
        "unsubscribe_checkout_diff_request",
        "checkout.diff.unsubscribe.request"
    ),
    request!(
        Git,
        "checkout.commits.list.request",
        "checkout.commits.list.request"
    ),
    request!(
        Git,
        "checkout.commits.file_diff.request",
        "checkout.commits.file_diff.request"
    ),
    request!(
        Git,
        "validate_branch_request",
        "checkout.branch.validate.request"
    ),
    request!(
        Git,
        "branch_suggestions_request",
        "checkout.branch.suggestions.request"
    ),
    request!(
        Git,
        "checkout_switch_branch_request",
        "checkout.branch.switch.request"
    ),
    request!(
        Git,
        "checkout.rename_branch.request",
        "checkout.rename_branch.request"
    ),
    request!(Git, "checkout_commit_request", "checkout.commit.request"),
    request!(Git, "checkout_merge_request", "checkout.merge.request"),
    request!(
        Git,
        "checkout_merge_from_base_request",
        "checkout.merge_from_base.request"
    ),
    request!(Git, "checkout_pull_request", "checkout.pull.request"),
    request!(Git, "checkout_push_request", "checkout.push.request"),
    request!(
        Git,
        "checkout.discard_changes.request",
        "checkout.discard_changes.request"
    ),
    request!(Git, "stash_save_request", "checkout.stash.save.request"),
    request!(Git, "stash_pop_request", "checkout.stash.pop.request"),
    request!(Git, "stash_list_request", "checkout.stash.list.request"),
    request!(Git, "forge.search.request", "forge.search.request"),
    request!(Git, "github_search_request", "github.search.request"),
    request!(
        Git,
        "checkout_pr_create_request",
        "checkout.pr.create.request"
    ),
    request!(
        Git,
        "checkout_pr_merge_request",
        "checkout.pr.merge.request"
    ),
    request!(
        Git,
        "checkout_pr_status_request",
        "checkout.pr.status.request"
    ),
    request!(
        Git,
        "pull_request_timeline_request",
        "checkout.pr.timeline.request"
    ),
    request!(
        Git,
        "checkout.forge.set_auto_merge.request",
        "checkout.forge.set_auto_merge.request"
    ),
    request!(
        Git,
        "checkout.forge.get_check_details.request",
        "checkout.forge.get_check_details.request"
    ),
    request!(
        Git,
        "checkout.github.set_auto_merge.request",
        "checkout.github.set_auto_merge.request"
    ),
    request!(
        Git,
        "checkout.github.get_check_details.request",
        "checkout.github.get_check_details.request"
    ),
    request!(
        Files,
        "directory_suggestions_request",
        "directory.suggestions.request"
    ),
    request!(Files, "file_explorer_request", "fs.explorer.request"),
    request!(
        Files,
        "fs.file.subscribe.request",
        "fs.file.subscribe.request"
    ),
    request!(
        Files,
        "fs.file.unsubscribe.request",
        "fs.file.unsubscribe.request"
    ),
    request!(Files, "fs.file.write.request", "fs.file.write.request"),
    request!(Files, "fs.entry.create.request", "fs.entry.create.request"),
    request!(Files, "fs.entry.rename.request", "fs.entry.rename.request"),
    request!(
        Files,
        "fs.entry.duplicate.request",
        "fs.entry.duplicate.request"
    ),
    request!(Files, "fs.entry.delete.request", "fs.entry.delete.request"),
    request!(
        Files,
        "file_download_token_request",
        "fs.file.download_token.request"
    ),
    request!(Files, "file.upload.request", "file.upload.request"),
    request!(Terminal, "list_terminals_request", "terminal.list.request"),
    request!(
        Terminal,
        "subscribe_terminals_request",
        "terminal.list.subscribe.request"
    ),
    request!(
        Terminal,
        "unsubscribe_terminals_request",
        "terminal.list.unsubscribe.request"
    ),
    request!(
        Terminal,
        "create_terminal_request",
        "terminal.create.request"
    ),
    request!(
        Terminal,
        "terminal.rename.request",
        "terminal.rename.request"
    ),
    request!(
        Terminal,
        "subscribe_terminal_request",
        "terminal.subscribe.request"
    ),
    request!(
        Terminal,
        "unsubscribe_terminal_request",
        "terminal.unsubscribe.request"
    ),
    event!(Terminal, "terminal_input", "terminal.input"),
    request!(Terminal, "kill_terminal_request", "terminal.kill.request"),
    request!(
        Terminal,
        "capture_terminal_request",
        "terminal.capture.request"
    ),
    request!(Schedule, "schedule/create", "schedule.create.request"),
    request!(Schedule, "schedule/list", "schedule.list.request"),
    request!(Schedule, "schedule/inspect", "schedule.inspect.request"),
    request!(Schedule, "schedule/logs", "schedule.logs.request"),
    request!(Schedule, "schedule/update", "schedule.update.request"),
    request!(Schedule, "schedule/pause", "schedule.pause.request"),
    request!(Schedule, "schedule/resume", "schedule.resume.request"),
    request!(Schedule, "schedule/delete", "schedule.delete.request"),
    request!(Schedule, "schedule/run-once", "schedule.run_once.request"),
    request!(Voice, "set_voice_mode", "voice.mode.set.request"),
    event!(Voice, "voice_audio_chunk", "voice.audio.chunk"),
    request!(Voice, "abort_request", "voice.abort.request"),
    event!(Voice, "audio_played", "voice.audio.played"),
    event!(Voice, "dictation_stream_start", "dictation.stream.start"),
    event!(Voice, "dictation_stream_chunk", "dictation.stream.chunk"),
    event!(Voice, "dictation_stream_finish", "dictation.stream.finish"),
    event!(Voice, "dictation_stream_cancel", "dictation.stream.cancel"),
    request!(
        Session,
        "session.events.set_subscription.request",
        "session.events.set_subscription.request"
    ),
    request!(
        Session,
        "subscription.release.request",
        "subscription.release.request"
    ),
    request!(
        Session,
        "creation.subscribe.request",
        "creation.subscribe.request"
    ),
    event!(Session, "client_heartbeat", "session.heartbeat"),
    request!(Session, "ping", "connection.ping"),
    event!(Push, "register_push_token", "push.register"),
    request!(Push, "push.unregister.request", "push.unregister.request"),
    request!(
        Browser,
        "browser.host.register.request",
        "browser.host.register.request"
    ),
    response!(
        Browser,
        "browser.automation.execute.response",
        "browser.automation.execute.response"
    ),
    request!(
        Editor,
        "list_available_editors_request",
        "editor.available.list.request"
    ),
    request!(Editor, "open_in_editor_request", "editor.open.request"),
];

/// Find the requested interface entry by its pinned Paseo name.
#[must_use]
pub fn by_paseo_name(name: &str) -> Option<&'static MethodSpec> {
    PASEO_METHODS
        .iter()
        .find(|method| method.paseo_name == name)
}

/// Find the first interface entry using a canonical wire name.
///
/// Multiple Paseo legacy operations can intentionally map to one canonical method.
#[must_use]
pub fn by_canonical_name(name: &str) -> Option<&'static MethodSpec> {
    PASEO_METHODS
        .iter()
        .find(|method| method.canonical_name == name)
}

#[cfg(test)]
mod tests;
