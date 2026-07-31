use std::time::{Duration, Instant};

use nr::events::{
    Activity, ActivityStatus, BuildCategory, BuildState, ParsedLine, categorize,
    extract_derivation_paths, parse_line,
};
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
fn categorizes_modern_wayland_desktop_stack() {
    assert_eq!(categorize("building niri"), BuildCategory::DesktopStack);
    assert_eq!(
        categorize("building quickshell"),
        BuildCategory::DesktopStack
    );
    assert_eq!(categorize("building hyprland"), BuildCategory::DesktopStack);
    assert_eq!(
        categorize("building something-random"),
        BuildCategory::Other
    );
}

#[test]
fn slowest_active_uses_elapsed_time() {
    let now = Instant::now();
    let mut state = BuildState::default();
    let newer = Activity {
        id: 1,
        parent: None,
        text: "building newer".to_string(),
        drv_path: Some("/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-newer.drv".to_string()),
        category: BuildCategory::Other,
        source_build: true,
        substitute: false,
        status: ActivityStatus::Running,
        started_at: now - Duration::from_secs(1),
    };
    let older = Activity {
        id: 2,
        parent: None,
        text: "building older".to_string(),
        drv_path: Some("/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-older.drv".to_string()),
        category: BuildCategory::Other,
        source_build: true,
        substitute: false,
        status: ActivityStatus::Running,
        started_at: now - Duration::from_secs(10),
    };
    state.running.insert(newer.id, newer);
    state.running.insert(older.id, older);

    assert_eq!(state.slowest_active().map(|activity| activity.id), Some(2));
}

#[test]
fn extracts_derivation_paths_from_plain_nix_output() {
    let paths = extract_derivation_paths(
        "  /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-nixos-system-host.drv",
    );

    assert_eq!(
        paths,
        vec!["/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-nixos-system-host.drv".to_string()]
    );
}

#[test]
fn dependency_graph_finds_path_from_system_derivation_to_active_drv() {
    let mut state = BuildState::default();
    state.note_derivation_paths_from_text(
        "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-nixos-system-host.drv",
    );
    state.note_derivation_graph(
        "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-nixos-system-host.drv",
        r#"
digraph G {
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-nixos-system-host.drv" [label = "nixos-system-host"];
"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-etc.drv" [label = "etc"];
"cccccccccccccccccccccccccccccccc-man-cache.drv" [label = "man-cache"];
"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-etc.drv" -> "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-nixos-system-host.drv";
"cccccccccccccccccccccccccccccccc-man-cache.drv" -> "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-etc.drv";
}
"#,
    );

    let path = state
        .dependency_graph
        .active_path(&["/nix/store/cccccccccccccccccccccccccccccccc-man-cache.drv".to_string()])
        .unwrap();

    assert_eq!(path.len(), 3);
    assert_eq!(
        state.dependency_graph.label(path.last().unwrap()),
        "man-cache"
    );
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
