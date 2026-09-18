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
