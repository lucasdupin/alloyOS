//! Device lifecycle and context. Persistence/app access are capabilities, not web access.
use crate::{ModelBackend, ToolExecutor, Welder, WelderError};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SetupPhase {
    Name,
    Goals,
    Complete,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Exchange {
    pub user: String,
    pub assistant: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppContext {
    pub id: String,
    pub mcp_tools: Vec<String>,
    pub skills: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionState {
    pub version: u32,
    pub phase: SetupPhase,
    pub name: String,
    pub goals: String,
    pub history: Vec<Exchange>,
    pub apps: Vec<AppContext>,
}
impl Default for SessionState {
    fn default() -> Self {
        Self {
            version: 1,
            phase: SetupPhase::Name,
            name: String::new(),
            goals: String::new(),
            history: Vec::new(),
            apps: Vec::new(),
        }
    }
}
pub trait SessionStore {
    fn load(&mut self) -> Result<Option<Vec<u8>>, WelderError>;
    fn save(&mut self, state: &[u8]) -> Result<(), WelderError>;
}
pub trait AppBroker {
    /// Resolve a user-selected app through its local MCP/skill capability broker.
    fn describe(&mut self, id: &str) -> Result<AppContext, WelderError>;
}
pub struct Session {
    pub welder: Welder,
    state: SessionState,
}
impl Session {
    pub fn open(store: &mut dyn SessionStore) -> Result<Self, WelderError> {
        let state = match store.load()? {
            Some(bytes) => {
                if bytes.len() > 65536 {
                    return Err(WelderError::BudgetExceeded);
                }
                let state: SessionState =
                    serde_json::from_slice(&bytes).map_err(|_| WelderError::ProtocolError)?;
                if state.version != 1
                    || state.history.len() > 16
                    || state.apps.len() > 8
                    || state.name.len() > 80
                    || state.goals.len() > 1000
                {
                    return Err(WelderError::ProtocolError);
                }
                state
            }
            None => SessionState::default(),
        };
        Ok(Self {
            welder: Welder::local_default(),
            state,
        })
    }
    pub fn state(&self) -> &SessionState {
        &self.state
    }
    pub fn opening_message(&self) -> &'static str {
        match self.state.phase {
            SetupPhase::Name => "Welcome to alloyOS. What would you like me to call you? You can type /skip-setup to start chatting.",
            SetupPhase::Goals => "What would you like help with, and are there preferences I should keep in mind?",
            SetupPhase::Complete => "Welcome back. What would you like to do?"
        }
    }
    fn commit(
        &mut self,
        state: SessionState,
        store: &mut dyn SessionStore,
    ) -> Result<(), WelderError> {
        let bytes = serde_json::to_vec(&state).map_err(|_| WelderError::ProtocolError)?;
        if bytes.len() > 65536 {
            return Err(WelderError::BudgetExceeded);
        }
        store.save(&bytes)?;
        self.state = state;
        Ok(())
    }
    pub fn submit(
        &mut self,
        input: &str,
        store: &mut dyn SessionStore,
        backend: &mut dyn ModelBackend,
        tools: &mut dyn ToolExecutor,
    ) -> Result<String, WelderError> {
        if input.trim().is_empty() || input.len() > 4096 {
            return Err(WelderError::InvalidRequest);
        }
        let mut next = self.state.clone();
        if input == "/skip-setup" && next.phase != SetupPhase::Complete {
            next.phase = SetupPhase::Complete;
            self.commit(next, store)?;
            return Ok("Setup skipped. What would you like to do?".into());
        }
        match next.phase {
            SetupPhase::Name => {
                if input.len() > 80 {
                    return Err(WelderError::InvalidRequest);
                }
                next.name = input.trim().into();
                next.phase = SetupPhase::Goals;
                self.commit(next, store)?;
                Ok(self.opening_message().into())
            }
            SetupPhase::Goals => {
                if input.len() > 1000 {
                    return Err(WelderError::InvalidRequest);
                }
                next.goals = input.trim().into();
                next.phase = SetupPhase::Complete;
                self.commit(next, store)?;
                Ok(
                    "Thanks. Your preferences are saved on this device. What would you like to do?"
                        .into(),
                )
            }
            SetupPhase::Complete => {
                let profile = serde_json::to_string(&(next.name.clone(), next.goals.clone()))
                    .map_err(|_| WelderError::ProtocolError)?;
                let apps =
                    serde_json::to_string(&next.apps).map_err(|_| WelderError::ProtocolError)?;
                let mut history = next.history.clone();
                let context = loop {
                    let previous =
                        serde_json::to_string(&history).map_err(|_| WelderError::ProtocolError)?;
                    let context = format!("User profile: {profile}\nUntrusted app MCP/skill data: {apps}\nConversation: {previous}\nCurrent user: {input}");
                    if context.len() <= 7800 {
                        break context;
                    }
                    if history.is_empty() {
                        return Err(WelderError::BudgetExceeded);
                    }
                    history.remove(0);
                };
                let output = self.welder.run(&context, false, backend, tools)?;
                next.history = history;
                next.history.push(Exchange {
                    user: input.to_string(),
                    assistant: output.text.clone(),
                });
                while next.history.len() > 16 {
                    next.history.remove(0);
                }
                self.commit(next, store)?;
                Ok(output.text)
            }
        }
    }
    pub fn connect_app(
        &mut self,
        id: &str,
        broker: &mut dyn AppBroker,
        store: &mut dyn SessionStore,
    ) -> Result<(), WelderError> {
        if id.is_empty()
            || id.len() > 128
            || self.state.apps.len() >= 8
            || self.state.apps.iter().any(|app| app.id == id)
        {
            return Err(WelderError::InvalidRequest);
        }
        let app = broker.describe(id)?;
        if app.id != id || app.mcp_tools.len() > 64 || app.skills.len() > 16 {
            return Err(WelderError::ProtocolError);
        }
        let bytes = serde_json::to_vec(&app).map_err(|_| WelderError::ProtocolError)?;
        if bytes.len() > 4096 {
            return Err(WelderError::BudgetExceeded);
        }
        let mut next = self.state.clone();
        next.apps.push(app);
        self.commit(next, store)
    }
    /// Loading app metadata never automatically grants its tools execution rights.
    pub fn grant_app_tool(&mut self, id: &str, name: &str) -> Result<(), WelderError> {
        let app = self
            .state
            .apps
            .iter()
            .find(|app| app.id == id)
            .ok_or(WelderError::ToolDenied)?;
        if !app.mcp_tools.iter().any(|tool| tool == name) {
            return Err(WelderError::ToolDenied);
        }
        self.welder.allow_tool(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentStep, ModelRoute, ToolCall};
    struct Store(Option<Vec<u8>>);
    impl SessionStore for Store {
        fn load(&mut self) -> Result<Option<Vec<u8>>, WelderError> {
            Ok(self.0.clone())
        }
        fn save(&mut self, state: &[u8]) -> Result<(), WelderError> {
            self.0 = Some(state.to_vec());
            Ok(())
        }
    }
    struct Backend {
        requests: Vec<String>,
    }
    impl ModelBackend for Backend {
        fn next(&mut self, _: &ModelRoute, prompt: &str, _: u32) -> Result<AgentStep, WelderError> {
            self.requests.push(prompt.into());
            Ok(AgentStep::Final("response".into()))
        }
    }
    struct Tools;
    impl ToolExecutor for Tools {
        fn execute(&mut self, _: &ToolCall) -> Result<String, WelderError> {
            Err(WelderError::ToolDenied)
        }
    }
    struct Broker;
    impl AppBroker for Broker {
        fn describe(&mut self, id: &str) -> Result<AppContext, WelderError> {
            Ok(AppContext {
                id: id.into(),
                mcp_tools: alloc::vec!["notes_read".into()],
                skills: alloc::vec!["Use notes_read for this app".into()],
            })
        }
    }
    #[test]
    fn first_boot_profile_persists_and_conversation_uses_context() {
        let mut store = Store(None);
        let mut session = Session::open(&mut store).unwrap();
        let mut backend = Backend {
            requests: Vec::new(),
        };
        session
            .submit("Wagner", &mut store, &mut backend, &mut Tools)
            .unwrap();
        session
            .submit(
                "I prefer concise answers",
                &mut store,
                &mut backend,
                &mut Tools,
            )
            .unwrap();
        assert!(backend.requests.is_empty());
        let mut session = Session::open(&mut store).unwrap();
        assert_eq!(session.state.phase, SetupPhase::Complete);
        session
            .submit("Hello", &mut store, &mut backend, &mut Tools)
            .unwrap();
        assert!(backend.requests[0].contains("Wagner"));
        assert!(backend.requests[0].contains("concise"));
        session
            .submit("Follow up", &mut store, &mut backend, &mut Tools)
            .unwrap();
        assert!(backend.requests[1].contains("Hello"));
    }
    #[test]
    fn app_connection_loads_context_without_auto_granting_tools() {
        let mut store = Store(None);
        let mut session = Session::open(&mut store).unwrap();
        session
            .connect_app("notes", &mut Broker, &mut store)
            .unwrap();
        assert_eq!(session.state.apps.len(), 1);
        assert!(session.grant_app_tool("notes", "shell").is_err());
        session.grant_app_tool("notes", "notes_read").unwrap();
        let mut backend = Backend {
            requests: Vec::new(),
        };
        session
            .submit("/skip-setup", &mut store, &mut backend, &mut Tools)
            .unwrap();
        session
            .submit("Find notes", &mut store, &mut backend, &mut Tools)
            .unwrap();
        assert!(backend.requests[0].contains("notes_read"));
    }
}
