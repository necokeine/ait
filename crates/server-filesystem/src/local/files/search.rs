use std::collections::{BTreeSet, VecDeque};
use std::fs;

use super::{LocalFiles, absolute, relative};
use crate::ports::files::{EntryKind, FileError, FileSearch};

const HIDDEN: &[&str] = &[
    ".agents",
    ".claude",
    ".codex",
    ".github",
    ".opencode",
    ".paseo",
    ".vscode",
];
const IGNORED: &[&str] = &[
    "node_modules",
    "venv",
    "env",
    "virtualenv",
    "dist",
    "build",
    "target",
    "out",
    "coverage",
    "vendor",
    "__pycache__",
    ".git",
];

pub(super) fn search(
    files: &LocalFiles,
    request: &FileSearch,
) -> Result<Vec<(String, EntryKind)>, FileError> {
    let workspace = request.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty());
    if (!request.include_files && !request.include_directories)
        || (workspace.is_none() && request.query.trim().is_empty())
    {
        return Ok(Vec::new());
    }
    let root = absolute(workspace.map_or_else(|| files.home.clone(), |cwd| files.expand(cwd)))?;
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let cwd = root.to_string_lossy();
    let query = request.query.trim();
    let expanded = files.expand(query);
    let query = if expanded.is_absolute() {
        let Ok(path) = relative(&root, &super::normalize(&expanded)) else {
            return Ok(Vec::new());
        };
        path
    } else {
        query.trim_start_matches("./").to_owned()
    };
    let query = if query == "." { String::new() } else { query };
    if query.split('/').any(|part| part == "..") {
        return Ok(Vec::new());
    }
    let mut exact = Vec::new();
    if (request.suffix || request.query.starts_with(['/', '~']))
        && let Ok(scoped) = files.scoped(&cwd, &query)
        && let Ok(metadata) = fs::metadata(&scoped.resolved)
    {
        let kind = if metadata.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        if included(request, kind) && !query.is_empty() {
            exact.push((
                format_path(&root, &scoped.relative, workspace.is_some()),
                kind,
            ));
        }
    }
    if exact.len() >= request.limit {
        exact.truncate(request.limit);
        return Ok(exact);
    }
    let ignored: BTreeSet<String> = if workspace.is_some() {
        crate::local::git::run(
            &root,
            &[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
                "-z",
            ],
        )
        .unwrap_or_default()
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_end_matches('/').to_owned())
        .collect()
    } else {
        BTreeSet::new()
    };
    let mut ranked = scan(files, request, &root, &query, &ignored);
    ranked.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    for (_, _, path, kind) in ranked {
        let path = format_path(&root, &path, workspace.is_some());
        if !exact.iter().any(|entry| entry.0 == path) {
            exact.push((path, kind));
        }
        if exact.len() >= request.limit {
            break;
        }
    }
    Ok(exact)
}

fn included(request: &FileSearch, kind: EntryKind) -> bool {
    match kind {
        EntryKind::File => request.include_files,
        EntryKind::Directory => request.include_directories,
    }
}

fn format_path(root: &std::path::Path, path: &str, workspace: bool) -> String {
    if workspace {
        path.to_owned()
    } else {
        root.join(path).to_string_lossy().into_owned()
    }
}

fn score(path: &str, query: &str, suffix: bool) -> Option<usize> {
    let path = path.to_lowercase();
    let query = query.to_lowercase();
    if suffix {
        return path.ends_with(&query).then_some(0);
    }
    let basename = path.rsplit('/').next().unwrap_or(&path);
    if basename == query {
        return Some(0);
    }
    if basename.starts_with(&query) {
        return Some(1);
    }
    if let Some(offset) = path.find(&query) {
        return Some(2 + offset);
    }
    let mut remaining = path.chars();
    let mut skipped = 0;
    for expected in query.chars() {
        let offset = remaining
            .by_ref()
            .position(|candidate| candidate == expected)?;
        skipped += offset;
    }
    Some(100 + skipped)
}

fn scan(
    files: &LocalFiles,
    request: &FileSearch,
    root: &std::path::Path,
    query: &str,
    ignored: &BTreeSet<String>,
) -> Vec<(usize, usize, String, EntryKind)> {
    let cwd = root.to_string_lossy();
    let browse = query.is_empty() || request.query.ends_with('/');
    let start = if browse && !query.is_empty() {
        query.trim_end_matches('/').to_owned()
    } else {
        ".".to_owned()
    };
    let mut queue = VecDeque::from([(start, 0)]);
    let mut visited = BTreeSet::new();
    let mut ranked = Vec::new();
    let mut scanned = 0;
    while let Some((path, depth)) = queue.pop_front() {
        let Ok(scoped) = files.scoped(&cwd, &path) else {
            continue;
        };
        if !visited.insert(scoped.resolved.clone()) {
            continue;
        }
        let Ok(children) = fs::read_dir(&scoped.resolved) else {
            continue;
        };
        for child in children {
            scanned += 1;
            if scanned > 20_000 {
                break;
            }
            let Ok(child) = child else {
                continue;
            };
            let name = child.file_name().to_string_lossy().into_owned();
            let path = super::join_relative(&scoped.relative, &name);
            let Ok(child_scope) = files.scoped(&cwd, &path) else {
                continue;
            };
            let Ok(metadata) = fs::metadata(&child_scope.resolved) else {
                continue;
            };
            let kind = if metadata.is_dir() {
                EntryKind::Directory
            } else if metadata.is_file() {
                EntryKind::File
            } else {
                continue;
            };
            if std::path::Path::new(&path)
                .ancestors()
                .any(|ancestor| ignored.contains(ancestor.to_string_lossy().as_ref()))
            {
                continue;
            }
            let hidden = name.starts_with('.');
            if kind == EntryKind::Directory
                && !browse
                && depth < 12
                && !IGNORED.contains(&name.as_str())
                && (!hidden || (request.cwd.is_some() && HIDDEN.contains(&name.as_str())))
            {
                queue.push_back((path.clone(), depth + 1));
            }
            if hidden || IGNORED.contains(&name.as_str()) || !included(request, kind) {
                continue;
            }
            if let Some(rank) = if browse {
                Some(0)
            } else {
                score(&path, query, request.suffix)
            } {
                ranked.push((rank, depth, path, kind));
            }
        }
        if scanned > 20_000 {
            break;
        }
    }
    ranked
}
