//! Paseo directory-suggestions and file-explorer behavior.

use super::*;

fn put(cwd: &str, path: &str) {
    let path = Path::new(cwd).join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "content").unwrap();
}

fn request(cwd: &str, query: &str) -> FileSearch {
    FileSearch {
        cwd: Some(cwd.to_owned()),
        query: query.to_owned(),
        include_files: true,
        include_directories: false,
        suffix: false,
        limit: 30,
    }
}

fn paths(files: &LocalFiles, request: &FileSearch) -> Vec<String> {
    files
        .search(request)
        .unwrap()
        .into_iter()
        .map(|entry| entry.0)
        .collect()
}

#[test]
fn suggestion_ranking_orders_exact_prefix_substring_then_fuzzy_matches() {
    let (_temp, files, cwd) = fixture();
    for name in [
        "msgrndr",
        "msgrndr-panel.tsx",
        "use-msgrndr.ts",
        "message-renderer.tsx",
    ] {
        put(&cwd, &format!("src/components/{name}"));
    }
    assert_eq!(
        paths(&files, &request(&cwd, "msgrndr")),
        [
            "src/components/msgrndr",
            "src/components/msgrndr-panel.tsx",
            "src/components/use-msgrndr.ts",
            "src/components/message-renderer.tsx"
        ]
    );
}

#[test]
fn suffix_search_requires_a_whole_path_segment_and_excludes_fuzzy_matches() {
    let (_temp, files, cwd) = fixture();
    for name in [
        "src/file.ts",
        "packages/app/src/file.ts",
        "src/paseo-config-file.ts",
        "src/fuzzy-file-test.ts",
    ] {
        put(&cwd, name);
    }
    let mut query = request(&cwd, "file.ts");
    query.suffix = true;
    assert_eq!(
        paths(&files, &query),
        ["src/file.ts", "packages/app/src/file.ts"]
    );
    query.query = "src/file.ts".to_owned();
    assert_eq!(
        paths(&files, &query),
        ["src/file.ts", "packages/app/src/file.ts"]
    );
}

#[test]
fn explicit_hidden_suffix_path_bypasses_hidden_discovery() {
    let (_temp, files, cwd) = fixture();
    put(&cwd, ".dev/paseo-home/daemon.log");
    assert!(paths(&files, &request(&cwd, "daemon")).is_empty());
    let mut query = request(&cwd, ".dev/paseo-home/daemon.log");
    query.suffix = true;
    query.limit = 1;
    assert_eq!(paths(&files, &query), [".dev/paseo-home/daemon.log"]);
}

#[test]
fn gitignored_paths_are_hidden_from_discovery_but_available_by_exact_suffix() {
    let (_temp, files, cwd) = fixture();
    crate::local::git::run(Path::new(&cwd), &["init", "--quiet"]).unwrap();
    fs::write(Path::new(&cwd).join(".gitignore"), "generated/\n").unwrap();
    put(&cwd, "generated/search-notes.md");
    put(&cwd, "src/search-notes.md");
    assert_eq!(
        paths(&files, &request(&cwd, "search-notes")),
        ["src/search-notes.md"]
    );
    let mut query = request(&cwd, "generated/search-notes.md");
    query.suffix = true;
    query.limit = 1;
    assert_eq!(paths(&files, &query), ["generated/search-notes.md"]);
}

#[test]
fn dependency_environment_and_build_output_directories_are_not_traversed() {
    let (_temp, files, cwd) = fixture();
    for directory in [
        "node_modules",
        "venv",
        "env",
        "dist",
        "build",
        "target",
        "vendor",
        "coverage",
        "__pycache__",
    ] {
        put(&cwd, &format!("{directory}/needle.txt"));
    }
    put(&cwd, "src/needle.txt");
    assert_eq!(paths(&files, &request(&cwd, "needle")), ["src/needle.txt"]);
}

#[test]
fn allowlisted_hidden_directories_are_traversed_without_suggesting_hidden_entries() {
    let (_temp, files, cwd) = fixture();
    for directory in [
        ".agents", ".claude", ".codex", ".github", ".vscode", ".private",
    ] {
        put(&cwd, &format!("{directory}/needle.txt"));
    }
    let mut result = paths(&files, &request(&cwd, "needle"));
    result.sort();
    assert_eq!(
        result,
        [
            ".agents/needle.txt",
            ".claude/needle.txt",
            ".codex/needle.txt",
            ".github/needle.txt",
            ".vscode/needle.txt"
        ]
    );
    let mut directories = request(&cwd, "");
    directories.include_directories = true;
    assert!(paths(&files, &directories).is_empty());
}

#[test]
fn absolute_directory_query_browses_only_its_direct_children() {
    let (_temp, files, cwd) = fixture();
    put(&cwd, "src/one.rs");
    put(&cwd, "src/nested/deep.rs");
    put(&cwd, "elsewhere/outside.rs");
    let mut query = request(&cwd, &format!("{cwd}/src/"));
    query.include_directories = true;
    let mut result = paths(&files, &query);
    result.sort();
    assert_eq!(result, ["src", "src/nested", "src/one.rs"]);
}

#[test]
fn home_relative_query_returns_absolute_paths_without_a_workspace() {
    let (_temp, files, cwd) = fixture();
    put(&cwd, "src/home.rs");
    let mut query = request(&cwd, "~/workspace/src/home.rs");
    query.cwd = None;
    query.suffix = true;
    let expected = Path::new(&cwd).join("src/home.rs");
    assert_eq!(
        paths(&files, &query),
        [expected.to_string_lossy().into_owned()]
    );
}

#[test]
fn entry_kind_filters_and_result_limits_apply_before_returning_suggestions() {
    let (_temp, files, cwd) = fixture();
    put(&cwd, "match-directory/match-one");
    put(&cwd, "match-two");
    let mut query = request(&cwd, "match");
    query.include_files = false;
    query.include_directories = true;
    assert_eq!(
        files.search(&query).unwrap(),
        [("match-directory".to_owned(), EntryKind::Directory)]
    );
    query.include_files = true;
    query.include_directories = false;
    query.limit = 1;
    let result = files.search(&query).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].1, EntryKind::File);
    query.include_files = false;
    assert!(files.search(&query).unwrap().is_empty());
}

#[test]
fn subsequent_search_observes_new_children_without_restart() {
    let (_temp, files, cwd) = fixture();
    assert!(paths(&files, &request(&cwd, "new-file")).is_empty());
    put(&cwd, "src/new-file.rs");
    assert_eq!(
        paths(&files, &request(&cwd, "new-file")),
        ["src/new-file.rs"]
    );
    fs::remove_file(Path::new(&cwd).join("src/new-file.rs")).unwrap();
    assert!(paths(&files, &request(&cwd, "new-file")).is_empty());
}

#[test]
fn path_fragments_match_at_any_depth_but_never_escape_the_workspace() {
    let (_temp, files, cwd) = fixture();
    put(&cwd, "packages/server/src/feature/file.rs");
    assert_eq!(
        paths(&files, &request(&cwd, "server/src/feature")),
        ["packages/server/src/feature/file.rs"]
    );
    assert!(paths(&files, &request(&cwd, "../workspace/packages")).is_empty());
}

#[cfg(unix)]
#[test]
fn symlink_directory_cycles_do_not_repeat_results_or_leave_the_root() {
    let (temp, files, cwd) = fixture();
    put(&cwd, "src/needle.rs");
    std::os::unix::fs::symlink("..", Path::new(&cwd).join("src/back")).unwrap();
    put(temp.path().to_str().unwrap(), "outside/needle-secret.rs");
    std::os::unix::fs::symlink(temp.path().join("outside"), Path::new(&cwd).join("outside"))
        .unwrap();
    assert_eq!(paths(&files, &request(&cwd, "needle")), ["src/needle.rs"]);
}

#[test]
fn tracked_and_untracked_case_only_renames_keep_content_and_index_names() {
    let (_temp, files, cwd) = fixture();
    crate::local::git::run(Path::new(&cwd), &["init", "--quiet"]).unwrap();
    put(&cwd, "tracked.txt");
    crate::local::git::run(Path::new(&cwd), &["add", "tracked.txt"]).unwrap();
    put(&cwd, "untracked.txt");
    assert_eq!(
        files.rename(&cwd, "tracked.txt", "Tracked.txt").unwrap(),
        "Tracked.txt"
    );
    assert_eq!(
        files
            .rename(&cwd, "untracked.txt", "Untracked.txt")
            .unwrap(),
        "Untracked.txt"
    );
    assert_eq!(
        crate::local::git::run(Path::new(&cwd), &["ls-files"]).unwrap(),
        "Tracked.txt"
    );
    assert_eq!(read(&files, &cwd, "Tracked.txt").1, b"content");
    assert_eq!(read(&files, &cwd, "Untracked.txt").1, b"content");
}

#[cfg(unix)]
#[test]
fn downloadable_symlink_info_uses_canonical_target_and_requested_display_path() {
    let (_temp, files, cwd) = fixture();
    put(&cwd, "safe.txt");
    std::os::unix::fs::symlink("safe.txt", Path::new(&cwd).join("safe-link.txt")).unwrap();
    let (info, bytes) = read(&files, &cwd, "safe-link.txt");
    assert_eq!(info.path, "safe-link.txt");
    assert_eq!(info.file_name, "safe-link.txt");
    assert_eq!(
        info.absolute_path,
        Path::new(&cwd)
            .join("safe.txt")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
    );
    assert_eq!(bytes, b"content");
}

#[test]
fn duplicate_preserves_hidden_basenames_and_uses_collision_free_numbered_names() {
    let (_temp, files, cwd) = fixture();
    put(&cwd, ".env");
    put(&cwd, ".env copy");
    assert_eq!(files.duplicate(&cwd, ".env").unwrap(), ".env copy 2");
    assert_eq!(read(&files, &cwd, ".env copy 2").1, b"content");
    assert_eq!(read(&files, &cwd, ".env copy").1, b"content");
}

#[test]
fn rename_to_the_same_name_is_a_no_op_for_tracked_and_untracked_files() {
    let (_temp, files, cwd) = fixture();
    put(&cwd, "plain");
    assert_eq!(files.rename(&cwd, "plain", "plain").unwrap(), "plain");
    crate::local::git::run(Path::new(&cwd), &["init", "--quiet"]).unwrap();
    crate::local::git::run(Path::new(&cwd), &["add", "plain"]).unwrap();
    let before = crate::local::git::run(Path::new(&cwd), &["status", "--porcelain"]).unwrap();
    assert_eq!(files.rename(&cwd, "plain", "plain").unwrap(), "plain");
    assert_eq!(
        crate::local::git::run(Path::new(&cwd), &["status", "--porcelain"]).unwrap(),
        before
    );
}
