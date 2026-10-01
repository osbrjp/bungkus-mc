//! `install.sh`'s folder choice (ARCHITECTURE §11), run with
//! `BUNGKUS_INSTALL_DRY_RUN=1` against fake PATH folders, so nothing is
//! downloaded or installed.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Returns `(dir, sudo, shadowed)` as the installer decides them.
#[expect(
    clippy::unwrap_used,
    reason = "test helper: a failure should fail the test"
)]
fn decide(home: &Path, path: &str, env: &[(&str, &Path)]) -> (PathBuf, bool, String) {
    let mut cmd = Command::new("bash");
    cmd.arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh"))
        .env_clear()
        .env("HOME", home)
        .env("PATH", format!("{path}:/usr/bin:/bin"))
        .env("BUNGKUS_INSTALL_DRY_RUN", "1");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    let field = |name: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{name}=")))
            .unwrap_or_default()
            .to_owned()
    };
    (
        PathBuf::from(field("dir")),
        field("sudo") == "1",
        field("shadowed"),
    )
}

/// Makes folder `dir` holding an executable `bungkus-mc`.
#[expect(
    clippy::unwrap_used,
    reason = "test helper: a failure should fail the test"
)]
fn with_binary(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let bin = dir.join("bungkus-mc");
    fs::write(&bin, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

#[test]
fn chooses_the_install_folder_without_sudo_where_it_can() {
    let root = std::env::temp_dir().join(format!("mc-install-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let home = root.join("home");
    let local = home.join(".local/bin");
    fs::create_dir_all(&local).unwrap();
    let s = |p: &Path| p.to_string_lossy().into_owned();

    let custom = root.join("custom");
    assert_eq!(
        decide(&home, "", &[("BUNGKUS_INSTALL_DIR", &custom)]),
        (custom, false, String::new()),
        "env override"
    );

    assert_eq!(
        decide(&home, &s(&root.join("empty")), &[]),
        (local.clone(), false, String::new()),
        "fresh install goes to ~/.local/bin"
    );

    let open = root.join("open");
    with_binary(&open);
    assert_eq!(
        decide(&home, &s(&open), &[]),
        (open.clone(), false, String::new()),
        "a writable existing install is updated in place"
    );

    let locked = root.join("locked");
    let old = with_binary(&locked);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    assert_eq!(
        decide(&home, &format!("{}:{}", s(&local), s(&locked)), &[]),
        (local.clone(), false, s(&old)),
        "~/.local/bin earlier on PATH shadows a read-only install"
    );
    assert_eq!(
        decide(&home, &format!("{}:{}", s(&locked), s(&local)), &[]),
        (locked.clone(), true, String::new()),
        "~/.local/bin later on PATH: sudo in place"
    );
    assert_eq!(
        decide(&home, &s(&locked), &[]),
        (locked.clone(), true, String::new()),
        "~/.local/bin not on PATH: sudo in place"
    );

    let fresh_home = root.join("fresh-home");
    fs::create_dir_all(&fresh_home).unwrap();
    let fresh_local = fresh_home.join(".local/bin");
    assert_eq!(
        decide(
            &fresh_home,
            &format!("{}/:{}", s(&fresh_local), s(&locked)),
            &[]
        ),
        (fresh_local, false, s(&old)),
        "a trailing slash on PATH still matches a ~/.local/bin that does not exist yet"
    );

    let real = root.join("real");
    with_binary(&real);
    let links = root.join("links");
    fs::create_dir_all(&links).unwrap();
    symlink(real.join("bungkus-mc"), links.join("bungkus-mc")).unwrap();
    assert_eq!(
        decide(
            &home,
            "",
            &[("BUNGKUS_CURRENT_BIN", &links.join("bungkus-mc"))]
        ),
        (real, false, String::new()),
        "a symlinked current binary updates its target's folder"
    );

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_dir_all(&root).unwrap();
}
