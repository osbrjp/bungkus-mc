#!/usr/bin/env python3
"""Runs each spike case in tmux (reference) and in each prototype, then scores them.

Usage: python3 spikes/compare.py [case ...]    (default: every case in spikes/cases)
Output: spikes/out/<impl>/<case>/..., and a summary table on stdout.
"""
import difflib, json, os, shlex, subprocess, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parent
COLS, ROWS = 100, 30
IMPLS = {
    "go": [str(ROOT / "go/bin/proto")],
    "rust": [str(ROOT / "rust/target/release/proto")],
}
TMUX = ["tmux", "-L", "bungkus-spike", "-f", "/dev/null"]
TMUX_KEYS = {"enter": "Enter", "shift+enter": "S-Enter", "esc": "Escape", "tab": "Tab",
             "shift+tab": "BTab", "up": "Up", "down": "Down", "left": "Left", "right": "Right",
             "ctrl+c": "C-c", "ctrl+d": "C-d", "backspace": "BSpace"}


def tmux(*args, check=True):
    return subprocess.run(TMUX + list(args), capture_output=True, text=True, check=check).stdout


def run_tmux(case, out):
    """Replays a case in a private tmux server and writes dumps like the prototypes do."""
    out.mkdir(parents=True, exist_ok=True)
    tmux("kill-server", check=False)
    env = " ".join(f"{k}={shlex.quote(v)}" for k, v in case["env"].items())
    cmd = f"env -u TMUX -u TMUX_PANE {env} " + " ".join(shlex.quote(a) for a in case["cmd"])
    # Start a placeholder, set options, then respawn the real command so a fast
    # exit can't beat remain-on-exit and the final screen stays capturable.
    tmux("new-session", "-d", "-s", "ref", "-x", str(COLS), "-y", str(ROWS), "-c", case["cwd"], "sleep 600")
    tmux("set", "-g", "remain-on-exit", "on")
    tmux("set", "-g", "remain-on-exit-format", "")
    tmux("set", "-s", "extended-keys", "on")
    start = time.monotonic()
    tmux("respawn-pane", "-k", "-t", "ref", "-c", case["cwd"], cmd)
    for step in case["steps"]:
        if "wait" in step:
            time.sleep(step["wait"] / 1000)
        elif "bytes" in step:
            tmux("send-keys", "-t", "ref", "-H", *[f"{b:02x}" for b in step["bytes"].encode()])
        elif "key" in step:
            tmux("send-keys", "-t", "ref", TMUX_KEYS[step["key"]])
        elif "paste" in step:
            tmux("set-buffer", "-b", "p", step["paste"])
            tmux("paste-buffer", "-p", "-b", "p", "-t", "ref")
        elif "resize" in step:
            c, r = step["resize"]
            tmux("resize-window", "-t", "ref", "-x", str(c), "-y", str(r))
        elif "dump" in step:
            text = tmux("capture-pane", "-p", "-t", "ref")
            rows = int(tmux("display", "-p", "-t", "ref", "#{pane_height}"))
            lines = [l.rstrip() for l in text.split("\n")][:rows]
            lines += [""] * (rows - len(lines))
            (out / f"{step['dump']}.txt").write_text("\n".join(lines) + "\n")
            (out / f"{step['dump']}.cursor").write_text(tmux("display", "-p", "-t", "ref", "#{cursor_y} #{cursor_x}"))
            alt = tmux("display", "-p", "-t", "ref", "#{alternate_on}").strip() == "1"
            (out / f"{step['dump']}.meta.json").write_text(json.dumps({"alt_screen": alt}))
        elif "waitexit" in step:
            deadline = time.monotonic() + step["waitexit"] / 1000
            while time.monotonic() < deadline:
                if tmux("display", "-p", "-t", "ref", "#{pane_dead}", check=False).strip() == "1":
                    break
                time.sleep(0.05)
    (out / "timing.json").write_text(json.dumps({"wall_ms": round((time.monotonic() - start) * 1000)}))
    tmux("kill-server", check=False)


def run_impl(binary, case_path, out):
    out.mkdir(parents=True, exist_ok=True)
    t = time.monotonic()
    p = subprocess.run(binary + ["--headless", "--cols", str(COLS), "--rows", str(ROWS),
                                 "--case", str(case_path), "--out", str(out)],
                       capture_output=True, text=True, timeout=180)
    (out / "run.log").write_text(p.stdout + p.stderr)
    return p.returncode, round((time.monotonic() - t) * 1000)


def score(ref_dir, dir_):
    """Per dump: share of rows identical to tmux, whole-screen similarity, and cursor match."""
    res = {}
    for ref in sorted(ref_dir.glob("*.txt")):
        mine = dir_ / ref.name
        if not mine.exists():
            res[ref.stem] = "missing"
            continue
        a, b = ref.read_text().split("\n"), mine.read_text().split("\n")
        rows = sum(x == y for x, y in zip(a, b)) / max(len(a), 1)
        sim = difflib.SequenceMatcher(None, "\n".join(a), "\n".join(b)).ratio()
        cur = (ref_dir / f"{ref.stem}.cursor").read_text().split() == \
              ((dir_ / f"{ref.stem}.cursor").read_text().split() if (dir_ / f"{ref.stem}.cursor").exists() else None)
        res[ref.stem] = f"rows {rows:.0%} sim {sim:.0%} cursor {'ok' if cur else 'diff'}"
    return res


def main():
    names = sys.argv[1:] or sorted(p.stem for p in (ROOT / "cases").glob("*.json"))
    for name in names:
        case_path = ROOT / "cases" / f"{name}.json"
        case = json.loads(case_path.read_text())
        ref = ROOT / "out/tmux" / name
        run_tmux(case, ref)
        ref_ms = json.loads((ref / "timing.json").read_text())["wall_ms"]
        print(f"\n== {name}  (tmux {ref_ms} ms)")
        for impl, binary in IMPLS.items():
            if not Path(binary[0]).exists():
                print(f"  {impl:5} not built")
                continue
            d = ROOT / "out" / impl / name
            try:
                code, ms = run_impl(binary, case_path, d)
            except subprocess.TimeoutExpired:
                print(f"  {impl:5} TIMEOUT")
                continue
            t = d / "timing.json"
            inner = json.loads(t.read_text()).get("wall_ms") if t.exists() else None
            print(f"  {impl:5} exit {code}  wall {inner} ms (process {ms} ms)")
            for dump, s in score(ref, d).items():
                print(f"        {dump:12} {s}")


if __name__ == "__main__":
    main()
