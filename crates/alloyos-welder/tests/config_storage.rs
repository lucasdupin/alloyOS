use alloyos_welder::{AgentRuntime, SharedMapVfsBackend, ToolVfsBackend};
use std::{cell::RefCell, rc::Rc};

#[test]
fn model_and_setup_acknowledgements_distinguish_durable_and_session_storage() {
    for persistent in [false, true] {
        let storage=Rc::new(RefCell::new(if persistent {SharedMapVfsBackend::new()} else {SharedMapVfsBackend::new_volatile()}));
        let mut agent=AgentRuntime::with_vfs_backend(storage.clone());
        let model=agent.handle_console_input("/model gemini",&mut |_|{}).unwrap();
        assert_eq!(model.config_persisted,persistent);
        assert_eq!(model.response_text.contains("session only"),!persistent);
        assert!(storage.borrow().files["/etc/ai.conf"].contains("provider=gemini"));
        for command in ["/knowledge pause", "/knowledge resume"] {
            let result=agent.handle_console_input(command,&mut |_|{}).unwrap();
            assert_eq!(result.config_persisted,persistent);
            assert_eq!(result.response_text.contains("session only"),!persistent);
        }
        let setup=agent.handle_console_input("/setup",&mut |_|{}).unwrap();
        assert_eq!(setup.config_persisted,persistent);
        assert_eq!(setup.response_text.contains("storage is volatile"),!persistent);
    }
}

#[derive(Debug)]
struct BrokenDisk;
impl ToolVfsBackend for BrokenDisk {
    fn read_file(&mut self,_:&str)->Option<String>{None}
    fn write_file(&mut self,_:&str,_:&str)->Result<(),String>{Err("fixture write failure".into())}
    fn delete_file(&mut self,_:&str)->Result<bool,String>{Ok(false)}
    fn list_dir(&mut self,_:&str)->Vec<String>{vec![]}
}
#[test]
fn failed_model_configuration_write_never_acknowledges_persistence() {
    let mut agent=AgentRuntime::with_vfs_backend(Rc::new(RefCell::new(BrokenDisk)));
    assert!(agent.handle_console_input("/model gemini",&mut |_|{}).is_err());
    for paused in [false, true] {
        agent.set_learning_paused(paused);
        for command in ["/knowledge pause", "/knowledge resume"] {
            assert!(agent.handle_console_input(command,&mut |_|{}).is_err());
            assert_eq!(agent.is_learning_paused(), paused);
        }
    }
}
