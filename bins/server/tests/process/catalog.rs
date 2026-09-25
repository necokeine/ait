use std::collections::{BTreeMap, BTreeSet};

use server_protocol::methods::PASEO_METHODS;

use super::{TOKEN, ready, start};

#[tokio::test]
async fn production_installs_every_in_scope_method_without_placeholders() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("server.log");
    let mut server = start(&root.path().join("state"), &log);
    let address = ready(&mut server, &log).await;
    let info: server_protocol::ServerInfo = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{address}/v1/server/info"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let published = info.capabilities.into_iter().collect::<BTreeSet<_>>();
    let implemented = info
        .implemented_capabilities
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_eq!(published.len(), 175);
    assert_eq!(implemented.len(), 175);
    assert!(implemented.is_subset(&published));
    assert!(!published.iter().any(|name| {
        ["hub.", "chat.", "loop.", "plugin."]
            .iter()
            .any(|prefix| name.starts_with(prefix))
    }));
    for retired in [
        "project.open",
        "project.list",
        "project.get",
        "project.close",
    ] {
        assert!(!published.contains(retired));
        assert!(!implemented.contains(retired));
    }
    let catalog = PASEO_METHODS
        .iter()
        .map(|spec| (spec.canonical_name, spec.kind))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(catalog.len(), 168);
    assert!(catalog.keys().all(|method| published.contains(*method)));
    let placeholders = catalog
        .into_iter()
        .filter(|(method, _)| !implemented.contains(*method))
        .collect::<Vec<_>>();
    assert_eq!(placeholders.len(), 0);
    assert_eq!(published, implemented);
}
