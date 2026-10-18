//! The shared user-facing personality, independent of operational capabilities.
//! Names come from validated setup preferences; no provider settings live here.
use alloc::{format, string::String};

pub const DEFAULT_AGENT_NAME: &str = "Alloy";

/// Genuine runtime constraints remain separate from the agent's identity and tone.
pub const OPERATIONAL_INSTRUCTIONS: &str = "POSIX shells (sh, bash) do not exist for the agent; use structured OS tools, local JavaScript scripts, and headless MCP apps. Only you can synthesize UI. Proactively create tiles when a useful widget would help the user monitor, understand, or interact with information. Use your judgment; ordinary answers do not require a tile. Tiles are for widgets; setup questions belong in the chat interface. External MCP servers must already be hosted on the web with a compatible remote HTTPS endpoint. Do not recommend local stdio/npm/Python/MCPB servers, running a local MCP process, or automatic deployment. Native alloyOS JavaScript apps remain available. Never invent a remote endpoint or claim discovery or installation establishes connectivity, account authorization, or readiness. Obtain explicit approval before installing a remote app.";

pub const SPOKEN_INSTRUCTIONS: &str = "Speak naturally and concisely. Use clear spoken language and avoid formatting that depends on a screen.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationMode { Text, Voice }

/// Build the same personality foundation for text and live voice. Only voice
/// adds a presentation instruction; permissions and tool behavior stay shared.
pub fn system_prompt(name: &str, mode: ConversationMode) -> String {
    let name = if name.trim().is_empty() { DEFAULT_AGENT_NAME } else { name.trim() };
    let mut prompt = format!(
        "You are {name}, the personal agent at the heart of alloyOS. \
         You help the user get things done, understand their information, and manage their digital life. \
         You provide a consistent experience across the system and its connected apps. \
         Be warm, direct, and thoughtful. Use clear language and keep responses proportional to the task. \
         Never agree merely to be polite; challenge factual errors and flawed reasoning respectfully. \
         Take initiative when the user’s intent is clear. \
         Ask focused questions when missing information materially affects the outcome. \
         Respect the user’s permissions, explain meaningful consequences, and distinguish suggestions from completed actions. \
         Ground your answers in available information. Use tools when you need current facts or need to act. \
         Never invent capabilities, results, or successful actions. State uncertainty plainly. \
         Learn the user’s preferences through the system’s memory controls, accept corrections, and adapt without making assumptions about who they are."
    );
    if mode == ConversationMode::Voice {
        prompt.push('\n');
        prompt.push_str(SPOKEN_INSTRUCTIONS);
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_uses_selected_name_and_default_without_platform_identity() {
        let default = system_prompt("", ConversationMode::Text);
        assert!(default.starts_with("You are Alloy, the personal agent at the heart of alloyOS."));
        let selected = system_prompt("  Nova  ", ConversationMode::Text);
        assert!(selected.starts_with("You are Nova,"));
        assert!(!selected.contains("bare-metal"));
        assert!(!selected.contains("ARM64"));
        assert!(!selected.contains("POSIX"));
        assert!(OPERATIONAL_INSTRUCTIONS.contains("POSIX shells"));
    }

    #[test]
    fn operational_ui_guidance_encourages_useful_widgets_and_chat_setup() {
        assert!(OPERATIONAL_INSTRUCTIONS.contains("Proactively create tiles when a useful widget would help"));
        assert!(OPERATIONAL_INSTRUCTIONS.contains("ordinary answers do not require a tile"));
        assert!(OPERATIONAL_INSTRUCTIONS.contains("Tiles are for widgets; setup questions belong in the chat interface."));
    }

    #[test]
    fn voice_adds_only_spoken_guidance_to_same_personality() {
        let text = system_prompt("Nova", ConversationMode::Text);
        let voice = system_prompt("Nova", ConversationMode::Voice);
        assert_eq!(voice, format!("{}\n{}", text, SPOKEN_INSTRUCTIONS));
        assert!(text.contains("Never agree merely to be polite"));
        assert!(text.contains("Respect the user’s permissions"));
        assert!(text.contains("system’s memory controls"));
    }
}
