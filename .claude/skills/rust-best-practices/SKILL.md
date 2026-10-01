---
name: rust-best-practices
description: Rules every agent must follow when writing, editing or reviewing Rust in this repo — idiomatic, lint-clean Rust and structured rustdoc comments (JSDoc/Javadoc-style sections). Use for any .rs file, Cargo.toml change, or Rust code review in bungkus-mc.
---

# Rust best practices for bungkus-mc

Follow these strictly. They sit on top of `docs/CODING_RULES.md` (simplicity, YAGNI,
tests next to code) and `docs/SECURITY.md`. If a rule here conflicts with those docs,
the docs win for behaviour and this file wins for Rust style.

Before you say a Rust change is done, all four must pass with no warnings:

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

## 1. Lints and formatting

Put this in `Cargo.toml` (single package, so `[lints]`, not `[workspace.lints]`)
and do not loosen it without a stated reason:

```toml
[lints.rust]
missing_docs = "deny"
unsafe_code = "deny"
unused_must_use = "deny"

[lints.clippy]
pedantic = { level = "warn", priority = -1 }
unwrap_used = "deny"
expect_used = "deny"            # a justified expect carries #[expect(clippy::expect_used, reason = "…")]
panic = "deny"
todo = "deny"
dbg_macro = "deny"
print_stdout = "deny"           # the TUI owns stdout; use the debug log
missing_errors_doc = "warn"
missing_panics_doc = "warn"
```

- `rustfmt` defaults plus `rustfmt.toml`: `edition = "2024"`, `max_width = 100`
  (stable options only; import grouping is kept by hand: std, external, crate).
- `clippy.toml`: `allow-unwrap-in-tests = true`, `allow-expect-in-tests = true`,
  `allow-panic-in-tests = true`, so the `deny` lints above apply to shipped code only.
- Silence a lint only on the item that needs it, with `#[expect(lint, reason = "…")]`
  (it errors once the lint no longer fires). Never put a blanket `allow` on a module.

## 2. Doc comments (rustdoc, structured like JSDoc/Javadoc)

Every public item gets a `///` doc comment. So do non-trivial private functions:
anything with a branch, a loop, I/O, or an invariant. Every module file starts
with a `//!` block that says what the module owns and what it must never do.

Shape of a doc comment:

1. **One-line summary** in the third person, ending with a period
   ("Spawns …", "Returns …").
2. A blank `///` line, then a short paragraph on behaviour, invariants and *why*.
   Link related items with intra-doc links: [`Session`], [`crate::ipc::Event`].
3. Sections, in this order. Include only the ones that apply:
   - `# Arguments`: one bullet per parameter, written `` * `name` - meaning ``.
   - `# Returns`: what comes back and in which cases.
   - `# Errors`: every error variant and when it happens. Required for any `Result`.
   - `# Panics`: required if the function can panic. Better still, make it not panic.
   - `# Safety`: required on every `unsafe fn`.
   - `# Examples`: optional, fenced as ```` ```ignore ````. This is a binary crate, so
     doctests don't run; behaviour is proven in unit tests.

```rust
//! Agent sessions: one PTY, one emulator, one child process per session.
//!
//! This module owns the child's lifetime. It never reads agent transcripts;
//! structure arrives through hook events (see [`crate::ipc`]).

/// Spawns an agent CLI inside a new pseudo-terminal.
///
/// The child gets the scrubbed environment from [`child_env`] and starts
/// at `size`. A reader thread forwards PTY output to the event loop, which
/// feeds the emulator; a writer thread owns the PTY writer, so keys, paste
/// and query replies never block the UI.
///
/// # Arguments
///
/// * `agent` - Which CLI to launch, with its launch options.
/// * `cwd`   - Project directory the agent runs in; must already exist.
/// * `size`  - Initial pane size in cells.
///
/// # Returns
///
/// A [`Session`] that owns the child, the PTY master and the emulator.
/// Dropping it stops the child's process group.
///
/// # Errors
///
/// * [`SpawnError::NotFound`] - the agent binary is not on `PATH`.
/// * [`SpawnError::Pty`] - the PTY could not be opened or resized.
///
/// # Examples
///
/// ```ignore
/// let session = Session::spawn(&Agent::claude(), Path::new("/work/kedai-web"), Size::new(80, 24))?;
/// ```
pub fn spawn(agent: &Agent, cwd: &Path, size: Size) -> Result<Session, SpawnError> {
    // …
}
```

Types and fields:

```rust
/// Usage figures for one session, as last reported by the agent.
///
/// Every field is optional: agents report nothing until their first reply,
/// and an unknown value renders as `-` (see DESIGN §6).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    /// Total input tokens, including cache reads and writes.
    pub input_tokens: Option<u64>,
    /// Estimated cost in USD at list price; `None` for Codex.
    pub cost_usd: Option<f64>,
}
```

The documentation effort goes into doc comments. Keep inline `//` comments rare:
write one only when the *why* isn't obvious from the code, such as a protocol quirk,
a limit that has a reason, or a workaround with its cause.

**No agent chatter.** Never write comments that:
- narrate an edit ("added", "changed to", "now uses", "fixed per review");
- restate what the next line does;
- talk to the reader or reviewer;
- credit a tool.

Change history goes in commit messages, not in code. A `// TODO` names a
milestone or issue: `// TODO(M6): …`.

## 3. Types and API design (Rust API Guidelines)

- **Naming follows the Rust API Guidelines.**
  - `snake_case` for functions and modules, `UpperCamelCase` for types, `SCREAMING_SNAKE_CASE` for consts.
  - Conversions are named `as_`/`to_`/`into_`, constructors `new`/`with_*`/`try_*`, getters with no `get_` prefix.
- **Newtypes for ids and units:** `SessionId(Uuid)`, `Pid(i32)`, `Cells(u16)`. Never pass a bare `String` or `u32` that carries meaning.
- **Enums over bools** for modes and states (`Focus::Output`, not `interact: bool`).
  - `match` on our own enums is exhaustive, with no `_ =>` arm, so adding a variant breaks the build where it matters.
- **`#[must_use]`** on functions whose result is meaningful. `#[non_exhaustive]` only on types other crates see.
- **Borrow in parameters** (`&str`, `&Path`, `&[T]`, `impl AsRef<Path>`), return owned values. Don't `clone()` just to silence the borrow checker; restructure instead.
- Derive `Debug` everywhere. `Clone`/`PartialEq`/`Eq`/`Hash`/`Default` only when used.
- **Keep `pub` narrow:** `pub(crate)` by default, `pub` only for the real module API.

## 4. Errors

- **Library modules** define error enums with `thiserror`: one enum per module boundary, with variants that say what failed. Keep the source with `#[source]` / `#[from]`.
- **`anyhow` only in `main.rs`** and top-level command handlers. Always add `.context("what we were doing")`.
- **No `unwrap()` outside tests.**
  - `expect("invariant: …")` only where the invariant is local and obvious, under
    `#[expect(clippy::expect_used, reason = "…")]`.
  - Parsing untrusted input (hook payloads, `ps` output, Codex logs, API responses) never panics. It returns an error or skips the record.
- **Nothing ignored silently.** A deliberate ignore is `let _ = x; // reason: …`.

## 5. Concurrency and I/O

- **One UI event loop owns all app state** (the Elm-style model); other threads send messages over `std::sync::mpsc` channels.
  - Reach for `Arc<Mutex<_>>` only when a channel is clearly worse, and document the lock order.
- **Blocking I/O** (PTY reads, `ps`, file tails) runs on dedicated threads. Do not add an async runtime unless a milestone needs one.
- **Processes** are spawned with an argument vector (`std::process::Command`), never through a shell. The documented hook-command exception lives in one function.
- **Timeouts and caps are consts** with a doc comment naming the source: `/// 1 MiB, per SECURITY.md "Hook payloads".`

## 6. `unsafe`

Denied. Use `rustix` safe wrappers and std APIs (e.g. `CommandExt::process_group`,
not `pre_exec`). Any exception follows SECURITY.md: one small module that allows
`unsafe_code`, with a `// SAFETY:` comment above every `unsafe` block stating why
each precondition holds.

## 7. Tests

- **Unit tests** go in `#[cfg(test)] mod tests` at the bottom of the file they test. They are table-driven where there are several cases.
- **Test names read as sentences:** `rejects_path_outside_codex_home`.
- **Fixtures** live in `testdata/` next to the module. Recorded hook payloads and `ps` output are scrubbed of personal data.
- **Rendering goldens** compare plain text plus cursor. Update them only on purpose (`UPDATE_GOLDEN=1`).
- `unwrap()` is fine in tests. Add a message when the failure would be confusing.

## 8. Dependencies

- **Before adding a crate,** check whether std or an existing dependency already covers it. Every new crate needs a line in `docs/TECH_STACK.md`: why it's there and what alternatives were rejected.
- **Version pinning:** `Cargo.lock` is committed and pins everything; 0.x crates are
  minor-pinned by their caret requirement.
- **Audit on every change:** `cargo deny check` (advisories, licences, bans, sources) runs
  in CI, under the remediation windows in SECURITY.md.

## 9. Review checklist

- [ ] The four commands at the top pass with no warnings.
- [ ] Every public item and every non-trivial function has a structured doc comment, including `# Errors` / `# Panics` where they apply.
- [ ] No `unwrap`, no `panic!` and no `todo!` in non-test code. Untrusted input can't panic.
- [ ] Types carry meaning (newtypes, enums); matches on our own enums are exhaustive.
- [ ] No new dependency without a TECH_STACK entry. `cargo deny` is clean.
- [ ] Inline comments are rare and explain why. No edit narration, restated code or agent chatter.
