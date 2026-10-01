# bungkus-mc — Tech Stack

Status: proposal. **bungkus-mc is written in Rust** (product-owner
decision, 2026-09-30, on the evidence of the M0 spike — PROPOSAL.md §6 and
`spikes/`). bungkus-cli stays Go. Rule: every crate must earn its line
here, with the alternatives rejected. Versions are the crates.io releases
checked on 2026-09-30 (`cargo search`; the `ureq` and `lexopt` numbers
were not built locally — re-check on crates.io when writing `Cargo.toml`
in M1). **`Cargo.lock` is committed and pins everything**; 0.x crates are
minor-pinned by caret (`"0.30.2"` cannot move to 0.31), 1.x/2.x crates are
major-pinned by caret, and `cargo update` is a reviewed commit.

## Language and toolchain

| Item | Choice | Why |
|------|--------|-----|
| Rust | stable **1.97**, pinned in `rust-toolchain.toml`; edition 2024 | Same toolchain on every machine and in CI; the spike built and passed its lint gate on 1.97 |
| Crate shape | one binary crate, modules (ARCHITECTURE.md §9); no workspace until a second crate exists | YAGNI |
| Lints | the set in `.claude/skills/rust-best-practices/SKILL.md` (`unsafe_code = "deny"`, `unwrap_used = "deny"`, pedantic, `missing_docs`), `rustfmt.toml` `edition = "2024"`, `max_width = 100`, `clippy.toml` test allowances | proven on the spike |
| Targets | darwin/linux × arm64/amd64 | Same release matrix as bungkus-cli |
| Release profile | `strip = true`, `lto = "thin"`, `codegen-units = 1` | 1.2 MB binary in the spike with `strip` alone |

## Direct dependencies (12)

| Crate | Version | Purpose | Alternatives rejected |
|-------|---------|---------|-----------------------|
| `ratatui` | 0.30 | Immediate-mode TUI: layout, `Buffer`/`Cell`, borders, `Color::Rgb`/`Color::Indexed`, our own widgets for panes and cards. Its re-exported **`ratatui::crossterm`** (0.29) is used for raw mode, alternate screen, key/mouse/paste/resize events, **kitty keyboard enhancement flags on the host side** (`PushKeyboardEnhancementFlags`), bracketed paste and synchronized updates — no direct `crossterm` dependency, so the two can never disagree on a version | cursive (retained-mode, heavier); raw crossterm drawing (we'd rewrite ratatui's buffer diff); termion / termwiz backends (no kitty flags / pulls half of wezterm) |
| `alacritty_terminal` | 0.26 | The embedded VT emulator for the output pane: grid, scrollback (`scrolling_history`, `scroll_display`), wide-char flags, mode tracking (DECCKM, 2004, mouse, kitty stack), and **terminal query replies** (DSR, DA, OSC 10/11/12, XTWINOPS) delivered as `Event`s | `wezterm-term` (+`termwiz`): has a key encoder, but **not on crates.io** — git-only dependency on a monorepo, no `cargo deny`/`cargo audit` story; `vt100`: on crates.io but **drops terminal queries**, which agents send at startup (DSR/DA/OSC 11), so every reply would be hand-written; spike verdict in `spikes/rust/README.md` |
| `portable-pty` | 0.9 | Open a PTY, spawn the child with a size, resize (`TIOCSWINSZ`), master reader/writer | `pty-process` (smaller, but tokio-flavoured API and less used); raw `rustix::pty` (we'd own openpty/login_tty/fork details) |
| `thiserror` | 2.0 | One error enum per module boundary (`term::SpawnError`, `ipc::Error`, …) | hand-written `Display`/`Error` impls (boilerplate) |
| `anyhow` | 1.0 | `main.rs` and the top-level subcommand entry points only, with `.context()` | — |
| `serde` (+ `derive`) | 1.0 | Config, state, hook payloads, status-line payload, Codex `token_count` records, TypeSafe request/response — **tolerant structs, never `deny_unknown_fields` for agent payloads** | hand-rolled JSON (no) |
| `serde_json` (+ `preserve_order`) | 1.0 | The JSON codec for all of the above; `from_slice` on capped buffers. `preserve_order` (pulls `indexmap`) keeps the user's key order when the settings screen writes `config.json` back | `simd-json` (unneeded speed, unsafe inside) |
| `uuid` (+ `v4`) | 1.26 | Session ids we choose for Claude (`--session-id`) and strict UUID validation of ids from hooks | a regex (we need generation too) |
| `lexopt` | 0.3 | CLI parsing for `bungkus-mc [workspace]`, `hook`, `statusline`, `update`, and ~6 flags | `clap` (derive): the obvious choice, but it adds ~15 crates and ~600 KB for four subcommands; `lexopt` has zero dependencies and the help text is 30 hand-written lines. Revisit if the CLI grows past two levels |
| `ureq` (rustls) | 3.4 | **Synchronous** HTTPS for the daily release check and the opt-in TypeSafe routing request: timeouts, no redirects, body size caps via ureq's body limit / `Read::take` — no async runtime anywhere in mc. Note: ureq 3's default rustls crypto provider is `ring`, which contains C/asm; that is fine on native runners, and `cargo-zigbuild` handles it, but it is the one place a pure-Rust build assumption breaks (switch to the `aws-lc-rs` or a pure-Rust provider feature only if cross-compiling ever fails) | `reqwest` (pulls tokio/hyper for two requests); `curl` bindings (C dependency); native-tls (platform TLS quirks) |
| `semver` | 1.0 | Compare the running version with the release tag; validate the cached tag before display | hand-written compare |
| `rustix` (+ `process`, `fs`, `event`, `stdio`) | 1.1 | **Safe wrappers** for what the port needs: `kill`/`kill_process_group`, `pidfd_open`/`pidfd_send_signal` (Linux), `getuid` (`process`); `OFlags::NONBLOCK` for the Codex log open (`fs`, nothing else — regular-file checks use std `File::metadata()`); `poll` on stdin for the start-up OSC 11 reply (`event`) — so `unsafe_code` stays denied crate-wide | `nix` (fine, but rustix is the modern, `unsafe`-free-at-the-API choice); `libc` directly (would require our own `unsafe`) |

Transitive crate count in the spike: 118 (7 direct). The stack above adds
`thiserror`, `uuid`, `lexopt`, `ureq`+`rustls`, `semver`, `rustix` —
expect ~160 crates in `Cargo.lock`. `deny.toml` denies duplicate crate
versions (`multiple-versions = "deny"`) with named exceptions, all
inside upstream crates' own trees and listed in `skip` with the reason:
`syn` 2 and 3 (strum/derive_more vs instability) and `hashbrown` 0.16 and
0.17 (kasuari vs lru); from M3 also `signal-hook` 0.3/0.4 (crossterm vs
alacritty_terminal) and `thiserror` 1/2 (portable-pty's `filedescriptor`).
Any other duplicate fails CI. `windows-sys` stays out
because `[graph] targets` is the four unix triples (`aarch64-apple-darwin`,
`x86_64-apple-darwin`, `aarch64-unknown-linux-gnu`,
`x86_64-unknown-linux-gnu`). Every crate here is on crates.io with a
permissive licence; the `cargo deny` allowlist is MIT, Apache-2.0,
Unicode-3.0 (`unicode-ident`, via ratatui) and Zlib (`foldhash`, via
ratatui). Both of the last two are permissive and allow use in proprietary
software with the notice shipped. ISC is added when a crate that needs it
(`ring` via `ureq`, M9) arrives.
bungkus-mc itself is proprietary: `Cargo.toml` sets `license-file = "LICENSE"`
and `publish = false`, and `deny.toml` sets `[licenses.private] ignore = true`
so our own crate isn't checked against the allowlist. The release job
generates `THIRD-PARTY-NOTICES` (dependency licence texts, as MIT/Apache
require) and ships it next to each binary.

## Deliberately not used

| Thing | Why not |
|-------|---------|
| Any async runtime (`tokio`, `smol`) | Every blocking I/O source is a thread sending `AppEvent` over `std::sync::mpsc`; there are at most a dozen threads. An executor would add crates and a second concurrency model for no throughput we need |
| A direct `crossterm` dependency | ratatui re-exports the crossterm it was built against (`ratatui::crossterm`); depending on it twice only creates version-skew bugs |
| `tracing` / `log` + a subscriber | The only consumer is a 0600 debug file under `--debug`. A ~20-line `debug_log!` macro in `store/debug.rs` (timestamp + module + message, `Mutex<Option<File>>`, no-op when the file is not open) replaces two crates and their transitive graph. `print_stdout` stays denied; the macro is the only diagnostic channel |
| `cargo audit` | `cargo deny check advisories` covers the same RustSec database; running both is one more tool to install and keep in sync |
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

- `claude` and/or `codex` on PATH (detected at start; shown in the setup wizard and settings).
- `bash`, `curl` for `bungkus-mc update` (the installer script is reused from bungkus-cli).
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
cargo deny check          # advisories (RustSec), licences, bans, sources (deny.toml)
```

Release (M8, following the OSBR repository template the repo was created
from): pushes to `main` make `prepare-release.yml` (`git-pr-release`) open a
release PR `main` → `release`; merging it runs `release.yml`, which tags
`v<version from Cargo.toml>` (bump it in the release PR; the job refuses a
tag that already exists), builds `--release --locked` for darwin/linux ×
arm64/amd64 on native runners (`macos-latest` for both macOS targets,
`ubuntu-latest` + `ubuntu-24.04-arm` for Linux), and publishes
`bungkus-mc-<os>-<arch>`, `checksums.txt`, `install.sh` and
`THIRD-PARTY-NOTICES` (generated by `cargo-about`, a CI tool, not a
dependency: `about.toml`, `about.hbs`) with release-drafter notes. The tag
is semver, not the template's CalVer, because `bungkus-mc update` compares
versions. The `release` branch must exist before the first release PR.
`install.sh` (bungkus-cli's script with `REPO`/`BIN_NAME` changed; the repo
is public, so it downloads with `curl`) is attached to every release and
served from `main` for the one-line install. `ureq` is only needed for
model routing (M9): the update check and update call `curl`, which the
installer needs anyway, so they add no HTTP code.

After the GitHub release, the announce job posts bungkus-cli's Slack
message (release title, notes, link, the install line) to
`SLACK_WEBHOOK_URL`, skipped while that secret is unset. npm and Homebrew
packaging were dropped (owner decision, 2026-10-01).
