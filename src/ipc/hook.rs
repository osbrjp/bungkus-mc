//! `bungkus-mc hook`: the command every agent hook runs.
//!
//! It reads the payload from stdin, trims it ([`super::trim`]) and sends one
//! JSON line to mc's socket. It is silent — no stdout, no stderr, exit 0
//! whatever happens — so it can never disturb or block the agent
//! (SECURITY.md "Hook subcommands").

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use crate::ipc::{Wire, trim};

/// Most payload bytes read from stdin; the rest is drained and dropped
/// (8 MiB, per SECURITY.md "Hook subcommands").
const STDIN_MAX: u64 = 8 * 1024 * 1024;

/// How long writing to mc's socket may take.
const SEND_TIMEOUT: Duration = Duration::from_secs(2);

/// Runs the hook subcommand; always succeeds.
///
/// Every step that can fail (stdin, JSON, a missing socket, a panic) ends
/// the command quietly: the agent must never notice mc.
pub(crate) fn run() {
    // reason: a panic here must not reach the agent as an error exit.
    let _ = std::panic::catch_unwind(|| {
        let mut stdin = std::io::stdin().lock();
        let mut payload = Vec::new();
        if (&mut stdin)
            .take(STDIN_MAX)
            .read_to_end(&mut payload)
            .is_err()
        {
            return;
        }
        // reason: draining only keeps the agent's pipe from blocking.
        let _ = std::io::copy(&mut stdin, &mut std::io::sink());
        let (Some(sock), Some(session)) = (
            std::env::var_os("BUNGKUS_MC_SOCK"),
            std::env::var("BUNGKUS_MC_SESSION").ok(),
        ) else {
            return;
        };
        if let Some(line) = encode(&payload, &session) {
            // reason: mc may have quit; the hook stays silent.
            let _ = send(std::path::Path::new(&sock), &line);
        }
    });
}

/// Builds the wire line for one payload, or `None` for invalid JSON.
fn encode(payload: &[u8], session: &str) -> Option<Vec<u8>> {
    let raw = serde_json::from_slice(payload).ok()?;
    let wire = Wire {
        mc_session: session.to_owned(),
        event: trim(&raw),
        usage: None,
    };
    let mut line = serde_json::to_vec(&wire).ok()?;
    line.push(b'\n');
    Some(line)
}

/// Writes one line to the socket at `path`.
pub(crate) fn send(path: &std::path::Path, line: &[u8]) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_write_timeout(Some(SEND_TIMEOUT))?;
    stream.write_all(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_one_line_with_the_mc_session() {
        let line = encode(br#"{"hook_event_name":"Stop","prompt":"secret"}"#, "m-1").unwrap();
        assert!(line.ends_with(b"\n"));
        let wire: Wire = serde_json::from_slice(&line).unwrap();
        assert_eq!(
            (wire.mc_session.as_str(), wire.event.name.as_str()),
            ("m-1", "Stop")
        );
        assert!(!String::from_utf8_lossy(&line).contains("secret"));
        assert_eq!(encode(b"not json", "m-1"), None);
    }
}
