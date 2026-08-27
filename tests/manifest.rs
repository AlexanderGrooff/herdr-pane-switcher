use std::fs;
use std::path::Path;

fn manifest() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("herdr-plugin.toml")).unwrap()
}

#[test]
fn manifest_declares_rust_plugin_commands() {
    let text = manifest();
    assert!(text.contains("id = \"herdr.pane-switcher\""));
    assert!(text.contains("command = [\"bash\", \"herdr/install.sh\"]"));
    assert!(text.matches("herdr-mru-cycle").count() >= 5);
}

#[test]
fn repository_contains_no_removed_sources() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert_no_removed_sources(root);
}

fn assert_no_removed_sources(path: &Path) {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path
            .file_name()
            .is_some_and(|name| name == ".git" || name == "target" || name == "bin")
        {
            continue;
        }
        if path.is_dir() {
            assert_no_removed_sources(&path);
        } else {
            assert_ne!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("py"),
                "{}",
                path.display()
            );
        }
    }
}
