//! Filesystem fixtures for platform tests. The production OS has no embedded apps.
#![allow(dead_code)]
extern crate std;
use std::{
    collections::BTreeMap,
    format,
    path::{Path, PathBuf},
    string::String,
};

pub fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/apps")
}

pub fn snapshot() -> alloyos_mcp::PackageSnapshot {
    fn walk(path: &Path, root: &Path, result: &mut alloyos_mcp::PackageSnapshot) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let target = format!("/apps/{}", path.strip_prefix(root).unwrap().display());
            if path.is_dir() {
                result.directories.push(target);
                walk(&path, root, result);
            } else {
                result
                    .files
                    .insert(target, std::fs::read_to_string(path).unwrap());
            }
        }
    }
    let mut result = alloyos_mcp::PackageSnapshot::default();
    let root = fixture_root();
    walk(&root, &root, &mut result);
    result
}

pub fn register_test_apps(
    registry: &mut alloyos_mcp::McpRegistry,
) -> Result<(), alloyos_mcp::McpSandboxError> {
    let scheduler = std::rc::Rc::new(std::cell::RefCell::new(alloyos_js::ScriptScheduler::new()));
    registry.discover_and_register_packages(&snapshot(), Some(scheduler))?;
    Ok(())
}

pub fn files() -> BTreeMap<String, String> {
    snapshot().files
}
