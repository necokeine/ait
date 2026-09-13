//! Streaming, scoped repository inspection with explicit incomplete-result markers.
use super::{HostIoCheckpoint, HostTools, MAX_BYTES, denied, failed, safe_component, string};
use ait_domain::DomainError;
use ait_ports::ToolInvocation;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
#[cfg(unix)]
use cap_std::fs::OpenOptionsExt;
use cap_std::fs::{Dir, File, OpenOptions};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read};

const MAX_ENTRIES: usize = 100_000;
const MAX_SCAN_BYTES: u64 = 16 * 1024 * 1024;

fn page(args: &Value, default_offset: u64) -> (usize, usize) {
    let offset = args
        .get("offset")
        .and_then(Value::as_u64)
        .unwrap_or(default_offset);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(200)
        .min(1000);
    (
        usize::try_from(offset).unwrap_or(usize::MAX),
        limit as usize,
    )
}

fn glob(pattern: &str) -> Result<globset::GlobMatcher, DomainError> {
    globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|_| failed())
}

impl HostTools {
    fn open_read(&self, path: &str, request: &ToolInvocation) -> Result<File, DomainError> {
        self.checkpoint(request, HostIoCheckpoint::BeforeRead)?;
        let (dir, name) = self.parent(path, request)?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        options.custom_flags(libc::O_NONBLOCK);
        let file = dir.open_with(name, &options).map_err(|_| failed())?;
        if !file.metadata().map_err(|_| failed())?.is_file() {
            return Err(denied());
        }
        Ok(file)
    }

    fn directory(&self, path: &str, request: &ToolInvocation) -> Result<Dir, DomainError> {
        if path.is_empty() || path == "." {
            return self.root.try_clone().map_err(|_| failed());
        }
        let (parent, name) = self.parent(path, request)?;
        parent.open_dir_nofollow(name).map_err(|_| denied())
    }

    fn collect_files(
        &self,
        dir: &Dir,
        prefix: &str,
        files: &mut Vec<String>,
        visited: &mut usize,
        request: &ToolInvocation,
    ) -> Result<bool, DomainError> {
        self.check(request)?;
        if prefix.bytes().filter(|byte| *byte == b'/').count() > 64 {
            return Ok(true);
        }
        // Sort before descending so bounded traversal is stable between pages.
        let mut entries = Vec::new();
        let mut truncated = false;
        for entry in dir.entries().map_err(|_| failed())? {
            self.check(request)?;
            *visited += 1;
            if *visited > MAX_ENTRIES {
                truncated = true;
                break;
            }
            let entry = entry.map_err(|_| failed())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if safe_component(&name) && !name.eq_ignore_ascii_case("target") {
                entries.push((name, entry.file_type().map_err(|_| failed())?));
            }
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, kind) in entries {
            let path = format!("{prefix}{name}");
            if kind.is_file() {
                files.push(path);
            } else if kind.is_dir() {
                if *visited >= MAX_ENTRIES {
                    truncated = true;
                    break;
                }
                truncated |= self.collect_files(
                    &dir.open_dir_nofollow(&name).map_err(|_| denied())?,
                    &format!("{path}/"),
                    files,
                    visited,
                    request,
                )?;
            }
        }
        Ok(truncated)
    }

    pub(super) fn read_window(&self, request: &ToolInvocation) -> Result<Value, DomainError> {
        let path = string(&request.arguments, "file_path")?;
        let (offset, limit) = page(&request.arguments, 1);
        let offset = offset.saturating_sub(1);
        if let Ok(dir) = self.directory(path, request) {
            let mut entries = Vec::new();
            let mut scan_truncated = false;
            for (index, entry) in dir.entries().map_err(|_| failed())?.enumerate() {
                self.check(request)?;
                if index >= MAX_ENTRIES {
                    scan_truncated = true;
                    break;
                }
                let entry = entry.map_err(|_| failed())?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if !safe_component(&name) || name.eq_ignore_ascii_case("target") {
                    continue;
                }
                let kind = entry.file_type().map_err(|_| failed())?;
                if kind.is_dir() {
                    entries.push(format!("{name}/"));
                } else if kind.is_file() {
                    entries.push(name);
                }
            }
            entries.sort();
            let total = entries.len();
            let mut results = Vec::new();
            let mut bytes = 0;
            for name in entries.into_iter().skip(offset).take(limit) {
                bytes += serde_json::to_string(&name).map_err(|_| failed())?.len() + 1;
                if bytes > MAX_BYTES / 2 {
                    break;
                }
                results.push(name);
            }
            let next = offset.saturating_add(results.len());
            return Ok(
                json!({"entries":results,"truncated":scan_truncated || next < total,"scan_truncated":scan_truncated,"next_offset":next+1}),
            );
        }
        let file = self.open_read(path, request)?;
        let mut reader = BufReader::new(file);
        let mut text = String::new();
        let mut line = Vec::new();
        let mut index = 0usize;
        let mut returned = 0;
        let mut scanned = 0;
        let truncated = loop {
            self.check(request)?;
            line.clear();
            let count = Read::by_ref(&mut reader)
                .take((MAX_BYTES + 1) as u64)
                .read_until(b'\n', &mut line)
                .map_err(|_| failed())?;
            if count == 0 {
                break false;
            }
            scanned += count;
            if count > MAX_BYTES || scanned as u64 > MAX_SCAN_BYTES {
                break true;
            }
            index += 1;
            if index <= offset {
                continue;
            }
            let value = std::str::from_utf8(&line).map_err(|_| failed())?;
            let formatted = format!("{index}: {}\n", value.trim_end_matches(['\r', '\n']));
            // Reserve room for JSON escaping and the response envelope.
            if returned >= limit || text.len() + formatted.len() > MAX_BYTES / 8 {
                break true;
            }
            text.push_str(&formatted);
            returned += 1;
        };
        Ok(
            json!({"text":text,"truncated":truncated,"next_offset":offset.saturating_add(returned).saturating_add(1)}),
        )
    }

    pub(super) fn search(&self, request: &ToolInvocation) -> Result<Value, DomainError> {
        let args = &request.arguments;
        let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
        let mut files = Vec::new();
        let scan_truncated = if let Ok(dir) = self.directory(path, request) {
            let prefix = if path == "." || path.is_empty() {
                String::new()
            } else {
                format!("{}/", path.trim_end_matches('/'))
            };
            self.collect_files(&dir, &prefix, &mut files, &mut 0, request)?
        } else {
            self.open_read(path, request)?;
            files.push(path.to_owned());
            false
        };
        files.sort();
        let is_glob = request.tool_name == "glob";
        let filter = if is_glob {
            Some(string(args, "pattern")?)
        } else {
            args.get("include").and_then(Value::as_str)
        };
        let matcher = filter.map(glob).transpose()?;
        let regex = if is_glob {
            None
        } else {
            Some(
                regex::RegexBuilder::new(string(args, "pattern")?)
                    .size_limit(MAX_BYTES)
                    .build()
                    .map_err(|_| failed())?,
            )
        };
        let mode = args
            .get("output_mode")
            .and_then(Value::as_str)
            .unwrap_or("content");
        let (offset, limit) = page(args, 0);
        let mut results = Vec::new();
        let mut total_results = 0usize;
        let mut total_count = 0usize;
        let mut skipped = 0usize;
        let mut bytes = 0;
        let mut truncated = scan_truncated;
        let mut page_full = false;
        let mut push = |value: Value| {
            total_results += 1;
            if total_results <= offset {
                return;
            }
            let size = value.to_string().len() + 1;
            if page_full || results.len() >= limit || bytes + size > MAX_BYTES / 2 {
                page_full = true;
                truncated = true;
                return;
            }
            bytes += size;
            results.push(value);
        };
        for path in files {
            self.check(request)?;
            if let Some(matcher) = &matcher {
                let candidate = if filter.is_some_and(|p| !p.contains('/')) {
                    path.rsplit('/').next().unwrap_or(&path)
                } else {
                    &path
                };
                if !matcher.is_match(candidate) {
                    continue;
                }
            }
            let Some(regex) = &regex else {
                push(json!(path));
                continue;
            };
            let mut line_count = 0usize;
            let scan = self.scan_lines(&path, request, |line, text| {
                if regex.is_match(text) {
                    line_count += 1;
                    if mode == "content" {
                        push(json!({"path":path,"line":line,"text":text.chars().take(2000).collect::<String>(),"text_truncated":text.chars().count()>2000}));
                    }
                }
            });
            if scan.is_err() {
                self.check(request)?;
                skipped += 1;
                continue;
            }
            total_count += line_count;
            if mode == "count" {
                push(json!({"path":path,"count":line_count}));
            } else if mode == "files_with_matches" && line_count > 0 {
                push(json!(path));
            }
        }
        let next = offset.saturating_add(results.len());
        Ok(
            json!({"matches":results,"total_count":total_count,"total_results":total_results,"truncated":truncated || skipped>0,"scan_truncated":scan_truncated,"count_complete":!scan_truncated && skipped==0,"skipped_files":skipped,"next_offset":next}),
        )
    }

    fn scan_lines(
        &self,
        path: &str,
        request: &ToolInvocation,
        mut visit: impl FnMut(usize, &str),
    ) -> Result<(), DomainError> {
        let file = self.open_read(path, request)?;
        if file.metadata().map_err(|_| failed())?.len() > MAX_SCAN_BYTES {
            return Err(failed());
        }
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        let mut index = 0;
        let mut bytes = 0;
        loop {
            self.check(request)?;
            line.clear();
            let count = Read::by_ref(&mut reader)
                .take((MAX_BYTES + 1) as u64)
                .read_until(b'\n', &mut line)
                .map_err(|_| failed())?;
            if count == 0 {
                return Ok(());
            }
            bytes += count;
            if count > MAX_BYTES || bytes as u64 > MAX_SCAN_BYTES || line.contains(&0) {
                return Err(failed());
            }
            index += 1;
            visit(
                index,
                std::str::from_utf8(&line)
                    .map_err(|_| failed())?
                    .trim_end_matches(['\r', '\n']),
            );
        }
    }
}
