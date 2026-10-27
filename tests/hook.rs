//! The silent agent bridges as the agents run them: both exit 0 without
//! output on valid input, garbage, a missing socket, and oversized input
//! (`CODING_RULES.md` §2 "Hook subcommand silence").

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn agent_bridge_commands_are_silent_and_exit_zero() {
    let big = "x".repeat(9 * 1024 * 1024);
    let cases: [(&str, Option<&str>); 3] = [
        (
            r#"{"hook_event_name":"Stop"}"#,
            Some("/nonexistent/mc.sock"),
        ),
        ("garbage", None),
        (&big, Some("/nonexistent/mc.sock")),
    ];
    for command in ["hook", "pi-hook"] {
        for (stdin, sock) in cases {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_bungkus-mc"));
            cmd.arg(command).env("BUNGKUS_MC_SESSION", "m-1");
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
                // reason: the bridge may stop reading at its cap; a broken pipe is fine.
                let _ = input.write_all(data.as_bytes());
            });
            let out = child.wait_with_output().unwrap();
            writer.join().unwrap();
            assert!(out.status.success(), "{command} exit {:?}", out.status);
            assert!(
                out.stdout.is_empty() && out.stderr.is_empty(),
                "{command} printed something"
            );
        }
    }
}
