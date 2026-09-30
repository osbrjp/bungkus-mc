# bungkus-mcc — Tech Stack

Status: proposal. **bungkus-mcc is written in Rust** (product-owner
decision, 2026-09-30, on the evidence of the M0 spike — PROPOSAL.md §6 and
`spikes/`). bungkus-cli stays Go. Rule: every crate must earn its line
here, with the alternatives rejected. Versions are the crates.io releases
checked on 2026-09-30 (`cargo search`); they are pinned to the exact minor
in `Cargo.toml` and `Cargo.lock` is committed.

## Language and toolchain

| Item | Choice | Why |
|------|--------|-----|
| Rust | stable **1.97**, pinned in `rust-toolchain.toml`; edition 2024 | Same toolchain on every machine and in CI; the spike built and passed its lint gate on 1.97 |
| Crate shape | one binary crate, modules (ARCHITECTURE.md §9); no workspace until a second crate exists | YAGNI |
| Lints | the set in `.claude/skills/rust-best-practices/SKILL.md` (`unsafe_code = "deny"`, `unwrap_used = "deny"`, pedantic, `missing_docs`), `rustfmt.toml` `edition = "2024"`, `max_width = 100`, `clippy.toml` test allowances | proven on the spike |
| Targets | darwin/linux × arm64/amd64 | Same release matrix as bungkus-cli |
| Release profile | `strip = true`, `lto = "thin"`, `codegen-units = 1` | 1.2 MB binary in the spike with `strip` alone |

## Direct dependencies (15)

| Crate | Version | Purpose | Alternatives rejected |
|-------|---------|---------|-----------------------|
| `ratatui` | 0.30 | Immediate-mode TUI: layout, `Buffer`/`Cell`, borders, `Color::Rgb`/`Color::Indexed`, our own widgets for panes and cards | cursive (retained-mode, heavier); raw crossterm drawing (we'd rewrite ratatui's buffer diff) |
| `crossterm` | 0.29 | Raw mode, alternate screen, key/mouse/paste/resize events, **kitty keyboard enhancement flags on the host side** (`PushKeyboardEnhancementFlags`), bracketed paste, OSC 22/23 title push/pop via raw writes | termion (no Windows is fine, but no kitty flags, unmaintained); termwiz (large, pulls half of wezterm) |
| `alacritty_terminal` | 0.26 | The embedded VT emulator for the output pane: grid, scrollback (`scrolling_history`, `scroll_display`), wide-char flags, mode tracking (DECCKM, 2004, mouse, kitty stack), and **terminal query replies** (DSR, DA, OSC 10/11/12, XTWINOPS) delivered as `Event`s | `wezterm-term` (+`termwiz`): has a key encoder, but **not on crates.io** — git-only dependency on a monorepo, no `cargo deny`/`cargo audit` story; `vt100`: on crates.io but **drops terminal queries**, which agents send at startup (DSR/DA/OSC 11), so every reply would be hand-written; spike verdict in `spikes/rust/README.md` |
| `portable-pty` | 0.9 | Open a PTY, spawn the child with a size, resize (`TIOCSWINSZ`), master reader/writer | `pty-process` (smaller, but tokio-flavoured API and less used); raw `rustix::pty` (we'd own openpty/login_tty/fork details) |
| `thiserror` | 2.0 | One error enum per module boundary (`term::SpawnError`, `ipc::Error`, …) | hand-written `Display`/`Error` impls (boilerplate) |
| `anyhow` | 1.0 | `main.rs` and top-level command handlers only, with `.context()` | — |
| `serde` (+ `derive`) | 1.0 | Config, state, hook payloads, status-line payload, Codex `token_count` records, TypeSafe request/response — **tolerant structs, never `deny_unknown_fields` for agent payloads** | hand-rolled JSON (no) |
| `serde_json` | 1.0 | The JSON codec for all of the above; `from_slice` on capped buffers | `simd-json` (unneeded speed, unsafe inside) |
| `uuid` (+ `v4`) | 1.26 | Session ids we choose for Claude (`--session-id`) and strict UUID validation of ids from hooks | a regex (we need generation too) |
| `lexopt` | 0.3 | CLI parsing for `bungkus-mcc [workspace]`, `hook`, `statusline`, `setup codex`, `update`, and ~6 flags | `clap` (derive): the obvious choice, but it adds ~15 crates and ~600 KB for four subcommands; `lexopt` has zero dependencies and the help text is 30 hand-written lines. Revisit if the CLI grows past two levels |
| `ureq` (rustls) | 3.4 | **Synchronous** HTTPS for the daily release check and the opt-in TypeSafe routing request: timeouts, no redirects, `LimitReader`-style body caps — no async runtime anywhere in mcc | `reqwest` (pulls tokio/hyper for two requests); `curl` bindings (C dependency); native-tls (platform TLS quirks; rustls is pure Rust and deterministic) |
| `semver` | 1.0 | Compare the running version with the release tag; validate the cached tag before display | hand-written compare |
| `rustix` (+ `process`, `fs`) | 1.1 | **Safe wrappers** for what the port needs: `kill`/`killpg`, `pidfd_open`/`pidfd_send_signal` (Linux), `setsid`, `getuid`, `fstat`, `O_NONBLOCK` opens — so `unsafe_code` stays denied crate-wide | `nix` (fine, but rustix is the modern, `unsafe`-free-at-the-API choice); `libc` directly (would require our own `unsafe`) |
| `tracing` | 0.1 | Structured debug events (`--debug`); the TUI owns stdout, so `print_stdout` is denied and every diagnostic goes through `tracing` | `log` + `env_logger` (env_logger writes to stderr, which the TUI owns; and `tracing` spans make the socket/PTY/threads readable) |
| `tracing-subscriber` (`fmt`) | 0.3 | The file writer behind `tracing`, enabled only under `--debug` | `tracing-appender` (rolling files; one 0600 file is enough) |

Transitive crate count in the spike: 118 (7 direct). The stack above adds
`thiserror`, `uuid`, `lexopt`, `ureq`+`rustls`, `semver`, `rustix`,
`tracing` — expect ~170 crates in `Cargo.lock`; `cargo deny` bans
duplicates of the heavy ones (`syn`, `windows-sys` families) from creeping
in twice. Every crate here is on crates.io with a permissive licence
(MIT/Apache-2.0/ISC; `cargo deny` enforces the allowlist).

## Deliberately not used

| Thing | Why not |
|-------|---------|
| Any async runtime (`tokio`, `smol`) | Every blocking I/O source is a thread sending `AppEvent` over `std::sync::mpsc`; there are at most a dozen threads. An executor would add crates and a second concurrency model for no throughput we need |
| `crossbeam-channel` | `std::sync::mpsc` covers one consumer with `recv_timeout`; revisit only if a `select!` over several receivers becomes necessary |
| A colour-profile crate (`supports-color`, `termcolor`, `anstyle-query`) | Detection is a few lines on `COLORTERM`, `TERM`, `NO_COLOR`, `CLICOLOR_FORCE`, `TERM_PROGRAM` (DESIGN.md §2.3); we output `Color::Rgb` at TrueColor and our **declared** `Color::Indexed` values otherwise — no automatic downsampling, so no crate |
| A snapshot crate (`insta`) | Goldens are plain text + cursor files under `testdata/`, compared with `assert_eq!` and updated with `UPDATE_GOLDEN=1` (skill §7); no macro layer between the test and the file |
| `notify` (fs events) | The Codex usage reader is a 1 s `metadata()` + `read_at` tail; macOS `notify` is kqueue/FSEvents and would still need the poll fallback |
| TOML/YAML config crates | `serde_json` for config; Claude Code and Codex users edit JSON already |
| SQLite (`rusqlite`) | State is one small JSON array |
| `sysinfo` / `procfs` crates | Descendant tracking needs `pid, ppid, uid, start time, comm`: Linux `/proc` text is a 40-line parser, macOS is one `ps` exec; ports are one `lsof` exec on both OSes |
| OS keychain crates (`keyring`) | The routing key comes from `TYPESAFE_API_KEY` or `routing.apiKeyCommand` (an argv the user configures) |
| A TypeSafe SDK | None exists for Rust (Python and JavaScript only); one `POST` with `ureq` + `serde_json` |
| `regex` | The few patterns (UUID, secret shapes, ps line layout) are hand-parsed or `uuid::Uuid::parse_str`; a regex engine is a large crate for six literals |

## Runtime prerequisites (not dependencies)

- `claude` and/or `codex` on PATH (detected at start; shown on the first-run screen).
- `bash`, `curl` for `bungkus-mcc update` (the installer script is reused from bungkus-cli).
- macOS: `ps` (ships with the OS); Linux: `/proc`. `lsof` on either OS for
  port annotation; missing `lsof` = no port labels in the quit dialog.
- A UTF-8 locale for box-drawing borders and the half-block mascot (ASCII
  forms otherwise); the default icon set is ASCII regardless.
- Routing only: a TypeSafe API key in `TYPESAFE_API_KEY` or via `routing.apiKeyCommand`.

## Tooling and CI

Every push and PR runs, in this order, all with no warnings:

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo deny check          # advisories, licences, bans, sources (deny.toml)
cargo audit               # RustSec advisories against Cargo.lock
```

Release: semantic-release with conventional commits (`main` = canary,
`release` = stable), the same four-workflow shape as bungkus-cli; the build
job compiles `--release` for darwin/linux × arm64/amd64 on native runners
(`macos-latest` for both macOS targets, `ubuntu-latest` + `ubuntu-24.04-arm`
for Linux), or `cargo-zigbuild` from one runner if the ARM Linux runner is
unavailable; uploads `bungkus-mcc-<os>-<arch>` + `checksums.txt`.
`install.sh` is bungkus-cli's script with `REPO`/`BIN_NAME` changed and is
fetched at the resolved release tag.
