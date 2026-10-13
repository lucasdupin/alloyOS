use liballoy::shell::{parse_command, parse_pipeline, History};

/// Tests placeholder for multi-stage command pipeline execution.
/// Multi-stage pipe execution is planned for future kernel IPC / pipe() syscall integration.
#[test]
fn shell_parse_pipe_placeholder() {
    let pipeline = parse_pipeline("echo hello | cat");
    assert!(pipeline.has_pipe);
    assert_eq!(pipeline.count, 2);
    assert_eq!(pipeline.commands[0].name, "echo");
    assert_eq!(pipeline.commands[0].args, ["hello"]);
    assert_eq!(pipeline.commands[1].name, "cat");
    assert_eq!(pipeline.commands[1].args.len(), 0);
}

#[test]
fn shell_parse_redirect_stdout() {
    let cmd = parse_command("echo hi > /tmp/f");
    assert_eq!(cmd.name, "echo");
    assert_eq!(cmd.args, ["hi"]);
    assert_eq!(cmd.redirect_stdout, Some("/tmp/f"));
}

#[test]
fn shell_history_items_ordered() {
    let mut history = History::new();
    history.push("cmd1");
    history.push("cmd2");
    history.push("cmd3");

    assert_eq!(history.count(), 3);
    assert_eq!(history.get_lifo(0), Some("cmd3"));
    assert_eq!(history.get_lifo(1), Some("cmd2"));
    assert_eq!(history.get_lifo(2), Some("cmd1"));
    assert_eq!(history.get_lifo(3), None);

    let collected: std::vec::Vec<&str> = history.iter_lifo().collect();
    assert_eq!(collected, &["cmd3", "cmd2", "cmd1"]);
}
