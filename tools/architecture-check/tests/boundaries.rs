use std::collections::BTreeMap;

use glade_architecture_check::check;
use serde_json::{Value, json};

fn fixture() -> (Value, Value, BTreeMap<String, String>) {
    let metadata = json!({"packages": [
        {"name": "reader-api", "dependencies": [], "targets": [
            {"kind": ["lib"], "src_path": "/api/src/lib.rs", "name": "reader_api"}
        ]},
        {"name": "reader-engine", "dependencies": [
            {"name": "reader-api", "kind": null, "rename": null}
        ], "targets": [
            {"kind": ["lib"], "src_path": "/engine/src/lib.rs", "name": "reader_engine"},
            {"kind": ["test"], "src_path": "/engine/tests/conformance.rs", "name": "conformance"}
        ]}
    ]});
    let policy = json!({"version": 1, "packages": {
        "reader-api": {"role": "contract", "dependencies": [],
            "traits": {"Reader": ["lookup"]}},
        "reader-engine": {"role": "implementation",
            "dependencies": ["normal:reader-api"],
            "implements": [{"package": "reader-api", "trait": "Reader", "type": "Engine"}],
            "conformance_tests": ["conformance"]}
    }});
    let sources = BTreeMap::from([
        ("/api/src/lib.rs".into(), "pub trait Reader { fn lookup(&self) -> bool; }".into()),
        ("/engine/src/lib.rs".into(), "pub struct Engine; impl reader_api::Reader for Engine { fn lookup(&self) -> bool { true } }".into()),
        ("/engine/tests/conformance.rs".into(), "#[test] fn obeys_contract() {}".into()),
    ]);
    (metadata, policy, sources)
}

fn errors(metadata: Value, policy: Value, sources: BTreeMap<String, String>) -> Vec<String> {
    check(metadata, policy, |path| {
        sources
            .get(&path.to_string_lossy().to_string())
            .cloned()
            .ok_or_else(|| "missing fixture".into())
    })
    .unwrap()
}

#[test]
fn valid_interface_implementation_and_conformance_target_pass() {
    let (m, p, s) = fixture();
    assert!(errors(m, p, s).is_empty());
}

#[test]
fn unclassified_library_fails_closed() {
    let (mut m, p, s) = fixture();
    m["packages"].as_array_mut().unwrap().push(json!({
        "name": "forgotten", "dependencies": [], "targets": [{"kind": ["lib"], "src_path": "/new/lib.rs", "name": "forgotten"}]
    }));
    assert!(errors(m, p, s).iter().any(|e| e.contains("ARCH-001")));
}

#[test]
fn normal_optional_target_build_and_dev_dependencies_are_checked_by_real_name() {
    for kind in [Value::Null, json!("build"), json!("dev")] {
        let (mut m, p, s) = fixture();
        m["packages"][0]["dependencies"].as_array_mut().unwrap().push(json!({
            "name": "iroh", "rename": "innocent", "kind": kind, "optional": true, "target": "cfg(windows)"
        }));
        assert!(errors(m, p, s).iter().any(|e| e.contains("ARCH-002")));
    }
}

#[test]
fn missing_private_empty_default_only_and_comment_traits_do_not_satisfy_contract() {
    for source in [
        "// pub trait Reader { fn lookup(&self) -> bool; }",
        "trait Reader { fn lookup(&self) -> bool; }",
        "pub trait Reader {}",
        "pub trait Reader { fn lookup(&self) -> bool { true } }",
        "#[cfg(test)] pub trait Reader { fn lookup(&self) -> bool; }",
        "pub mod hidden { pub trait Reader { fn lookup(&self) -> bool; } }",
    ] {
        let (m, p, mut s) = fixture();
        s.insert("/api/src/lib.rs".into(), source.into());
        assert!(
            errors(m, p, s).iter().any(|e| e.contains("ARCH-003")),
            "{source}"
        );
    }
}

#[test]
fn an_empty_trait_policy_is_not_an_exemption() {
    let (m, mut p, s) = fixture();
    p["packages"]["reader-api"]["traits"]["Reader"] = json!([]);
    assert!(errors(m, p, s).iter().any(|e| e.contains("ARCH-003")));
}

#[test]
fn implementation_must_implement_the_named_contract() {
    for source in [
        "pub struct Engine;",
        "pub struct Engine; impl Engine { pub fn lookup(&self) -> bool { true } }",
    ] {
        let (m, p, mut s) = fixture();
        s.insert("/engine/src/lib.rs".into(), source.into());
        assert!(errors(m, p, s).iter().any(|e| e.contains("ARCH-004")));
    }
}

#[test]
fn missing_contract_declaration_or_conformance_suite_fails() {
    for key in ["implements", "conformance_tests"] {
        let (m, mut p, s) = fixture();
        p["packages"]["reader-engine"][key] = json!([]);
        assert!(!errors(m, p, s).is_empty());
    }
    let (m, p, mut s) = fixture();
    s.insert(
        "/engine/tests/conformance.rs".into(),
        "// #[test] fn pretend() {}".into(),
    );
    assert!(errors(m, p, s).iter().any(|e| e.contains("ARCH-005")));
}

#[test]
fn contract_cannot_depend_on_implementation_even_if_allowlisted() {
    let (mut m, mut p, s) = fixture();
    m["packages"][0]["dependencies"] = json!([{"name": "reader-engine", "kind": null}]);
    p["packages"]["reader-api"]["dependencies"] = json!(["normal:reader-engine"]);
    assert!(errors(m, p, s).iter().any(|e| e.contains("ARCH-006")));
}

#[test]
fn exemptions_need_a_reason_and_stale_policy_entries_fail() {
    let (m, mut p, s) = fixture();
    p["packages"]["reader-api"]["role"] = json!("pure");
    assert!(errors(m, p, s).iter().any(|e| e.contains("ARCH-007")));
    let (m, mut p, s) = fixture();
    p["packages"]["no-longer-present"] =
        json!({"role": "pure", "reason": "removed", "dependencies": []});
    assert!(errors(m, p, s).iter().any(|e| e.contains("ARCH-001")));
}

#[test]
fn public_out_of_line_modules_are_followed_but_orphan_files_are_not() {
    let (m, mut p, mut s) = fixture();
    p["packages"]["reader-api"]["traits"] = json!({"ports::Reader": ["lookup"]});
    p["packages"]["reader-engine"]["implements"][0]["trait"] = json!("ports::Reader");
    s.insert("/api/src/lib.rs".into(), "pub mod ports;".into());
    s.insert(
        "/api/src/ports.rs".into(),
        "pub trait Reader { fn lookup(&self) -> bool; }".into(),
    );
    s.insert("/engine/src/lib.rs".into(), "pub struct Engine; impl reader_api::ports::Reader for Engine { fn lookup(&self) -> bool { true } }".into());
    assert!(errors(m.clone(), p.clone(), s.clone()).is_empty());
    s.insert(
        "/api/src/lib.rs".into(),
        "// unreferenced ports.rs does not count".into(),
    );
    assert!(errors(m, p, s).iter().any(|e| e.contains("ARCH-003")));
}

#[test]
fn malformed_policy_and_source_are_errors_not_silent_success() {
    let (m, mut p, _s) = fixture();
    p["packages"]["reader-api"]["role"] = json!("anything-goes");
    assert!(check(m, p, |_| Err("unused".into())).is_err());
    let (m, p, mut s) = fixture();
    s.insert("/api/src/lib.rs".into(), "pub trait {".into());
    assert!(!errors(m, p, s).is_empty());
}

#[test]
fn implementation_to_implementation_dependency_is_rejected_even_if_allowlisted() {
    let (mut m, mut p, s) = fixture();
    m["packages"][1]["dependencies"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "other-engine", "kind": null}));
    p["packages"]["reader-engine"]["dependencies"]
        .as_array_mut()
        .unwrap()
        .push(json!("normal:other-engine"));
    p["packages"]["other-engine"] = json!({"role": "implementation", "dependencies": []});
    assert!(errors(m, p, s).iter().any(|e| e.contains("ARCH-006")));
}

#[test]
fn ignored_tests_and_conditional_impls_are_not_evidence() {
    let (m, p, mut s) = fixture();
    s.insert(
        "/engine/tests/conformance.rs".into(),
        "#[test] #[ignore] fn disabled() {}".into(),
    );
    assert!(errors(m, p, s).iter().any(|e| e.contains("ARCH-005")));
    let (m, p, mut s) = fixture();
    s.insert("/engine/src/lib.rs".into(), "pub struct Engine; #[cfg(test)] impl reader_api::Reader for Engine { fn lookup(&self) -> bool { true } }".into());
    assert!(errors(m, p, s).iter().any(|e| e.contains("ARCH-004")));
}

#[test]
fn a_renamed_contract_dependency_can_be_implemented_explicitly() {
    let (mut m, p, mut s) = fixture();
    m["packages"][1]["dependencies"][0]["rename"] = json!("api");
    s.insert(
        "/engine/src/lib.rs".into(),
        "pub struct Engine; impl api::Reader for Engine { fn lookup(&self) -> bool { true } }"
            .into(),
    );
    assert!(errors(m, p, s).is_empty());
}
