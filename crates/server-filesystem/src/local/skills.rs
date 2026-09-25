//! Bounded filesystem scans and journaled installation across agent homes.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use server_model::ErrorCode;

use crate::ports::skills::{ApplyMode, SkillStore};
use crate::protocol::skills::{Kind, Operation, Selection, Snapshot};

mod transaction;
mod tree;

const LEGACY: &[&str] = &[
    "paseo-chat",
    "paseo-epic",
    "paseo-orchestrate",
    "paseo-orchestrator",
];

/// Filesystem adapter. Targets are fixed by host configuration, never by WebSocket parameters.
#[derive(Debug)]
pub struct LocalSkills {
    source: PathBuf,
    targets: [PathBuf; 3],
    state: PathBuf,
}

impl LocalSkills {
    /// Resolve roots without creating target directories; reject overlapping installation roots.
    /// # Errors
    /// Returns invalid configuration or filesystem errors.
    pub fn new(source: &Path, targets: &[PathBuf; 3], state: &Path) -> Result<Self, ErrorCode> {
        let result = Self {
            source: tree::normalize(source)?,
            targets: [
                tree::normalize(&targets[0])?,
                tree::normalize(&targets[1])?,
                tree::normalize(&targets[2])?,
            ],
            state: tree::normalize(state)?,
        };
        let roots = [
            &result.source,
            &result.state,
            &result.targets[0],
            &result.targets[1],
            &result.targets[2],
        ];
        for (index, root) in roots.iter().enumerate() {
            if roots
                .iter()
                .skip(index + 1)
                .any(|other| root.starts_with(other) || other.starts_with(root))
            {
                return Err(ErrorCode::InvalidMessage);
            }
        }
        Ok(result)
    }

    fn available(&self) -> Result<Vec<String>, ErrorCode> {
        tree::safe(&self.source)?;
        let entries = match fs::read_dir(&self.source) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(ErrorCode::RegistryIo),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|_| ErrorCode::RegistryIo)?;
            if !entry
                .file_type()
                .map_err(|_| ErrorCode::RegistryIo)?
                .is_dir()
            {
                continue;
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| ErrorCode::InvalidMessage)?;
            if !valid_name(&name) {
                return Err(ErrorCode::InvalidMessage);
            }
            names.push(name);
            if names.len() > 256 {
                return Err(ErrorCode::ResourceExhausted);
            }
        }
        names.sort();
        Ok(names)
    }
}

fn valid_name(name: &str) -> bool {
    tree::valid_relative(name) && !name.contains('/') && !name.starts_with(".ait-skills-")
}

impl SkillStore for LocalSkills {
    fn recover(&mut self) -> Result<(), ErrorCode> {
        transaction::recover(self)
    }

    fn selection(&self) -> Result<Option<Selection>, ErrorCode> {
        tree::load(&self.state.join("selection.json"))
    }

    fn import(&mut self, selection: &Selection) -> Result<(), ErrorCode> {
        tree::save(&self.state.join("selection.json"), selection)
    }

    fn scan(&self, selection: &Selection) -> Result<Snapshot, ErrorCode> {
        let available = self.available()?;
        let names: BTreeSet<_> = available
            .iter()
            .map(String::as_str)
            .chain(LEGACY.iter().copied())
            .collect();
        let mut installed = Vec::new();
        let mut ops = Vec::new();
        for name in names {
            let desired = selection.contains(name) && available.iter().any(|entry| entry == name);
            let bundle = if desired {
                tree::read(&self.source.join(name))?
            } else {
                None
            };
            let mut present = 0;
            let mut changed = false;
            for root in &self.targets {
                if let Some(tree) = tree::read(&root.join(name))? {
                    present += 1;
                    changed |= bundle
                        .as_ref()
                        .is_some_and(|bundle| !tree.matches_bundle(bundle));
                }
            }
            if present > 0 {
                installed.push(name.to_owned());
            }
            let kind = if bundle.is_some() {
                if present < 3 {
                    Some(Kind::Add)
                } else if changed {
                    Some(Kind::Update)
                } else {
                    None
                }
            } else if present > 0 {
                Some(Kind::Delete)
            } else {
                None
            };
            if let Some(kind) = kind {
                ops.push(Operation {
                    kind,
                    name: name.to_owned(),
                });
            }
        }
        let state = if installed.is_empty() {
            "not-installed"
        } else if ops.is_empty() {
            "up-to-date"
        } else {
            "drift"
        };
        Ok(Snapshot {
            state: state.to_owned(),
            ops,
            available,
            installed,
            selection: selection.clone(),
        })
    }

    fn apply(
        &mut self,
        selection: &Selection,
        ops: &[Operation],
        mode: ApplyMode,
    ) -> Result<(), ErrorCode> {
        let snapshot = self.scan(selection)?;
        let expected: Vec<_> = match mode {
            ApplyMode::Save => snapshot.ops,
            ApplyMode::Reconcile => snapshot
                .ops
                .into_iter()
                .filter(|op| op.kind != Kind::Delete)
                .collect(),
            ApplyMode::Uninstall => snapshot
                .installed
                .into_iter()
                .map(|name| Operation {
                    kind: Kind::Delete,
                    name,
                })
                .collect(),
        };
        if expected != ops {
            return Err(ErrorCode::ResourceExhausted);
        }
        transaction::apply(self, selection, ops, mode)
    }
}

#[cfg(test)]
mod tests;
