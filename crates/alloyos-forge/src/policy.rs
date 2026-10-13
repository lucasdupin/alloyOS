use crate::provider::AiError;
use alloc::format;

/// Canonical explanation returned whenever a user or model attempts a POSIX shell escape or tool.
pub const SHELL_FREE_POLICY_MESSAGE: &str =
    "alloyOS is a shell-free, AI-first operating system: POSIX shell access (`sh`, `bash`, `sh_exec`, `!cmd`) is strictly forbidden. Use structured native OS tools (`fs_*`, `sys_status`), local JavaScript automation (`script_create`, `script_run`, `script_schedule`), and headless MCP apps (`mcp_*`) instead.";

/// Returns `true` if a tool identifier corresponds to a forbidden shell/process execution primitive.
pub fn is_forbidden_shell_tool(tool_name: &str) -> bool {
    let normalized = tool_name.trim().to_ascii_lowercase();
    if normalized.starts_with('!')
        || normalized.starts_with("sh_")
        || normalized.starts_with("bash_")
        || normalized.starts_with("shell_")
    {
        return true;
    }

    matches!(
        normalized.as_str(),
        "sh" | "bash"
            | "zsh"
            | "fish"
            | "csh"
            | "ksh"
            | "dash"
            | "sh_exec"
            | "bash_exec"
            | "exec"
            | "execve"
            | "cmd"
            | "cmd_exec"
            | "shell"
            | "run_shell"
            | "run_terminal_cmd"
            | "system"
            | "popen"
            | "spawn_shell"
            | "/bin/sh"
            | "/bin/bash"
    )
}

/// Validates raw user or console input, rejecting `!` shell escapes and direct shell invocations.
pub fn validate_console_input_no_shell(input: &str) -> Result<(), AiError> {
    let trimmed = input.trim();
    if trimmed.starts_with('!') {
        return Err(AiError::ShellAccessForbidden(format!(
            "Rejected shell escape '{}'. {}",
            trimmed, SHELL_FREE_POLICY_MESSAGE
        )));
    }

    let first_token = trimmed.split_whitespace().next().unwrap_or("");
    if is_forbidden_shell_tool(first_token) {
        return Err(AiError::ShellAccessForbidden(format!(
            "Rejected shell command '{}'. {}",
            first_token, SHELL_FREE_POLICY_MESSAGE
        )));
    }

    Ok(())
}
