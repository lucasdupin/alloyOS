use alloyos_welder::{AgentRuntime, ToolVfsBackend};
use std::{cell::RefCell, rc::Rc};

#[derive(Debug)]
struct FailedStorage;
impl ToolVfsBackend for FailedStorage {
    fn read_file(&mut self, _: &str) -> Option<String> {
        None
    }
    fn write_file(&mut self, _: &str, _: &str) -> Result<(), String> {
        Err("disk write failed".into())
    }
    fn delete_file(&mut self, _: &str) -> Result<bool, String> {
        Ok(false)
    }
    fn list_dir(&mut self, _: &str) -> Vec<String> {
        Vec::new()
    }
}

#[test]
fn failed_pin_write_keeps_previous_dashboard() {
    let mut runtime = AgentRuntime::with_vfs_backend(Rc::new(RefCell::new(FailedStorage)));
    let previous = runtime.pinned_tile_ids().to_vec();
    assert!(runtime.set_pinned_tile_ids(&["fixture.now".into()]).is_err());
    assert_eq!(runtime.pinned_tile_ids(), previous);
}
