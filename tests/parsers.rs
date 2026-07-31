use nr::events::{BuildCategory, BuildState, ParsedLine, parse_line};
use nr::impact::{parse_activation_impact, parse_closure_diff};

#[test]
fn parses_internal_json_without_failing_unknown_events() {
    let line = r#"@nix {"action":"start","id":7,"parent":1,"type":105,"text":"building '/nix/store/aaa-linux.drv'","fields":[]}"#;
    let ParsedLine::Event(event) = parse_line(line) else {
        panic!("expected event");
    };
    assert_eq!(event.action, "start");
    let mut state = BuildState::default();
    state.ingest(&event);
    assert_eq!(state.running.len(), 1);
    assert_eq!(
        state.running.get(&7).unwrap().category,
        BuildCategory::KernelBoot
    );

    let ParsedLine::Event(event) = parse_line(r#"@nix {"action":"future","id":1}"#) else {
        panic!("expected unknown event");
    };
    state.ingest(&event);
    assert_eq!(state.unknown_events, 1);
}

#[test]
fn broken_internal_json_requests_fallback() {
    assert!(matches!(
        parse_line("@nix {bad json"),
        ParsedLine::BrokenInternalJson(_)
    ));
}

#[test]
fn parses_closure_diff_summary() {
    let diff = parse_closure_diff(
        r#"
+ linux-6.10
- old-service-1.0
openssl: 3.0 -> 3.1
closure size: 1.0 GiB -> 1.1 GiB, +100.0 MiB
"#,
    );
    assert_eq!(diff.additions, vec!["linux-6.10".to_string()]);
    assert_eq!(diff.removals, vec!["old-service-1.0".to_string()]);
    assert_eq!(diff.upgrades, vec!["openssl: 3.0 -> 3.1".to_string()]);
    assert_eq!(
        diff.size_delta.as_deref(),
        Some("closure size: 1.0 GiB -> 1.1 GiB, +100.0 MiB")
    );
    assert!(diff.important.contains(&"linux-6.10".to_string()));
}

#[test]
fn parses_dry_activation_units() {
    let impact = parse_activation_impact(
        r#"
would stop the following units: old.service
would restart the following units: sshd.service display-manager.service
warning: user services are not handled by this dry activation
"#,
    );
    assert_eq!(impact.stopped, vec!["old.service".to_string()]);
    assert_eq!(
        impact.restarted,
        vec![
            "sshd.service".to_string(),
            "display-manager.service".to_string()
        ]
    );
    assert_eq!(impact.caveats.len(), 1);
}
