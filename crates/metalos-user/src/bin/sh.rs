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
    libmetal::shell::run_shell();
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

    #[test]
    fn builtin_cd_and_pwd_navigation() {
        use libmetal::shell::{builtin_cd, builtin_pwd, current_dir, resolve_path};

        assert_eq!(resolve_path("/", "dev"), "/dev");
        assert_eq!(resolve_path("/dev", ".."), "/");
        assert_eq!(resolve_path("/dev", "../proc/./"), "/proc");
        assert_eq!(resolve_path("/etc", "~"), "/");

        // Reset to /
        assert!(builtin_cd(&[]).is_ok());
        assert_eq!(current_dir(), "/");
        assert_eq!(builtin_pwd(), "/\n");

        // Relative cd into dev -> /dev
        assert_eq!(builtin_cd(&["dev"]), Ok(None));
        assert_eq!(current_dir(), "/dev");
        assert_eq!(builtin_pwd(), "/dev\n");

        // Relative cd ../proc -> /proc
        assert_eq!(builtin_cd(&["../proc"]), Ok(None));
        assert_eq!(current_dir(), "/proc");

        // cd - returns to /dev and outputs "/dev\n"
        assert_eq!(builtin_cd(&["-"]), Ok(Some("/dev\n".into())));
        assert_eq!(current_dir(), "/dev");

        // Invalid directory returns error and preserves cwd
        let err = builtin_cd(&["/nonexistent"]).unwrap_err();
        assert_eq!(err, "cd: /nonexistent: No such file or directory\n");
        assert_eq!(current_dir(), "/dev");

        // cd with no args returns to /
        assert_eq!(builtin_cd(&[]), Ok(None));
        assert_eq!(current_dir(), "/");
    }

    #[test]
    fn builtin_ls_paths_and_globs() {
        use libmetal::shell::{builtin_cd, builtin_ls};

        assert_eq!(builtin_cd(&[]), Ok(None));

        // ls / and ls (in /)
        let root_out = builtin_ls(&[]);
        assert!(root_out.contains("bin/\ndev/\nproc/\netc/\ntmp/\n"));
        assert_eq!(builtin_ls(&["/"]), root_out);

        // ls /etc and ls etc
        let etc_out = builtin_ls(&["/etc"]);
        assert_eq!(etc_out, "motd\nai.conf\nschedules.conf\n");
        assert_eq!(builtin_ls(&["etc"]), etc_out);

        // ls etc/* and ls /etc/*
        assert_eq!(
            builtin_ls(&["etc/*"]),
            "etc/motd\netc/ai.conf\netc/schedules.conf\n"
        );
        assert_eq!(
            builtin_ls(&["/etc/*"]),
            "/etc/motd\n/etc/ai.conf\n/etc/schedules.conf\n"
        );
        assert_eq!(
            builtin_ls(&["/etc/*.conf"]),
            "/etc/ai.conf\n/etc/schedules.conf\n"
        );

        // Single file and nonexistent path
        assert_eq!(builtin_ls(&["/etc/motd"]), "/etc/motd\n");
        assert_eq!(
            builtin_ls(&["/no_such_dir"]),
            "ls: cannot access '/no_such_dir': No such file or directory\n"
        );
    }
}
