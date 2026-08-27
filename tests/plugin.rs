#[path = "../src/test_support.rs"]
mod support;

use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BINARY: &str = env!("CARGO_BIN_EXE_herdr-mru-cycle");

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("herdr-{label}-{}-{nonce}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run_action(
    server: &support::MockHerdrServer,
    state_dir: &Path,
    action: &str,
    pane_id: Option<&str>,
) {
    let env = support::action_env(
        support::base_env(server.socket_path(), state_dir),
        action,
        pane_id,
    );
    let output = support::run_plugin(Path::new(BINARY), &env).unwrap();
    assert!(
        output.status.success(),
        "{action} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_event(server: &support::MockHerdrServer, state_dir: &Path, event: &str, pane_id: &str) {
    let env = support::event_env(
        support::base_env(server.socket_path(), state_dir),
        event,
        &json!({"pane_id": pane_id}),
    );
    let output = support::run_plugin(Path::new(BINARY), &env).unwrap();
    assert!(
        output.status.success(),
        "{event} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn actions_focus_expected_panes() {
    let root = TempDir::new("actions");
    let panes = support::generate_panes(5);
    let server = support::MockHerdrServer::new(
        root.path().join("herdr.sock"),
        panes.clone(),
        Some("pane-0000".into()),
    )
    .unwrap();

    run_action(
        &server,
        &root.path().join("cycle"),
        "cycle",
        Some("pane-0000"),
    );
    assert_eq!(server.focus_calls(), ["pane-0001"]);
    server.clear_focus_calls();

    run_action(
        &server,
        &root.path().join("focus-attention"),
        "focus-attention",
        Some("pane-0003"),
    );
    assert_eq!(server.focus_calls(), ["pane-0000"]);
    server.clear_focus_calls();

    run_action(
        &server,
        &root.path().join("cycle-attention"),
        "cycle-attention",
        Some("pane-0000"),
    );
    assert_eq!(server.focus_calls(), ["pane-0004"]);
}

#[test]
fn events_update_state_without_focusing() {
    let root = TempDir::new("events");
    let panes = support::generate_panes(5);
    let server = support::MockHerdrServer::new(
        root.path().join("herdr.sock"),
        panes,
        Some("pane-0000".into()),
    )
    .unwrap();

    run_event(
        &server,
        &root.path().join("state"),
        "pane.focused",
        "pane-0002",
    );
    run_event(
        &server,
        &root.path().join("state"),
        "pane.closed",
        "pane-0001",
    );

    assert!(server.focus_calls().is_empty());
}

#[test]
fn repeated_cycle_continues_through_mru_order() {
    let root = TempDir::new("repeat");
    let panes = support::generate_panes(5);
    let server = support::MockHerdrServer::new(
        root.path().join("herdr.sock"),
        panes,
        Some("pane-0000".into()),
    )
    .unwrap();
    let state_dir = root.path().join("state");
    let mut current = "pane-0000";

    for expected in ["pane-0001", "pane-0002", "pane-0003"] {
        run_action(&server, &state_dir, "cycle", Some(current));
        assert_eq!(server.focus_calls(), [expected]);
        server.clear_focus_calls();
        current = expected;
    }
}

#[test]
fn cycle_resets_after_timeout() {
    let root = TempDir::new("timeout");
    let server = support::MockHerdrServer::new(
        root.path().join("herdr.sock"),
        support::generate_panes(5),
        Some("pane-0000".into()),
    )
    .unwrap();
    let state_dir = root.path().join("state");

    run_action(&server, &state_dir, "cycle", Some("pane-0000"));
    assert_eq!(server.focus_calls(), ["pane-0001"]);
    server.clear_focus_calls();
    thread::sleep(Duration::from_millis(1_200));

    run_action(&server, &state_dir, "cycle", Some("pane-0001"));
    assert_eq!(server.focus_calls(), ["pane-0000"]);
}

#[test]
fn closed_and_disappeared_panes_are_excluded() {
    let root = TempDir::new("removed");
    let panes = support::generate_panes(5);
    let server = support::MockHerdrServer::new(
        root.path().join("herdr.sock"),
        panes.clone(),
        Some("pane-0000".into()),
    )
    .unwrap();
    let state_dir = root.path().join("state");

    run_event(&server, &state_dir, "pane.closed", "pane-0001");
    let remaining = panes
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != 1)
        .map(|(_, pane)| pane.clone())
        .collect();
    server.set_panes(remaining);
    run_action(&server, &state_dir, "cycle", Some("pane-0000"));
    assert_eq!(server.focus_calls(), ["pane-0002"]);
    server.clear_focus_calls();

    server.set_panes(panes.clone());
    let disappear_state = root.path().join("disappear");
    run_action(&server, &disappear_state, "cycle", Some("pane-0000"));
    assert_eq!(server.focus_calls(), ["pane-0001"]);
    server.clear_focus_calls();

    server.set_panes(panes[..4].to_vec());
    run_action(&server, &disappear_state, "cycle", Some("pane-0001"));
    assert_eq!(server.focus_calls(), ["pane-0000"]);
}
