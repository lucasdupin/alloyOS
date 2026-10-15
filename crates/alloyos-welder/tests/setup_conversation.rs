use alloyos_forge::{
    cloud::{ApiProvider, HttpRequest, HttpResponse, HttpsClient},
    Forge, ForgeError,
};
use alloyos_welder::{AgentRuntime, SharedMapVfsBackend};
use std::{cell::RefCell, rc::Rc, task::Poll};

struct SetupModel {
    step: usize,
}
impl HttpsClient for SetupModel {
    fn post(&mut self, request: &HttpRequest) -> Result<HttpResponse, ForgeError> {
        assert!(request.body.contains("setup_answer"));
        assert!(request.body.contains("Interpret the user's meaning"));
        let reply = match self.step {
            0 => {
                assert!(request.body.contains("you can call me Lucas"));
                r#"{"kind":"tools","calls":[{"name":"setup_answer","arguments":{"question_id":"1","value":"Lucas"}}]}"#
            }
            1 => {
                assert!(request.body.contains("Nice to meet you, Lucas!"));
                r#"{"kind":"final","text":"Nice to meet you, Lucas! What would you like to call me?"}"#
            }
            2 => {
                r#"{"kind":"final","text":"You can choose any name you like. What would you prefer?"}"#
            }
            3 => {
                assert!(request.body.contains("call yourself Nova"));
                r#"{"kind":"tools","calls":[{"name":"setup_answer","arguments":{"question_id":"2","value":"Nova"}}]}"#
            }
            4 => {
                r#"{"kind":"final","text":"Nova it is. What city and country are you in?"}"#
            }
            _ => panic!("unexpected model turn"),
        };
        self.step += 1;
        Ok(HttpResponse {status:200,body:serde_json::json!({"candidates":[{"content":{"parts":[{"text":reply}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":10,"totalTokenCount":20}}).to_string()})
    }
}
fn finish(agent: &mut AgentRuntime) -> alloyos_welder::ConsoleTurnOutput {
    for _ in 0..100 {
        if let Poll::Ready(result) = agent.poll_console_reply() {
            return result.unwrap();
        }
    }
    panic!("setup model did not finish");
}
#[test]
fn model_interprets_conversation_before_setup_persists_answers() {
    let storage = SharedMapVfsBackend::shared();
    let forge = Rc::new(RefCell::new(Forge::new(0)));
    forge
        .borrow_mut()
        .connect_api(
            ApiProvider::Gemini,
            ApiProvider::Gemini.default_model(),
            "fixture",
            Box::new(SetupModel { step: 0 }),
        )
        .unwrap();
    let mut agent = AgentRuntime::with_vfs_backend(storage.clone());
    agent.set_forge(forge);
    agent.switch_provider("gemini").unwrap();
    let opening = agent.startup_greeting().unwrap();
    assert!(!opening.contains("Alloy"));
    assert!(agent
        .start_console_input("you can call me Lucas")
        .unwrap()
        .is_none());
    assert!(
        agent
            .setup_view()
            .unwrap()
            .prompt
            .contains("What should I call you?"),
        "raw text must not advance setup before model execution"
    );
    let reply = finish(&mut agent);
    assert!(reply.response_text.contains("Lucas"));
    assert!(!reply.response_text.contains("you can call me Lucas"));
    let revision = agent.setup_view().unwrap().revision;
    assert!(agent
        .start_console_input("What kind of name can I choose?")
        .unwrap()
        .is_none());
    finish(&mut agent);
    assert_eq!(
        agent.setup_view().unwrap().revision,
        revision,
        "clarification must not save a name or advance setup"
    );
    assert!(agent
        .start_console_input("call yourself Nova")
        .unwrap()
        .is_none());
    finish(&mut agent);
    assert_eq!(agent.agent_name(), "Nova");
    let mut restored = AgentRuntime::with_vfs_backend(storage);
    assert_eq!(restored.agent_name(), "Nova");
    assert!(restored.startup_greeting().unwrap().contains("clock"));
    assert!(restored
        .tool_registry
        .knowledge_bounded_summary(4096)
        .contains("Lucas"));
    assert!(!restored
        .tool_registry
        .knowledge_bounded_summary(4096)
        .contains("you can call me"));
}

#[test]
fn unavailable_model_does_not_save_raw_setup_text() {
    let mut agent = AgentRuntime::new();
    agent.startup_greeting().unwrap();
    let opening = agent.setup_view().unwrap().clone();
    let result = agent.start_console_input("you can call me Lucas");
    if matches!(result, Ok(None)) {
        for _ in 0..100 {
            if agent.poll_console_reply().is_ready() {
                break;
            }
        }
    }
    assert_eq!(agent.setup_view().unwrap(), &opening);
    assert!(!agent
        .tool_registry
        .knowledge_bounded_summary(4096)
        .contains("Lucas"));
}

#[test]
fn voice_setup_uses_same_validated_answer_tool() {
    let mut agent = AgentRuntime::new();
    agent.startup_greeting().unwrap();
    agent.record_voice_turn("you can call me Lucas", "");
    assert!(!agent
        .tool_registry
        .knowledge_bounded_summary(4096)
        .contains("Lucas"));
    let question = agent.setup_view().unwrap().revision.to_string();
    let call = alloyos_forge::provider::ToolCall::new(
        "voice-name",
        "setup_answer",
        serde_json::json!({"question_id":question,"value":"Lucas"}).to_string(),
    );
    let Poll::Ready(Ok(output)) = agent.poll_voice_tool(&call) else {
        panic!("voice setup tool failed");
    };
    assert!(output.synthesized_view.is_none());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output.output_json).unwrap()["ok"],
        true
    );
    assert!(agent
        .setup_view()
        .unwrap()
        .prompt
        .contains("Nice to meet you, Lucas!"));
    let stale = agent.poll_voice_tool(&call);
    let Poll::Ready(Ok(output)) = stale else {
        panic!("missing validation result");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output.output_json).unwrap()["ok"],
        false
    );
    assert_eq!(
        agent.agent_name(),
        "Alloy",
        "stale answer must not overwrite agent name"
    );
}
