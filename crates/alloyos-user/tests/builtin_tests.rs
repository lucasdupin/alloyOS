use liballoy::shell::{echo, unknown_command_message};

#[test]
fn echo_plain_text() {
    assert_eq!(echo("foo bar"), "foo bar\n");
}

#[test]
fn echo_multiple_args() {
    let args = ["one", "two", "three"];
    let out = echo(&args);
    assert_eq!(out, "one two three\n");
}

#[test]
fn unknown_command_error_message() {
    let err = unknown_command_message("fake_cmd");
    assert!(err.contains("not found"));
    assert!(err.contains("fake_cmd"));
}
