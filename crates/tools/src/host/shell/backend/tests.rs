use super::*;
use crate::host::{HostTools, Workers, open_project_root};
use ait_domain::RunPermissionProfile;
use ait_ports::RunTool;
use std::{os::unix::fs::PermissionsExt, sync::Arc};

#[test]
fn existing_but_unusable_backends_are_not_advertised() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let binary = root.join("sandbox-backend");
    // Model the three failure modes independently: no execute permission,
    // namespace/policy startup failure, and a backend that never starts.
    for (permissions, script) in [
        (0o600, "#!/bin/sh\nexit 0\n"),
        (0o700, "#!/bin/sh\nexit 1\n"),
        (0o700, "#!/bin/sh\nexec /bin/sleep 30\n"),
    ] {
        std::fs::write(&binary, script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(permissions)).unwrap();
        assert!(binary.is_file());
        for sandbox in [SandboxAccess::ReadOnly, SandboxAccess::WorkspaceWrite] {
            let start = Instant::now();
            let backend = ShellBackend::probe(&root, sandbox, &binary, Duration::from_millis(100));
            assert!(start.elapsed() < Duration::from_secs(3));
            let tools = HostTools {
                root: Arc::new(open_project_root(&root).unwrap()),
                root_path: root.clone(),
                authority: None,
                profile: RunPermissionProfile {
                    sandbox,
                    ..Default::default()
                },
                shell_backend: backend,
                observer: None,
                workers: Arc::new(Workers::default()),
            };
            assert!(!tools.executable_tools().contains(&"bash".to_owned()));
            assert!(tools.executable_tools().contains(&"read".to_owned()));
            assert!(tools.shell_command(&root).is_err());
        }
    }
}
