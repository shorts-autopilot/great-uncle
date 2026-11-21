//! The binary must load, model and snapshot with no display attached.

use std::process::Command;

#[test]
fn the_binary_builds_a_snapshot_without_a_display() {
    let output = Command::new(env!("CARGO_BIN_EXE_openlight-desktop"))
        .arg("--headless-smoke")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .expect("run openlight-desktop");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("OpenLight desktop"), "{text}");
    assert!(text.contains("sheet rows"), "{text}");
    assert!(text.contains("universes"), "{text}");
}
