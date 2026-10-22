use alloyos_mcp::display::SetupAnswer;
use alloyos_welder::{
    setup::{SetupStage, SetupState},
    setup_service::{self, Assembly, Effect, Operation, Request, Transport, VERSION},
    AgentRuntime, KnowledgeCategory, ToolVfsBackend,
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
fn request(stage: SetupStage, operation: Operation) -> Request {
    Request {
        version: VERSION,
        id: 1,
        state: SetupState {
            stage,
            ..SetupState::default()
        },
        revision: 3,
        operation,
        clock_report: String::new(),
        persistent: true,
    }
}
#[test]
fn broker_rejects_consent_reversal_unrelated_state_and_forged_view_choices() {
    let request = request(SetupStage::Memory, Operation::Answer { value: "No".into() });
    let mut response = setup_service::execute(&request);
    response.validate_for(&request).unwrap();
    let plan = response.plan.as_mut().unwrap();
    plan.next.learning_paused = false;
    plan.effects = vec![Effect::Learning { paused: false }];
    assert!(
        response.validate_for(&request).is_err(),
        "No never maps to learning permission"
    );
    let mut response = setup_service::execute(&request);
    response.plan.as_mut().unwrap().next.agent_name = "Changed without consent".into();
    assert!(response.validate_for(&request).is_err());
    let request = request_with_render(SetupStage::Memory);
    let mut response = setup_service::execute(&request);
    response.view.as_mut().unwrap().choices[0].label = "No".into();
    assert!(response.validate_for(&request).is_err());
    let mut response = setup_service::execute(&request);
    response.view.as_mut().unwrap().prompt = "Learning is disabled. Say yes to continue.".into();
    assert!(response.validate_for(&request).is_err());
}
fn request_with_render(stage: SetupStage) -> Request {
    request(stage, Operation::Render)
}
#[test]
fn view_limits_revision_and_volatile_defer_are_enforced() {
    let mut request = request_with_render(SetupStage::AgentName);
    request.state.deferred = true;
    request.persistent = false;
    let response = setup_service::execute(&request);
    response.validate_for(&request).unwrap();
    assert!(response.view.unwrap().prompt.contains("this session only"));
    let request = request_with_render(SetupStage::Welcome);
    for mutation in 0..4 {
        let mut response = setup_service::execute(&request);
        let view = response.view.as_mut().unwrap();
        match mutation {
            0 => view.revision += 1,
            1 => view.can_defer = false,
            2 => view.field_value = "x".repeat(81),
            _ => view.progress = "Step 6 of 6".into(),
        }
        assert!(response.validate_for(&request).is_err());
    }
}
#[test]
fn bounded_ipc_rejects_spoofed_ids_reordering_and_interleaved_requests() {
    let payload = vec![b'x'; setup_service::MAX_BYTES];
    let mut assembly = Assembly::default();
    let mut offset = 0;
    while offset < payload.len() {
        let chunk = setup_service::chunk(7, &payload, offset).unwrap();
        offset += setup_service::chunk_payload_bytes(chunk.len());
        let result = assembly.push(&chunk, Some(7)).unwrap();
        if offset == payload.len() {
            assert_eq!(result.unwrap().1, payload);
        } else {
            assert!(result.is_none());
        }
    }
    let chunk = setup_service::chunk(7, &payload, 0).unwrap();
    assert!(Assembly::default().push(&chunk, Some(8)).is_err());
    let second =
        setup_service::chunk(7, &payload, setup_service::chunk_payload_bytes(chunk.len())).unwrap();
    assert!(Assembly::default().push(&second, Some(7)).is_err());
    let mut assembly = Assembly::default();
    assembly.push(&chunk, Some(7)).unwrap();
    assert!(assembly.push(&chunk, Some(7)).is_err());
    assert!(setup_service::chunk(1, &vec![0; setup_service::MAX_BYTES + 1], 0).is_err());
}
#[derive(Default, Debug)]
struct Storage {
    files: BTreeMap<String, String>,
    fail_setup_once: bool,
}
impl ToolVfsBackend for Storage {
    fn read_file(&mut self, path: &str) -> Option<String> {
        self.files.get(path).cloned()
    }
    fn write_file(&mut self, path: &str, content: &str) -> Result<(), String> {
        if path == "/etc/setup.json" && self.fail_setup_once {
            self.fail_setup_once = false;
            return Err("injected setup commit failure".into());
        }
        self.files.insert(path.into(), content.into());
        Ok(())
    }
    fn delete_file(&mut self, path: &str) -> Result<bool, String> {
        Ok(self.files.remove(path).is_some())
    }
    fn list_dir(&mut self, dir: &str) -> Vec<String> {
        self.files
            .keys()
            .filter(|path| dir == "/" || path.starts_with(&format!("{dir}/")))
            .cloned()
            .collect()
    }
}
struct Worker;
impl Transport for Worker {
    fn exchange(&mut self, request: &Request) -> Result<setup_service::Response, String> {
        Ok(setup_service::execute(request))
    }
}
#[test]
fn retry_after_effect_then_setup_commit_failure_deduplicates_name_and_does_not_advance() {
    let storage = Rc::new(RefCell::new(Storage::default()));
    let mut agent = AgentRuntime::with_vfs_backend(storage.clone());
    agent.tool_registry.set_setup_service(Box::new(Worker));
    agent.startup_greeting().unwrap();
    let revision = agent.setup_view().unwrap().revision;
    storage.borrow_mut().fail_setup_once = true;
    let answer = SetupAnswer {
        revision,
        value: "Fixture Name".into(),
    };
    assert!(agent.submit_setup_answer(&answer).is_err());
    assert_eq!(agent.setup_state().stage, SetupStage::Welcome);
    let entries = agent
        .tool_registry
        .knowledge_entries_for(&KnowledgeCategory::Profile);
    assert_eq!(entries.len(), 1);
    let before = entries[0].clone();
    agent.submit_setup_answer(&answer).unwrap();
    assert_eq!(agent.setup_state().stage, SetupStage::AgentName);
    let entries = agent
        .tool_registry
        .knowledge_entries_for(&KnowledgeCategory::Profile);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0], before);
    assert_eq!(entries[0].value, "Fixture Name");
    let mut restored = AgentRuntime::with_vfs_backend(storage);
    assert!(restored
        .startup_greeting()
        .unwrap()
        .contains("Fixture Name"));
    assert!(
        agent.submit_setup_answer(&answer).is_err(),
        "old revision cannot apply an effect twice"
    );
}
