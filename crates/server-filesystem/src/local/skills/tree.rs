use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Serialize, de::DeserializeOwned};
use server_model::ErrorCode;
use sha2::{Digest, Sha256};

pub(super) const MANIFEST: &str = ".paseo-managed-files.json";
const MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Tree {
    pub files: BTreeMap<String, Vec<u8>>,
    pub directories: BTreeSet<String>,
    permissions: BTreeMap<String, fs::Permissions>,
    root_permissions: Option<fs::Permissions>,
}

pub(super) fn safe(path: &Path) -> Result<(), ErrorCode> {
    let mut current = PathBuf::new();
    for part in path.components() {
        if matches!(part, Component::ParentDir) {
            return Err(ErrorCode::InvalidMessage);
        }
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Err(ErrorCode::RegistryIo),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(ErrorCode::RegistryIo),
        }
    }
    Ok(())
}

pub(super) fn normalize(path: &Path) -> Result<PathBuf, ErrorCode> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(ErrorCode::InvalidMessage);
    }
    let mut existing = path;
    let mut suffix = Vec::new();
    while !existing.exists() {
        suffix.push(existing.file_name().ok_or(ErrorCode::InvalidMessage)?);
        existing = existing.parent().ok_or(ErrorCode::InvalidMessage)?;
    }
    let mut resolved = existing.canonicalize().map_err(|_| ErrorCode::RegistryIo)?;
    for part in suffix.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

pub(super) fn read(path: &Path) -> Result<Option<Tree>, ErrorCode> {
    safe(path)?;
    if !path.exists() {
        return Ok(None);
    }
    if !path.is_dir() {
        return Err(ErrorCode::RegistryIo);
    }
    let mut tree = Tree {
        root_permissions: Some(
            fs::metadata(path)
                .map_err(|_| ErrorCode::RegistryIo)?
                .permissions(),
        ),
        ..Tree::default()
    };
    walk(path, path, &mut tree, &mut 0, 0)?;
    Ok(Some(tree))
}

fn walk(
    root: &Path,
    path: &Path,
    tree: &mut Tree,
    bytes: &mut usize,
    depth: usize,
) -> Result<(), ErrorCode> {
    if depth > 64 {
        return Err(ErrorCode::ResourceExhausted);
    }
    for entry in fs::read_dir(path).map_err(|_| ErrorCode::RegistryIo)? {
        let entry = entry.map_err(|_| ErrorCode::RegistryIo)?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|_| ErrorCode::RegistryIo)?;
        let name = path
            .strip_prefix(root)
            .map_err(|_| ErrorCode::RegistryIo)?
            .to_str()
            .ok_or(ErrorCode::InvalidMessage)?
            .to_owned();
        #[cfg(windows)]
        let name = name.replace('\\', "/");
        if !valid_relative(&name) {
            return Err(ErrorCode::InvalidMessage);
        }
        if kind.is_dir() {
            tree.permissions.insert(
                name.clone(),
                entry
                    .metadata()
                    .map_err(|_| ErrorCode::RegistryIo)?
                    .permissions(),
            );
            tree.directories.insert(name);
            walk(root, &path, tree, bytes, depth + 1)?;
        } else if kind.is_file() {
            let mut content = Vec::new();
            fs::File::open(&path)
                .map_err(|_| ErrorCode::RegistryIo)?
                .take((MAX_BYTES + 1) as u64)
                .read_to_end(&mut content)
                .map_err(|_| ErrorCode::RegistryIo)?;
            *bytes = bytes.saturating_add(content.len());
            if *bytes > MAX_BYTES {
                return Err(ErrorCode::ResourceExhausted);
            }
            tree.permissions.insert(
                name.clone(),
                entry
                    .metadata()
                    .map_err(|_| ErrorCode::RegistryIo)?
                    .permissions(),
            );
            tree.files.insert(name, content);
        } else {
            return Err(ErrorCode::RegistryIo);
        }
        if tree.files.len() + tree.directories.len() > 4096 {
            return Err(ErrorCode::ResourceExhausted);
        }
    }
    Ok(())
}

pub(super) fn valid_relative(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\\')
        && Path::new(name)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

impl Tree {
    pub fn fingerprint(&self) -> String {
        let mut hash = Sha256::new();
        for name in &self.directories {
            hash.update(b"d");
            hash.update(name.len().to_le_bytes());
            hash.update(name.as_bytes());
        }
        for (name, bytes) in &self.files {
            hash.update(b"f");
            hash.update(name.len().to_le_bytes());
            hash.update(name.as_bytes());
            hash.update(Sha256::digest(bytes));
        }
        format!("{hash:x}", hash = hash.finalize())
    }

    pub fn matches_bundle(&self, bundle: &Self) -> bool {
        bundle
            .files
            .iter()
            .filter(|(name, _)| name.as_str() != MANIFEST)
            .all(|(name, bytes)| self.files.get(name) == Some(bytes))
    }

    pub fn overlay(&mut self, bundle: &Self) -> Result<(), ErrorCode> {
        if let Some(bytes) = self.files.get(MANIFEST)
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes)
            && value["version"] == 1
            && let Some(previous) = value["files"].as_object()
        {
            let mut removed = Vec::new();
            for (name, hash) in previous {
                if !valid_relative(name) {
                    return Err(ErrorCode::RegistryIo);
                }
                if !bundle.files.contains_key(name)
                    && self.files.get(name).is_some_and(|bytes| {
                        hash.as_str() == Some(format!("{:x}", Sha256::digest(bytes)).as_str())
                    })
                {
                    removed.push(name.clone());
                }
            }
            for name in removed {
                self.files.remove(&name);
            }
        }
        let mut hashes = BTreeMap::new();
        for (name, bytes) in &bundle.files {
            if name == MANIFEST {
                continue;
            }
            hashes.insert(name.clone(), format!("{:x}", Sha256::digest(bytes)));
            self.files.insert(name.clone(), bytes.clone());
        }
        self.directories.extend(bundle.directories.iter().cloned());
        self.files.insert(
            MANIFEST.to_owned(),
            serde_json::to_vec_pretty(&serde_json::json!({"version":1,"files":hashes}))
                .map_err(|_| ErrorCode::RegistryIo)?,
        );
        Ok(())
    }

    pub fn write(&self, path: &Path) -> Result<(), ErrorCode> {
        safe(path)?;
        fs::create_dir_all(path).map_err(|_| ErrorCode::RegistryIo)?;
        for directory in &self.directories {
            fs::create_dir_all(path.join(directory)).map_err(|_| ErrorCode::RegistryIo)?;
        }
        for (name, bytes) in &self.files {
            let path = path.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|_| ErrorCode::RegistryIo)?;
            }
            let mut file = fs::File::create(&path).map_err(|_| ErrorCode::RegistryIo)?;
            if let Some(permissions) = self.permissions.get(name) {
                file.set_permissions(permissions.clone())
                    .map_err(|_| ErrorCode::RegistryIo)?;
            }
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| ErrorCode::RegistryIo)?;
        }
        for directory in self.directories.iter().rev() {
            if let Some(permissions) = self.permissions.get(directory) {
                fs::set_permissions(path.join(directory), permissions.clone())
                    .map_err(|_| ErrorCode::RegistryIo)?;
            }
        }
        if let Some(permissions) = &self.root_permissions {
            fs::set_permissions(path, permissions.clone()).map_err(|_| ErrorCode::RegistryIo)?;
        }
        Ok(())
    }
}

pub(super) fn load<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, ErrorCode> {
    safe(path)?;
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ErrorCode::RegistryIo),
    };
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ErrorCode::RegistryIo)?;
    if bytes.len() > 1024 * 1024 {
        return Err(ErrorCode::ResourceExhausted);
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| ErrorCode::RegistryIo)
}

pub(super) fn save(path: &Path, value: &impl Serialize) -> Result<(), ErrorCode> {
    safe(path)?;
    let parent = path.parent().ok_or(ErrorCode::RegistryIo)?;
    fs::create_dir_all(parent).map_err(|_| ErrorCode::RegistryIo)?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| ErrorCode::RegistryIo)?;
    serde_json::to_writer(&mut file, value).map_err(|_| ErrorCode::RegistryIo)?;
    file.as_file()
        .sync_all()
        .map_err(|_| ErrorCode::RegistryIo)?;
    file.persist(path).map_err(|_| ErrorCode::RegistryIo)?;
    Ok(())
}
