//! `bungkus-mc hook` as the agent runs it: silent and exit 0 on every
//! input, including no socket, garbage and a payload over the 8 MiB cap
//! (`CODING_RULES.md` §2 "Hook subcommand silence").

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn the_hook_command_is_silent_and_exits_zero() {
    let big = "x".repeat(9 * 1024 * 1024);
    let cases: [(&str, Option<&str>); 3] = [
        (
            r#"{"hook_event_name":"Stop"}"#,
            Some("/nonexistent/mc.sock"),
        ),
        ("garbage", None),
        (&big, Some("/nonexistent/mc.sock")),
    ];
    for (stdin, sock) in cases {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_bungkus-mc"));
        cmd.arg("hook").env("BUNGKUS_MC_SESSION", "m-1");
        match sock {
            Some(s) => cmd.env("BUNGKUS_MC_SOCK", s),
            None => cmd.env_remove("BUNGKUS_MC_SOCK"),
        };
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let data = stdin.to_owned();
        let writer = std::thread::spawn(move || {
            // reason: the hook may stop reading at its cap; a broken pipe is fine.
            let _ = input.write_all(data.as_bytes());
        });
        let out = child.wait_with_output().unwrap();
        writer.join().unwrap();
        assert!(out.status.success(), "exit {:?}", out.status);
        assert!(
            out.stdout.is_empty() && out.stderr.is_empty(),
            "the hook printed something"
        );
    }
}
