use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use server_ports::provisioning::{ProjectIcon, ProjectIconStore, ProjectIconStoreError};
use sha2::{Digest, Sha256};

const MAX_CUSTOM_BYTES: usize = 512 * 1024;
const MAX_AUTOMATIC_BYTES: u64 = 32 * 1024;
const PATTERNS: &[&str] = &[
    "favicon.svg",
    "favicon.png",
    "favicon-*.svg",
    "favicon-*.png",
    "favico.svg",
    "favico.png",
    "icon.svg",
    "icon.png",
    "app-icon.svg",
    "app-icon.png",
    "apple-touch-icon.png",
    "apple-touch-icon-*.png",
    "icon-*.png",
    "android-chrome-*.png",
    "safari-pinned-tab.svg",
    "mstile-*.png",
    "logo.svg",
    "logo.png",
    "favicon.ico",
    "favico.ico",
];
const PRIORITY_DIRS: &[&str] = &["public", "static", "priv/static", "assets", "images", "img"];
const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".output",
    "coverage",
    ".cache",
    "vendor",
    "src",
    "lib",
    "test",
    "tests",
    "__tests__",
];

/// File-backed custom icons plus bounded automatic project icon discovery.
#[derive(Debug)]
pub struct LocalProjectIconStore {
    icon_directory: PathBuf,
}

impl LocalProjectIconStore {
    /// Store custom icons below the server's private data directory.
    #[must_use]
    pub fn new(icon_directory: PathBuf) -> Self {
        Self { icon_directory }
    }

    fn path(&self, project_id: &str) -> PathBuf {
        let digest = Sha256::digest(project_id.as_bytes());
        self.icon_directory.join(format!("{digest:x}.bin"))
    }
}

impl ProjectIconStore for LocalProjectIconStore {
    fn write_custom(&self, project_id: &str, bytes: &[u8]) -> Result<(), ProjectIconStoreError> {
        validate_custom(bytes)?;
        std::fs::create_dir_all(&self.icon_directory).map_err(|_| ProjectIconStoreError::Io)?;
        let mut staged = tempfile::NamedTempFile::new_in(&self.icon_directory)
            .map_err(|_| ProjectIconStoreError::Io)?;
        staged
            .write_all(bytes)
            .and_then(|()| staged.as_file().sync_all())
            .map_err(|_| ProjectIconStoreError::Io)?;
        staged
            .persist(self.path(project_id))
            .map_err(|_| ProjectIconStoreError::Io)?;
        #[cfg(unix)]
        File::open(&self.icon_directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| ProjectIconStoreError::Io)?;
        Ok(())
    }

    fn remove_custom(&self, project_id: &str) -> Result<(), ProjectIconStoreError> {
        match std::fs::remove_file(self.path(project_id)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(ProjectIconStoreError::Io),
        }
    }

    fn read_custom(&self, project_id: &str) -> Result<Option<ProjectIcon>, ProjectIconStoreError> {
        let bytes = match std::fs::read(self.path(project_id)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ProjectIconStoreError::Io),
        };
        Ok(validate_custom(&bytes).ok())
    }

    fn find_automatic(
        &self,
        project_root: &str,
    ) -> Result<Option<ProjectIcon>, ProjectIconStoreError> {
        let root = std::fs::canonicalize(project_root).map_err(|_| ProjectIconStoreError::Io)?;
        if !root.is_dir() {
            return Err(ProjectIconStoreError::Io);
        }
        let candidate = find_automatic_path(&root);
        let Some(path) = candidate else {
            return Ok(None);
        };
        match path.metadata() {
            Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_AUTOMATIC_BYTES => {}
            _ => return Ok(None),
        }
        let Ok(bytes) = std::fs::read(&path) else {
            return Ok(None);
        };
        Ok(validate_automatic(&bytes, &path))
    }
}

fn validate_custom(bytes: &[u8]) -> Result<ProjectIcon, ProjectIconStoreError> {
    if bytes.is_empty() || bytes.len() > MAX_CUSTOM_BYTES {
        return Err(ProjectIconStoreError::Invalid);
    }
    let (mime_type, dimensions) =
        raster_type_and_dimensions(bytes).ok_or(ProjectIconStoreError::Invalid)?;
    let (width, height) = dimensions.ok_or(ProjectIconStoreError::Invalid)?;
    if width == 0 || width != height || width > 1024 {
        return Err(ProjectIconStoreError::Invalid);
    }
    Ok(ProjectIcon {
        bytes: bytes.to_vec(),
        mime_type: mime_type.to_owned(),
    })
}

fn validate_automatic(bytes: &[u8], path: &Path) -> Option<ProjectIcon> {
    if let Some((mime_type, dimensions)) = raster_type_and_dimensions(bytes) {
        let (width, height) = dimensions?;
        return (width > 0 && width == height).then(|| ProjectIcon {
            bytes: bytes.to_vec(),
            mime_type: mime_type.to_owned(),
        });
    }
    let is_svg = path.extension().is_some_and(|extension| extension == "svg")
        && String::from_utf8_lossy(bytes)
            .trim_start_matches('\u{feff}')
            .trim_start()
            .starts_with("<svg");
    is_svg.then(|| ProjectIcon {
        bytes: bytes.to_vec(),
        mime_type: "image/svg+xml".to_owned(),
    })
}

fn raster_type_and_dimensions(bytes: &[u8]) -> Option<(&'static str, Option<(u32, u32)>)> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some(("image/png", png_dimensions(bytes)));
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some(("image/jpeg", jpeg_dimensions(bytes)));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(("image/gif", gif_dimensions(bytes)));
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return Some(("image/webp", webp_dimensions(bytes)));
    }
    if bytes.len() >= 22 && bytes.get(0..4) == Some(&[0, 0, 1, 0]) {
        return Some(("image/x-icon", Some((1, 1))));
    }
    None
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    Some((
        u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?),
        u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?),
    ))
}

fn gif_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    Some((
        u16::from_le_bytes(bytes.get(6..8)?.try_into().ok()?).into(),
        u16::from_le_bytes(bytes.get(8..10)?.try_into().ok()?).into(),
    ))
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut offset = 2;
    while offset + 8 < bytes.len() {
        if bytes[offset] != 0xff {
            offset += 1;
            continue;
        }
        let marker = bytes[offset + 1];
        if (0xc0..=0xc2).contains(&marker) {
            return Some((
                u16::from_be_bytes(bytes.get(offset + 7..offset + 9)?.try_into().ok()?).into(),
                u16::from_be_bytes(bytes.get(offset + 5..offset + 7)?.try_into().ok()?).into(),
            ));
        }
        let length = usize::from(u16::from_be_bytes(
            bytes.get(offset + 2..offset + 4)?.try_into().ok()?,
        ));
        if length == 0 {
            return None;
        }
        offset = offset.checked_add(2 + length)?;
    }
    None
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    match bytes.get(12..16)? {
        b"VP8 " => Some((
            u32::from(u16::from_le_bytes(bytes.get(26..28)?.try_into().ok()?) & 0x3fff),
            u32::from(u16::from_le_bytes(bytes.get(28..30)?.try_into().ok()?) & 0x3fff),
        )),
        b"VP8L" => {
            let bits = u32::from_le_bytes(bytes.get(21..25)?.try_into().ok()?);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        _ => None,
    }
}

fn find_automatic_path(root: &Path) -> Option<PathBuf> {
    for directory in PRIORITY_DIRS {
        let directory = root.join(directory);
        if let Some(path) = find_in_tree(&directory, 0, 2) {
            return Some(path);
        }
    }
    for collection in ["packages", "apps"] {
        let Ok(entries) = std::fs::read_dir(root.join(collection)) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                for directory in PRIORITY_DIRS {
                    if let Some(path) = find_in_tree(&entry.path().join(directory), 0, 2) {
                        return Some(path);
                    }
                }
                if let Some(path) = find_in_directory(&entry.path()) {
                    return Some(path);
                }
            }
        }
    }
    find_in_directory(root)
}

fn find_in_tree(directory: &Path, depth: usize, max_depth: usize) -> Option<PathBuf> {
    if depth > max_depth || !directory.is_dir() {
        return None;
    }
    if let Some(path) = find_in_directory(directory) {
        return Some(path);
    }
    let entries = std::fs::read_dir(directory).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if IGNORED_DIRS.iter().any(|ignored| name == *ignored)
            || !entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            continue;
        }
        if let Some(path) = find_in_tree(&entry.path(), depth + 1, max_depth) {
            return Some(path);
        }
    }
    None
}

fn find_in_directory(directory: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(directory)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .collect::<Vec<_>>();
    for pattern in PATTERNS {
        for entry in &entries {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if matches_pattern(name, pattern) {
                return Some(entry.path());
            }
        }
    }
    None
}

fn matches_pattern(name: &str, pattern: &str) -> bool {
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return name == pattern;
    };
    name.starts_with(prefix) && name.ends_with(suffix) && name.len() >= prefix.len() + suffix.len()
}

#[cfg(test)]
mod tests;
