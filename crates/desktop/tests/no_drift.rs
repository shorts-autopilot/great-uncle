//! Structural guard: the window must not be able to grow a second engine.
//!
//! It only sees `openlight-app`; the engine, renderer, playback and timecode
//! modules are not dependencies at all, so the compiler enforces what the
//! architecture asks for.

use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn the_desktop_crate_only_depends_on_the_app() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let dependencies = manifest
        .split("[dependencies]")
        .nth(1)
        .expect("a [dependencies] section")
        .split('[')
        .next()
        .unwrap();
    for forbidden in ["openlight-engine", "openlight-dmx", "openlight-remote"] {
        assert!(
            !dependencies.contains(forbidden),
            "the desktop must not depend on {forbidden}: {dependencies}"
        );
    }
    assert!(
        dependencies.contains("openlight-app"),
        "the desktop should build on the shared app crate: {dependencies}"
    );

    let mut stack: Vec<PathBuf> = vec![root.join("src")];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
                continue;
            }
            let text = fs::read_to_string(&path).unwrap();
            for forbidden in [
                "openlight_engine",
                "engine::render",
                "engine::playback",
                "engine::timecode",
            ] {
                assert!(
                    !text.contains(forbidden),
                    "{} reaches around openlight-app with `{forbidden}`",
                    path.display()
                );
            }
        }
    }
}
