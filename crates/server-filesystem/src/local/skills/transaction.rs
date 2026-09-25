use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use server_model::ErrorCode;
use uuid::Uuid;

use super::{LocalSkills, tree, valid_name};
use crate::ports::skills::{ApplyMode, SkillStore};
use crate::protocol::skills::{Kind, Operation, Selection};

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    target: usize,
    name: String,
    before: Option<String>,
    after: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Journal {
    id: String,
    roots: [std::path::PathBuf; 3],
    previous: Option<Selection>,
    entries: Vec<Entry>,
    committed: bool,
}

pub(super) fn apply(
    store: &mut LocalSkills,
    selection: &Selection,
    ops: &[Operation],
    mode: ApplyMode,
) -> Result<(), ErrorCode> {
    let mut journal = Journal {
        id: Uuid::new_v4().to_string(),
        roots: store.targets.clone(),
        previous: store.selection()?,
        entries: Vec::new(),
        committed: false,
    };
    let prepared = prepare(store, &mut journal, ops);
    if let Err(error) = prepared {
        recover(store)?;
        return Err(error);
    }
    let result = publish(store, &mut journal, selection, mode);
    if let Err(error) = result {
        recover(store)?;
        return Err(error);
    }
    // The committed journal remains authoritative if best-effort cleanup fails.
    let _ = recover(store);
    Ok(())
}

fn prepare(store: &LocalSkills, journal: &mut Journal, ops: &[Operation]) -> Result<(), ErrorCode> {
    // An empty journal makes interrupted staging discoverable before any live mutation.
    tree::save(&store.state.join("transaction.json"), journal)?;
    for op in ops {
        if !valid_name(&op.name) {
            return Err(ErrorCode::InvalidMessage);
        }
        let bundle = if op.kind == Kind::Delete {
            None
        } else {
            Some(tree::read(&store.source.join(&op.name))?.ok_or(ErrorCode::RegistryIo)?)
        };
        for target in 0..3 {
            let path = store.targets[target].join(&op.name);
            let before = tree::read(&path)?;
            if before.is_none() && bundle.is_none() {
                continue;
            }
            let fingerprint = before.as_ref().map(tree::Tree::fingerprint);
            let stage = stage(journal, target, &op.name);
            tree::safe(&stage)?;
            fs::create_dir_all(&stage).map_err(|_| ErrorCode::RegistryIo)?;
            let after = if let Some(bundle) = &bundle {
                let mut next = before.unwrap_or_default();
                next.overlay(bundle)?;
                next.write(&stage.join("after"))?;
                Some(next.fingerprint())
            } else {
                None
            };
            journal.entries.push(Entry {
                target,
                name: op.name.clone(),
                before: fingerprint,
                after,
            });
        }
    }
    tree::save(&store.state.join("transaction.json"), journal)
}

fn stage(journal: &Journal, target: usize, name: &str) -> std::path::PathBuf {
    journal.roots[target]
        .join(format!(".ait-skills-{}", journal.id))
        .join(name)
}

fn fingerprint(path: &Path) -> Result<Option<String>, ErrorCode> {
    Ok(tree::read(path)?.as_ref().map(tree::Tree::fingerprint))
}

fn publish(
    store: &mut LocalSkills,
    journal: &mut Journal,
    selection: &Selection,
    mode: ApplyMode,
) -> Result<(), ErrorCode> {
    for entry in &journal.entries {
        let path = store.targets[entry.target].join(&entry.name);
        let stage = stage(journal, entry.target, &entry.name);
        if fingerprint(&path)? != entry.before {
            return Err(ErrorCode::ResourceExhausted);
        }
        if entry.before.is_some() {
            fs::rename(&path, stage.join("before")).map_err(|_| ErrorCode::RegistryIo)?;
            if fingerprint(&stage.join("before"))? != entry.before {
                return Err(ErrorCode::ResourceExhausted);
            }
        }
        // Refuse a destination recreated before publication.
        if path.exists() {
            return Err(ErrorCode::ResourceExhausted);
        }
        if entry.after.is_some() {
            fs::rename(stage.join("after"), &path).map_err(|_| ErrorCode::RegistryIo)?;
        }
    }
    if mode == ApplyMode::Save {
        if !store.scan(selection)?.ops.is_empty() {
            return Err(ErrorCode::ResourceExhausted);
        }
        store.import(selection)?;
    }
    journal.committed = true;
    tree::save(&store.state.join("transaction.json"), journal)
}

pub(super) fn recover(store: &mut LocalSkills) -> Result<(), ErrorCode> {
    let path = store.state.join("transaction.json");
    let Some(journal): Option<Journal> = tree::load(&path)? else {
        return Ok(());
    };
    if Uuid::parse_str(&journal.id).is_err()
        || journal.roots != store.targets
        || journal.entries.len() > 780
        || journal
            .entries
            .iter()
            .any(|entry| entry.target >= 3 || !valid_name(&entry.name))
    {
        return Err(ErrorCode::RegistryIo);
    }
    if !journal.committed {
        for entry in journal.entries.iter().rev() {
            rollback(&journal, entry)?;
        }
        match &journal.previous {
            Some(selection) => store.import(selection)?,
            None => remove_file(&store.state.join("selection.json"))?,
        }
    }
    cleanup(&journal)?;
    remove_file(&path)
}

fn cleanup(journal: &Journal) -> Result<(), ErrorCode> {
    for root in &journal.roots {
        let directory = root.join(format!(".ait-skills-{}", journal.id));
        tree::safe(&directory)?;
        if directory.exists() {
            fs::remove_dir_all(directory).map_err(|_| ErrorCode::RegistryIo)?;
        }
    }
    Ok(())
}

fn rollback(journal: &Journal, entry: &Entry) -> Result<(), ErrorCode> {
    let path = journal.roots[entry.target].join(&entry.name);
    let stage = stage(journal, entry.target, &entry.name);
    let backup = stage.join("before");
    let current = fingerprint(&path)?;
    let original = fingerprint(&backup)?;
    if original.is_some() {
        if original != entry.before
            || (current.is_some() && (current != entry.after || stage.join("after").exists()))
        {
            return Err(ErrorCode::ResourceExhausted);
        }
        if current.is_some() {
            fs::remove_dir_all(&path).map_err(|_| ErrorCode::RegistryIo)?;
        }
        fs::rename(backup, path).map_err(|_| ErrorCode::RegistryIo)?;
    } else if entry.before.is_none() && current.is_some() && !stage.join("after").exists() {
        if current != entry.after || stage.join("after").exists() {
            return Err(ErrorCode::ResourceExhausted);
        }
        fs::remove_dir_all(path).map_err(|_| ErrorCode::RegistryIo)?;
    }
    Ok(())
}

fn remove_file(path: &Path) -> Result<(), ErrorCode> {
    tree::safe(path)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ErrorCode::RegistryIo),
    }
}

#[cfg(test)]
mod tests;
