use serde::{Deserialize, Serialize};

use super::serde_fields::{present, required_nullable};

/// Normalized Paseo checkout union; non-Git, external Git and managed Git remain distinct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "CheckoutInput")]
pub struct ProjectCheckoutLitePayload {
    /// Selected directory, which can be below the checkout root.
    pub cwd: String,
    /// Whether Git metadata was observed.
    pub is_git: bool,
    /// Current branch; null for a non-Git directory or detached HEAD.
    pub current_branch: Option<String>,
    /// Observed remote URL, if any.
    pub remote_url: Option<String>,
    /// Git root defaults to cwd; always null for non-Git directories.
    pub worktree_root: Option<String>,
    /// Original source field; managed Git requires a main repository root.
    pub is_paseo_owned_worktree: bool,
    /// Main repository root, null when unavailable.
    pub main_repo_root: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckoutInput {
    cwd: String,
    is_git: bool,
    #[serde(deserialize_with = "required_nullable")]
    current_branch: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    remote_url: Option<String>,
    #[serde(default, deserialize_with = "present")]
    worktree_root: Option<Option<String>>,
    is_paseo_owned_worktree: bool,
    #[serde(default, deserialize_with = "present")]
    main_repo_root: Option<Option<String>>,
}

impl TryFrom<CheckoutInput> for ProjectCheckoutLitePayload {
    type Error = &'static str;

    fn try_from(input: CheckoutInput) -> Result<Self, Self::Error> {
        let worktree_root = if input.is_git {
            if matches!(input.worktree_root, Some(None))
                || (input.is_paseo_owned_worktree && !matches!(input.main_repo_root, Some(Some(_))))
            {
                return Err("invalid Git checkout placement");
            }
            Some(
                input
                    .worktree_root
                    .flatten()
                    .unwrap_or_else(|| input.cwd.clone()),
            )
        } else {
            if input.current_branch.is_some()
                || input.remote_url.is_some()
                || input.is_paseo_owned_worktree
                || matches!(input.worktree_root, Some(Some(_)))
                || input.main_repo_root != Some(None)
            {
                return Err("invalid non-Git checkout placement");
            }
            None
        };
        Ok(Self {
            cwd: input.cwd,
            is_git: input.is_git,
            current_branch: input.current_branch,
            remote_url: input.remote_url,
            worktree_root,
            is_paseo_owned_worktree: input.is_paseo_owned_worktree,
            main_repo_root: input.main_repo_root.flatten(),
        })
    }
}
