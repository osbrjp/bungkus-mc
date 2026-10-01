//! `bungkus-mc statusline`: Claude's status-line command while it runs
//! inside mc (ARCHITECTURE §6.2).
//!
//! It forwards the usage figures to mc's socket with a 200 ms budget, then
//! runs the user's own status-line command (resolved by mc at launch and
//! passed in `BUNGKUS_MC_USER_STATUSLINE`) under `sh -c` with the same
//! stdin, so the user's line still renders. It writes nothing of its own
//! to stdout or stderr; the user's command's stdout passes through.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::agent::usage::from_statusline;
use crate::ipc::hook::send;
use crate::ipc::{HookEvent, Wire};

/// Largest payload that is forwarded (1 MiB, SECURITY.md "Status-line
/// payload"); a larger one is only passed through.
const FORWARD_MAX: usize = 1024 * 1024;

/// Largest payload read at all; the user's command gets at most this.
const STDIN_MAX: u64 = 8 * 1024 * 1024;

/// How long forwarding may delay the user's status line (ARCHITECTURE §6.2).
const FORWARD_BUDGET: Duration = Duration::from_millis(200);

/// Runs the status-line subcommand.
///
/// # Returns
///
/// The exit code: the user's command's, or 0 when there is none or
/// anything fails.
#[must_use]
pub(crate) fn run() -> i32 {
    std::panic::catch_unwind(|| {
        let mut payload = Vec::new();
        // reason: an unreadable stdin leaves an empty payload, which is fine.
        let _ = std::io::stdin()
            .lock()
            .take(STDIN_MAX)
            .read_to_end(&mut payload);
        forward(&payload);
        std::env::var("BUNGKUS_MC_USER_STATUSLINE").map_or(0, |cmd| run_user(&cmd, &payload))
    })
    .unwrap_or(0)
}

/// Sends the payload's usage figures to mc's socket, waiting at most
/// [`FORWARD_BUDGET`].
fn forward(payload: &[u8]) {
    let (Some(sock), Ok(session)) = (
        std::env::var_os("BUNGKUS_MC_SOCK"),
        std::env::var("BUNGKUS_MC_SESSION"),
    ) else {
        return;
    };
    let Some(line) = encode(payload, &session) else {
        return;
    };
    let (done, wait) = mpsc::channel();
    std::thread::spawn(move || {
        // reason: mc may be gone; the user's line must render anyway.
        let _ = send(std::path::Path::new(&sock), &line);
        let _ = done.send(());
    });
    // reason: a slow socket just loses this report; the next one comes soon.
    let _ = wait.recv_timeout(FORWARD_BUDGET);
}

/// Builds the usage line for one payload; `None` when it is too large or
/// not JSON.
fn encode(payload: &[u8], session: &str) -> Option<Vec<u8>> {
    if payload.len() > FORWARD_MAX {
        return None;
    }
    let raw = serde_json::from_slice(payload).ok()?;
    let wire = Wire {
        mc_session: session.to_owned(),
        event: HookEvent::default(),
        usage: Some(from_statusline(&raw)),
    };
    let mut line = serde_json::to_vec(&wire).ok()?;
    line.push(b'\n');
    Some(line)
}

/// Runs the user's own status-line command with the payload on stdin and
/// returns its exit code. This is one of the two documented places mc
/// runs a shell (SECURITY.md); the command comes from the user's own
/// Claude settings, never from the agent or the socket.
fn run_user(command: &str, payload: &[u8]) -> i32 {
    let child = Command::new("sh")
        .args(["-c", command])
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return 0 };
    if let Some(mut stdin) = child.stdin.take() {
        // reason: a command that ignores stdin closes the pipe early.
        let _ = stdin.write_all(payload);
    }
    child.wait().ok().and_then(|s| s.code()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_usage_without_the_model() {
        let payload =
            br#"{"session_name":"x","model":{"id":"claude-secret"},"cost":{"total_cost_usd":1.5}}"#;
        let line = encode(payload, "m-1").unwrap();
        let text = String::from_utf8_lossy(&line);
        assert!(
            !text.contains("claude-secret"),
            "model.id is never forwarded"
        );
        let wire: Wire = serde_json::from_slice(&line).unwrap();
        assert_eq!(wire.usage.unwrap().cost_usd, Some(1.5));
        assert_eq!(
            encode(&vec![b' '; FORWARD_MAX + 1], "m-1"),
            None,
            "too large: pass through only"
        );
    }

    #[test]
    fn the_user_command_gets_the_exact_stdin() {
        let dir = std::env::temp_dir().join(format!("mc-sl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("got");
        let code = run_user(&format!("cat > '{}'; exit 4", out.display()), b"{\"a\":1}");
        assert_eq!(code, 4);
        assert_eq!(std::fs::read(&out).unwrap(), b"{\"a\":1}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
