use alloyos_welder::{
    knowledge::{KnowledgeBasis, KnowledgeCategory, KnowledgeEntry},
    NativeToolRegistry, ToolVfsBackend,
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

#[derive(Debug, Clone, Copy)]
enum Failure {
    StagingCorruption,
    CommitCorruption,
    CommitError,
}

#[derive(Debug, Default)]
struct Storage {
    files: BTreeMap<String, String>,
    failure: Option<Failure>,
}

impl ToolVfsBackend for Storage {
    fn read_file(&mut self, path: &str) -> Option<String> {
        self.files.get(path).cloned()
    }
    fn write_file(&mut self, path: &str, content: &str) -> Result<(), String> {
        let should_fail = match self.failure {
            Some(Failure::StagingCorruption) => path.ends_with(".tmp"),
            Some(_) => path == KnowledgeCategory::Preferences.path(),
            None => false,
        };
        if should_fail {
            let failure = self.failure.take().unwrap();
            // Model a backend that changed disk bytes before reporting failure, too.
            self.files.insert(path.into(), "incomplete write".into());
            if matches!(failure, Failure::CommitError) {
                return Err("injected I/O error".into());
            }
        } else {
            self.files.insert(path.into(), content.into());
        }
        Ok(())
    }
    fn delete_file(&mut self, path: &str) -> Result<bool, String> {
        Ok(self.files.remove(path).is_some())
    }
    fn list_dir(&mut self, dir: &str) -> Vec<String> {
        self.files
            .keys()
            .filter(|path| path.starts_with(&format!("{dir}/")))
            .cloned()
            .collect()
    }
}

fn preference(value: &str) -> KnowledgeEntry {
    KnowledgeEntry::new(
        KnowledgeCategory::Preferences,
        "Answer style",
        value,
        KnowledgeBasis::Explicit,
        "user:turn-1",
        "2026-10-15",
        "global",
    )
}

#[test]
fn failed_knowledge_updates_preserve_previous_record_after_reconstruction() {
    for failure in [
        Failure::StagingCorruption,
        Failure::CommitCorruption,
        Failure::CommitError,
    ] {
        let storage = Rc::new(RefCell::new(Storage::default()));
        let mut registry = NativeToolRegistry::with_vfs_backend(storage.clone());
        registry
            .knowledge_upsert(preference("concise answers"), None)
            .unwrap();
        let before = storage.borrow().files.clone();
        storage.borrow_mut().failure = Some(failure);
        assert!(
            registry
                .knowledge_upsert(preference("detailed answers"), None)
                .is_err(),
            "{failure:?}"
        );
        assert_eq!(
            storage.borrow().files,
            before,
            "failed update must preserve bytes and remove staging: {failure:?}"
        );

        let mut restored = NativeToolRegistry::with_vfs_backend(storage.clone());
        let entries = restored.knowledge_entries_for(&KnowledgeCategory::Preferences);
        assert_eq!(entries.len(), 1, "{failure:?}");
        assert_eq!(entries[0].value, "concise answers", "{failure:?}");
        // A failed update must not poison the next successful write.
        restored
            .knowledge_upsert(preference("detailed answers"), None)
            .unwrap();
        assert_eq!(
            restored.knowledge_entries_for(&KnowledgeCategory::Preferences)[0].value,
            "detailed answers"
        );
    }
}
