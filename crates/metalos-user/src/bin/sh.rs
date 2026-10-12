#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

pub use libmetal::shell::{
    clear, echo, execute_command, line_from_buf, parse_command, parse_pipeline, readline,
    unknown_command_message, Command, History,
};

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libmetal::io::eprint("metal-sh: panic occurred\n");
    libmetal::process::exit(1);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    main_loop();
    libmetal::process::exit(0);
}

#[cfg(not(target_os = "none"))]
fn main() {
    main_loop();
}

pub fn main_loop() {
    let mut history = History::new();
    loop {
        libmetal::io::print("metalos:/$ ");
        let line_buf = readline();
        let line_str = line_from_buf(&line_buf);
        let trimmed = line_str.trim();
        if !trimmed.is_empty() {
            history.push(trimmed);
        }
        let cmd = parse_command(line_str);
        execute_command(&cmd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libmetal::shell::builtin_uname;

    #[test]
    fn parse_simple_command() {
        let cmd = parse_command("ls -l");
        assert_eq!(cmd.name, "ls");
        assert_eq!(cmd.args, ["-l"]);
    }

    #[test]
    fn parse_empty_input() {
        let cmd = parse_command("");
        assert!(cmd.is_empty());
        assert_eq!(cmd.name, "");
        assert_eq!(cmd.args.len(), 0);

        let cmd_spaces = parse_command("   ");
        assert!(cmd_spaces.is_empty());
    }

    #[test]
    fn parse_multi_args() {
        let cmd = parse_command("echo a b c");
        assert_eq!(cmd.name, "echo");
        assert_eq!(cmd.args, ["a", "b", "c"]);
    }

    #[test]
    fn parse_quoted_string() {
        let cmd = parse_command("echo 'hello world'");
        assert_eq!(cmd.name, "echo");
        assert_eq!(cmd.args, ["hello world"]);
    }

    #[test]
    fn parse_whitespace_trimmed() {
        let cmd = parse_command("  ls  ");
        assert_eq!(cmd.name, "ls");
        assert_eq!(cmd.args.len(), 0);
    }

    #[test]
    fn builtin_echo_output() {
        let out = echo("hello world");
        assert_eq!(out, "hello world\n");
    }

    #[test]
    fn builtin_clear_produces_ansi() {
        let out = clear();
        assert_eq!(out, "\x1b[2J\x1b[H");
    }

    #[test]
    fn builtin_uname_contains_metalos() {
        let out = builtin_uname(false);
        assert!(out.contains("metalOS"));
    }

    #[test]
    fn shell_unknown_command_message() {
        let out = unknown_command_message("not_a_cmd");
        assert!(out.contains("not found"));
    }
}
