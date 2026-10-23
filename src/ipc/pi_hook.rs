//! `bungkus-mc pi-hook`: the silent bridge used by mc's bundled Pi extension.
//!
//! It accepts only the extension's documented lifecycle fields and numeric
//! usage summary, applies the same bounds as other hook sources, and writes
//! one [`super::Wire`] line to the private socket. It never reads Pi session
//! files or forwards message content.

use std::io::Read;

use serde_json::Value;

use crate::agent::usage::Usage;
use crate::ipc::{HookEvent, Wire, trim};
use crate::ui::sanitise::truncate;

/// Most bytes accepted from the extension (1 MiB, per SECURITY.md).
const STDIN_MAX: u64 = 1024 * 1024;

/// Runs the Pi hook subcommand; always succeeds and stays silent.
pub(crate) fn run() {
    // reason: an extension integration failure must not disturb Pi's UI.
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
        // reason: draining prevents Pi's writer from blocking on oversized input.
        let _ = std::io::copy(&mut stdin, &mut std::io::sink());
        let (Some(sock), Some(session)) = (
            std::env::var_os("BUNGKUS_MC_SOCK"),
            std::env::var("BUNGKUS_MC_SESSION").ok(),
        ) else {
            return;
        };
        if let Some(line) = encode(&payload, &session) {
            // reason: mc may have quit; the extension bridge stays silent.
            let _ = super::hook::send(std::path::Path::new(&sock), &line);
        }
    });
}

/// Builds one bounded socket line from a Pi extension payload.
fn encode(payload: &[u8], session: &str) -> Option<Vec<u8>> {
    let raw: Value = serde_json::from_slice(payload).ok()?;
    let usage = pi_usage(&raw);
    let event = if usage.is_some() {
        HookEvent::default()
    } else {
        trim(&raw)
    };
    if usage.is_none() && event.name.is_empty() {
        return None;
    }
    let wire = Wire {
        mc_session: session.to_owned(),
        event,
        usage,
    };
    let mut line = serde_json::to_vec(&wire).ok()?;
    line.push(b'\n');
    Some(line)
}

/// Extracts Pi's numeric usage summary without accepting message content.
fn pi_usage(raw: &Value) -> Option<Usage> {
    let raw = raw.get("bungkus_mc_pi_usage")?.as_object()?;
    let int = |key: &str| raw.get(key).and_then(Value::as_u64);
    let finite = |key: &str| {
        raw.get(key)
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value >= 0.0)
    };
    let direct = int("input");
    let output = int("output");
    let cache_read = int("cacheRead");
    let cache_write = int("cacheWrite");
    let cost_usd = finite("cost");
    let ctx_pct = finite("contextPercent").filter(|percent| *percent <= 100.0);
    let ctx_size = int("contextWindow");
    if [direct, output, cache_read, cache_write, ctx_size]
        .into_iter()
        .all(|value| value.is_none())
        && cost_usd.is_none()
        && ctx_pct.is_none()
    {
        return None;
    }
    let input = direct
        .unwrap_or(0)
        .checked_add(cache_read.unwrap_or(0))?
        .checked_add(cache_write.unwrap_or(0));
    Some(Usage {
        session_name: raw
            .get("sessionName")
            .and_then(Value::as_str)
            .map(|name| truncate(name, 80)),
        input,
        output,
        cache_read,
        cache_write,
        cost_usd,
        ctx_pct,
        ctx_size,
        limits: Vec::new(),
        added_dirs: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_lifecycle_events_through_the_shared_trimmer() {
        let line = encode(
            br#"{"hook_event_name":"PreToolUse","session_id":"pi-1","tool_name":"bash","secret":"drop"}"#,
            "m-1",
        )
        .unwrap();
        let wire: Wire = serde_json::from_slice(&line).unwrap();
        assert_eq!(wire.mc_session, "m-1");
        assert_eq!(wire.event.name, "PreToolUse");
        assert_eq!(wire.event.session_id.as_deref(), Some("pi-1"));
        assert_eq!(wire.event.tool_name.as_deref(), Some("bash"));
        assert_eq!(wire.usage, None);
        assert!(!String::from_utf8_lossy(&line).contains("secret"));
    }

    #[test]
    fn maps_usage_without_forwarding_messages() {
        let line = encode(
            br#"{"bungkus_mc_pi_usage":{"input":10,"output":4,"cacheRead":20,"cacheWrite":3,"cost":0.25,"contextPercent":12.5,"contextWindow":200000,"sessionName":"pi work"},"message":"secret"}"#,
            "m-2",
        )
        .unwrap();
        let wire: Wire = serde_json::from_slice(&line).unwrap();
        let usage = wire.usage.unwrap();
        assert_eq!(usage.input, Some(33));
        assert_eq!(usage.output, Some(4));
        assert_eq!(usage.cache_read, Some(20));
        assert_eq!(usage.cache_write, Some(3));
        assert_eq!(usage.cost_usd, Some(0.25));
        assert_eq!(usage.ctx_pct, Some(12.5));
        assert_eq!(usage.ctx_size, Some(200_000));
        assert_eq!(usage.session_name.as_deref(), Some("pi work"));
        assert!(!String::from_utf8_lossy(&line).contains("secret"));
    }

    #[test]
    fn accepts_the_representative_lifecycle_fixture() {
        let payloads =
            include_bytes!("../agent/testdata/pi/lifecycle.jsonl").split(|b| *b == b'\n');
        let wires: Vec<Wire> = payloads
            .filter(|payload| !payload.is_empty())
            .map(|payload| {
                let line = encode(payload, "m-3").expect("fixture event");
                serde_json::from_slice(&line).expect("socket line")
            })
            .collect();
        assert_eq!(wires.len(), 6);
        assert_eq!(wires[0].event.name, "SessionStart");
        assert_eq!(wires[2].event.name, "PreToolUse");
        assert_eq!(wires[4].usage.as_ref().and_then(|u| u.input), Some(33));
        assert_eq!(wires[5].event.name, "Stop");
    }

    #[test]
    fn rejects_malformed_empty_and_overflowing_payloads() {
        assert_eq!(encode(b"not json", "m"), None);
        assert_eq!(encode(b"{}", "m"), None);
        assert_eq!(encode(br#"{"bungkus_mc_pi_usage":{}}"#, "m"), None);
        assert_eq!(
            encode(
                br#"{"bungkus_mc_pi_usage":{"input":18446744073709551615,"cacheRead":1}}"#,
                "m"
            ),
            None
        );
    }
}
