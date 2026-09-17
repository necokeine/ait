use super::{HostTools, MAX_BYTES, failed, string};
use ait_domain::DomainError;
use ait_ports::ToolInvocation;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use serde_json::{Value, json};
use std::io::Read;

const SKILL_ROOTS: &[&[&str]] = &[
    &[".agents", "skills"],
    &[".opencode", "skills"],
    &[".ait", "skills"],
    &["skills"],
];

fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn open_dir(root: &Dir, parts: &[&str]) -> Option<Dir> {
    let mut directory = root.try_clone().ok()?;
    for part in parts {
        directory = directory.open_dir_nofollow(part).ok()?;
    }
    Some(directory)
}

fn read_skill(root: &Dir, name: &str) -> Result<Option<(String, String)>, DomainError> {
    for prefix in SKILL_ROOTS {
        let Some(skills) = open_dir(root, prefix) else {
            continue;
        };
        let Ok(skill) = skills.open_dir_nofollow(name) else {
            continue;
        };
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let Ok(file) = skill.open_with("SKILL.md", &options) else {
            continue;
        };
        if !file.metadata().map_err(|_| failed())?.is_file() {
            continue;
        }
        let mut bytes = Vec::new();
        file.take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| failed())?;
        if bytes.len() > MAX_BYTES {
            return Err(failed());
        }
        let content = String::from_utf8(bytes).map_err(|_| failed())?;
        let path = format!("{}/{name}/SKILL.md", prefix.join("/"));
        return Ok(Some((path, content)));
    }
    Ok(None)
}

fn available_skills(root: &Dir) -> Vec<String> {
    let mut names = Vec::new();
    for prefix in SKILL_ROOTS {
        let Some(skills) = open_dir(root, prefix) else {
            continue;
        };
        let Ok(entries) = skills.entries() else {
            continue;
        };
        for entry in entries.flatten().take(100) {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if valid_skill_name(&name)
                && skills
                    .open_dir_nofollow(&name)
                    .ok()
                    .and_then(|directory| directory.symlink_metadata("SKILL.md").ok())
                    .is_some_and(|metadata| metadata.is_file() && !metadata.is_symlink())
            {
                names.push(name);
            }
        }
    }
    names.sort();
    names.dedup();
    names.truncate(100);
    names
}

impl HostTools {
    pub(super) fn utility(&self, request: &ToolInvocation) -> Result<Value, DomainError> {
        self.check(request)?;
        match request.tool_name.as_str() {
            "todowrite" => Ok(json!({
                "updated": true,
                "todos": request.arguments.get("todos").cloned().ok_or_else(failed)?,
            })),
            "skill" => {
                let name = string(&request.arguments, "name")?;
                if !valid_skill_name(name) {
                    return Err(failed());
                }
                match read_skill(&self.root, name)? {
                    Some((path, content)) => Ok(json!({
                        "name": name,
                        "path": path,
                        "content": content,
                    })),
                    None => Ok(json!({
                        "error": "skill_not_found",
                        "name": name,
                        "available": available_skills(&self.root),
                    })),
                }
            }
            _ => Err(failed()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_std::ambient_authority;

    #[test]
    fn skill_names_reject_paths_and_discovery_is_deterministic() {
        assert!(valid_skill_name("rust-review"));
        assert!(!valid_skill_name("../secret"));
        let temporary = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temporary.path().join("skills/zeta")).unwrap();
        std::fs::create_dir_all(temporary.path().join(".agents/skills/alpha")).unwrap();
        std::fs::write(temporary.path().join("skills/zeta/SKILL.md"), "z").unwrap();
        std::fs::write(temporary.path().join(".agents/skills/alpha/SKILL.md"), "a").unwrap();
        let root = Dir::open_ambient_dir(temporary.path(), ambient_authority()).unwrap();
        assert_eq!(available_skills(&root), ["alpha", "zeta"]);
        assert_eq!(read_skill(&root, "alpha").unwrap().unwrap().1, "a");
    }
}
