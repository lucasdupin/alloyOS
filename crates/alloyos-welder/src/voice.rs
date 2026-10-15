//! Voice uses the same Welder context and tool inventory as typed conversation.
use crate::AgentRuntime;
use alloc::{
    string::{String, ToString},
    vec,
    vec::Vec,
};
use alloyos_forge::{
    provider::{AiError, Role},
    voice::LiveSession,
};
use serde_json::{json, Value};

impl AgentRuntime {
    pub fn record_voice_tool(
        &mut self,
        call: &alloyos_forge::provider::ToolCall,
        response: &Value,
    ) {
        self.history
            .push(alloyos_forge::provider::Message::assistant_with_tools(
                "",
                vec![call.clone()],
            ));
        self.history
            .push(alloyos_forge::provider::Message::tool_result(
                &call.id,
                response.to_string(),
            ));
    }

    pub fn start_voice_session(&mut self, id: u64, model: &str) -> Result<LiveSession, AiError> {
        if self.active_provider_name() != "gemini" {
            return Err(AiError::PolicyViolation(
                "Voice currently requires Gemini. Select Gemini to start a voice session.".into(),
            ));
        }
        let context = self.voice_context()?;
        let mut instruction = String::new();
        for message in context.iter().filter(|m| m.role == Role::System) {
            if instruction.len() + message.content.len() < 32768 {
                instruction.push_str(&message.content);
                instruction.push('\n');
            }
        }
        let declarations: Vec<Value> = self.tool_registry.specs().iter().map(|tool| {
            let mut properties = serde_json::Map::new();
            let mut required = Vec::new();
            for parameter in &tool.parameters {
                properties.insert(parameter.name.clone(), json!({"type":parameter.param_type.to_ascii_uppercase(),"description":parameter.description}));
                if parameter.required { required.push(parameter.name.clone()); }
            }
            json!({"name":tool.name,"description":tool.description,"parameters":{"type":"OBJECT","properties":properties,"required":required}})
        }).collect();
        let mut live = LiveSession::gemini(
            id,
            model,
            &instruction,
            vec![json!({"functionDeclarations":declarations})],
        )
        .map_err(|_| AiError::PolicyViolation("Voice setup exceeds provider limits".into()))?;
        // Bound reconnect context and keep tools/results out of prose. Live tools
        // receive their own structured results over the live session.
        let mut used = 0;
        let mut turns: Vec<Value> = context.iter().rev().filter_map(|message| {
            if !matches!(message.role, Role::User | Role::Assistant) || message.content.is_empty() { return None; }
            used += message.content.len();
            if used > 16384 { return None; }
            Some(json!({"role":if message.role == Role::User {"user"} else {"model"},"parts":[{"text":message.content}]}))
        }).collect();
        turns.reverse();
        if !turns.is_empty() {
            live.seed_conversation(id, turns).map_err(|_| {
                AiError::PolicyViolation("Voice context exceeds provider limits".into())
            })?;
        }
        Ok(live)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_setup_uses_welder_tools_and_does_not_silently_switch_provider() {
        let mut agent = AgentRuntime::new();
        agent.switch_provider("local").unwrap();
        assert!(agent.start_voice_session(1, "fixture-live").is_err());
        agent.switch_provider("gemini").unwrap();
        let mut live = agent.start_voice_session(2, "fixture-live").unwrap();
        let setup: Value = serde_json::from_str(&live.take_outbound().unwrap()).unwrap();
        assert_eq!(
            setup["setup"]["generationConfig"]["responseModalities"][0],
            "AUDIO"
        );
        assert!(!setup["setup"]["tools"][0]["functionDeclarations"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn live_setup_uses_selected_name_shared_personality_and_separate_constraints() {
        let mut agent = AgentRuntime::new();
        agent.setup.agent_name = "Nova".into();
        agent.switch_provider("gemini").unwrap();
        let mut live = agent.start_voice_session(12, "fixture-live").unwrap();
        let setup: Value = serde_json::from_str(&live.take_outbound().unwrap()).unwrap();
        let instruction = setup["setup"]["systemInstruction"]["parts"][0]["text"].as_str().unwrap();
        let personality = crate::personality::system_prompt("Nova", crate::personality::ConversationMode::Voice);
        assert!(instruction.contains(&personality));
        assert!(instruction.contains(crate::personality::OPERATIONAL_INSTRUCTIONS));
        assert!(!instruction.contains("bare-metal ARM64"));
        assert!(!instruction.contains("Your user-facing name is"));
    }

    #[test]
    fn reconnect_seeds_completed_voice_history_after_setup_ready() {
        let mut agent = AgentRuntime::new();
        agent.switch_provider("gemini").unwrap();
        agent.record_voice_turn("previous question", "previous answer");
        let mut live = agent.start_voice_session(9, "fixture-live").unwrap();
        live.take_outbound().unwrap();
        assert!(live.take_outbound().is_none());
        live.ingest(9, r#"{"setupComplete":{}}"#).unwrap();
        let history: Value = serde_json::from_str(&live.take_outbound().unwrap()).unwrap();
        let turns = history["clientContent"]["turns"].as_array().unwrap();
        assert_eq!(turns[turns.len() - 2]["role"], "user");
        assert_eq!(
            turns[turns.len() - 2]["parts"][0]["text"],
            "previous question"
        );
        assert_eq!(turns[turns.len() - 1]["role"], "model");
        assert_eq!(
            turns[turns.len() - 1]["parts"][0]["text"],
            "previous answer"
        );
    }
    #[test]
    fn reconnect_context_is_bounded_and_setup_preserves_system_role() {
        let mut agent = AgentRuntime::new();
        agent.switch_provider("gemini").unwrap();
        agent.record_voice_turn(&"x".repeat(32768), "recent concise response");
        let mut live = agent.start_voice_session(10, "fixture-live").unwrap();
        let setup: Value = serde_json::from_str(&live.take_outbound().unwrap()).unwrap();
        assert!(setup["setup"]["systemInstruction"]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Speak naturally"));
        live.ingest(10, r#"{"setupComplete":{}}"#).unwrap();
        let history = live.take_outbound().unwrap();
        assert!(history.len() < 20000);
        assert!(history.contains("recent concise response"));
        assert!(!history.contains(&"x".repeat(100)));
    }
    #[test]
    fn executed_voice_tool_is_paired_with_its_result_in_shared_history() {
        let mut agent = AgentRuntime::new();
        agent.record_voice_turn("do action", "");
        let call = alloyos_forge::provider::ToolCall::new("c1", "fixture_tool", "{}");
        agent.record_voice_tool(&call, &json!({"ok":true}));
        let history = &agent.history;
        assert_eq!(history[history.len() - 2].tool_calls[0].id, "c1");
        assert_eq!(
            history[history.len() - 1].tool_call_id.as_deref(),
            Some("c1")
        );
        assert!(history[history.len() - 1].content.contains("true"));
    }
}
