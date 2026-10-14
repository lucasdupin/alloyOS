//! UI-owned pin order and typed events. Persistence is supplied by the OS owner.
use alloc::{string::String, vec::Vec};
use alloyos_mcp::protocol::AppTile;

pub use alloyos_mcp::display::TileEvent;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DashboardState {
    pinned: Vec<String>,
    revision: u64,
}
impl DashboardState {
    pub fn pinned_ids(&self) -> &[String] {
        &self.pinned
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn restore(&mut self, ids: &[String]) {
        let mut unique = Vec::new();
        for id in ids.iter().take(64) {
            if !id.is_empty() && id.len() <= 128 && !unique.contains(id) {
                unique.push(id.clone());
            }
        }
        if self.pinned != unique {
            self.pinned = unique;
            self.revision = self.revision.saturating_add(1);
        }
    }
    pub fn pin(&mut self, tile_id: &str) -> bool {
        if tile_id.is_empty()
            || tile_id.len() > 128
            || self.pinned.len() >= 64
            || self.pinned.iter().any(|id| id == tile_id)
        {
            return false;
        }
        self.pinned.push(tile_id.into());
        self.revision = self.revision.saturating_add(1);
        true
    }
    pub fn unpin(&mut self, tile_id: &str) -> bool {
        let Some(index) = self.pinned.iter().position(|id| id == tile_id) else {
            return false;
        };
        self.pinned.remove(index);
        self.revision = self.revision.saturating_add(1);
        true
    }
    pub fn move_to(&mut self, tile_id: &str, index: usize) -> bool {
        let Some(old) = self.pinned.iter().position(|id| id == tile_id) else {
            return false;
        };
        let index = index.min(self.pinned.len() - 1);
        if old == index {
            return false;
        }
        let id = self.pinned.remove(old);
        self.pinned.insert(index, id);
        self.revision = self.revision.saturating_add(1);
        true
    }
    pub fn tiles(&self, available: &[AppTile]) -> Vec<AppTile> {
        self.pinned
            .iter()
            .filter_map(|id| available.iter().find(|tile| &tile.tile_id == id).cloned())
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pins_are_unique_ordered_and_restorable() {
        let mut state = DashboardState::default();
        assert!(!state.pin(&"x".repeat(129)));
        assert!(state.pin("sample_timer.now"));
        assert!(!state.pin("sample_timer.now"));
        assert!(state.pin("sample_scheduler.board"));
        assert!(state.move_to("sample_scheduler.board", 0));
        assert_eq!(state.pinned_ids()[0], "sample_scheduler.board");
        let revision = state.revision();
        assert!(!state.move_to("sample_scheduler.board", 0));
        assert_eq!(revision, state.revision());
        state.restore(&["sample_timer.now".into(), "sample_timer.now".into(), "".into()]);
        assert_eq!(state.pinned_ids(), &[String::from("sample_timer.now")]);
        assert!(state.unpin("sample_timer.now"));
        assert!(!state.unpin("sample_timer.now"));
    }
}
