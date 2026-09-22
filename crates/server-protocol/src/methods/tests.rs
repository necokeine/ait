use std::collections::{BTreeMap, BTreeSet};

use super::{InboundKind, PASEO_METHODS, by_canonical_name, by_paseo_name};

#[test]
fn source_names_are_unique_and_canonical_names_are_well_formed() {
    let mut sources = BTreeSet::new();
    for method in PASEO_METHODS {
        assert!(sources.insert(method.paseo_name), "{}", method.paseo_name);
        assert!(!method.canonical_name.contains('/'), "{method:?}");
        assert!(!method.canonical_name.starts_with('.'), "{method:?}");
        assert!(!method.canonical_name.ends_with('.'), "{method:?}");
        assert!(!method.canonical_name.contains(".."), "{method:?}");
        assert!(
            method
                .canonical_name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || b"._".contains(&byte)),
            "{method:?}"
        );
        match method.kind {
            InboundKind::Request => assert!(
                method.canonical_name.ends_with(".request")
                    || method.canonical_name == "connection.ping",
                "{method:?}"
            ),
            InboundKind::Response => {
                assert!(method.canonical_name.ends_with(".response"), "{method:?}");
            }
            InboundKind::Event => assert!(
                !method.canonical_name.ends_with(".request")
                    && !method.canonical_name.ends_with(".response"),
                "{method:?}"
            ),
        }
    }
}

#[test]
fn every_canonical_collision_is_an_intentional_legacy_merge() {
    let mut sources_by_canonical = BTreeMap::<&str, Vec<&str>>::new();
    for method in PASEO_METHODS {
        sources_by_canonical
            .entry(method.canonical_name)
            .or_default()
            .push(method.paseo_name);
    }
    let collisions = sources_by_canonical
        .into_iter()
        .filter(|(_, sources)| sources.len() > 1)
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        collisions,
        BTreeMap::from([
            (
                "agent.create.request",
                vec!["agent.create.request", "create_agent_request"]
            ),
            (
                "project.icon.get.request",
                vec!["project.icon.get.request", "project_icon_request"]
            ),
            (
                "workspace.script.start.request",
                vec![
                    "workspace.script.start.request",
                    "start_workspace_script_request"
                ]
            ),
        ])
    );
}

#[test]
fn canonicalizes_representative_legacy_names_without_accepting_them_as_wire_names() {
    let cases = [
        ("read_project_config_request", "project.config.read.request"),
        ("fetch_workspaces_request", "workspace.list.request"),
        ("checkout_status_request", "checkout.status.get.request"),
        ("schedule/run-once", "schedule.run_once.request"),
        ("set_voice_mode", "voice.mode.set.request"),
    ];
    for (source, canonical) in cases {
        assert_eq!(
            by_paseo_name(source).map(|method| method.canonical_name),
            Some(canonical)
        );
        assert_eq!(
            by_canonical_name(canonical).map(|method| method.canonical_name),
            Some(canonical)
        );
        assert_eq!(by_canonical_name(source), None);
    }
}
