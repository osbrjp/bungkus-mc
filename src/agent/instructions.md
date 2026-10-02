# bungkus-mc session rules

You were started from bungkus-mc (mc), a terminal screen that runs several agent sessions side by side. Other sessions may be working in the same repository right now. Follow these rules unless the workspace's own instructions (after this text) or the user say otherwise.

## Session id

- mc shows every session on its card as `<agent letter> #<4 hex> <name>`, for example `C #12bc fix login`. People refer to sessions by that `#` number.
- Your own number: the first four characters of the environment variable `BUNGKUS_MC_SESSION` (`echo "#${BUNGKUS_MC_SESSION:0:4}"`).
- Another session's number: look it up in `${XDG_STATE_HOME:-$HOME/.local/state}/bungkus/mc/sessions.json`, a JSON array. Find the entry whose `id` starts with those four characters and read its `name`, `agent` and `cwd`; ignore `status`. Never edit that file.
- Sessions are addressed by name, not by number: use the `name` you found to pick the session from your list of peer sessions. Long names are cut with `…`, so match on the start.
- When you say who you are, give both your `#` number and your session name.

## Branching

- Start every new branch from the up-to-date default branch (`git fetch`, then branch from `origin/<default branch>`).
- Branch from something else only when the user, or another session's handoff, names a different base.

## Worktrees

- Name every git worktree you create `<project-name>-<session-id>`: the project's folder name, then your four-character number without the `#` (for example `my-app-12bc`). Create it under the project's `.claude/worktrees/` folder. The branch keeps its own name.
- One worktree per session; if you need a second one in the same project, ask the user for its name.
- When the work in a worktree is merged or abandoned, remove the worktree with `git worktree remove <path>` and no `--force`. If it holds uncommitted or untracked files, stop and say so instead of deleting them. If you are running inside it, remove it as your last step from the main checkout.
- Keep the branch unless the user asks to delete it.
