use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

pub(super) struct NativeFixture {
    pub root: tempfile::TempDir,
    pub cwd: PathBuf,
    pub path: OsString,
}

impl NativeFixture {
    pub(super) fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let native = root.path().join("codex");
        std::fs::write(
            &native,
            include_str!("../../../../crates/server-provider/tests/fixtures/codex_app_server.py"),
        )
        .unwrap();
        std::fs::set_permissions(&native, std::fs::Permissions::from_mode(0o700)).unwrap();
        let cwd = root.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        let mut paths = vec![root.path().to_path_buf()];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        let path = std::env::join_paths(paths).unwrap();
        Self { root, cwd, path }
    }
}
