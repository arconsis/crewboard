# Headless Codex dev agent

Goal: an unattended Codex CLI dev agent. A scheduler runs a small wake script every 5 minutes.
The script polls the board with a program and starts `codex exec` only when there is something
to do. A quiet board costs zero tokens. The Claude Code version is
[headless-claude.md](headless-claude.md). Read its parts "What one wake does" and "Before the
install" too: this guide does not repeat them.

`crewboard wake` runs the wake, and `crewboard wake install` installs the scheduler unit. The wake
gives the wake command the other keys of the env file (for example a model API key).

## The short path

```sh
crewboard wake install --runtime codex --agent-dir ~/.config/crewboard/agents/<agent-dir> \
  --env-file ~/.config/crewboard/<initiative>.env --work-dir <work-dir> \
  --shim-dir ~/.config/crewboard/agents/<agent-dir>/bin
```

`--scheduler auto` (default) picks launchd on macOS and systemd or cron on Linux. The command writes
the wrapper (with the absolute path of `codex`) and the unit, and prints the enable command and
the log path. Run the enable command, and after the first run read the log once:
`tail -n 20 ~/.local/state/crewboard/agents/<initiative>-<agent-dir>/wake.log`.

## What was tested

Codex CLI 0.158.0 is installed on the macOS machine of this test (2026-10-17), but its account
there could not run a model (the account refused every model that was tried). So **no Codex
run was tested end to end**. Marked below:

- Tested on macOS: the flags `exec`, `exec resume`, `--sandbox`, `--add-dir`, `-C`,
  `-c key=value`, `--skip-git-repo-check`, `--ephemeral` and
  `--dangerously-bypass-approvals-and-sandbox` exist in `codex exec --help` (the parser
  accepts them). `codex exec` has **no** `--ask-for-approval` flag.
- Tested on macOS: `crewboard wake --runtime codex` builds the right command for a fake `codex`
  (`loops/tests/test-loops.sh`, cases A2, A6, A8, IN1 and IN3).
- **Untested** (from the Codex docs and `--help` text): that `codex exec` runs without approval
  prompts by default, that `workspace-write` blocks the network until
  `sandbox_workspace_write.network_access=true` is set, that `--add-dir <config-dir>` lets
  `crewboard` write its state there, and which environment variables the shell tool keeps.
  Check these on your host with the commands in "Check before you rely on it".
- Untested: Linux, cron and systemd with a real Codex.

## Does Codex keep `GH_TOKEN`?

Unknown. Codex has a `shell_environment_policy` setting. Its documented default drops variables
whose name contains `KEY`, `SECRET` or `TOKEN`. That would drop `GH_TOKEN`. Rule: **use the gh
shim** ([gh-shim.md](gh-shim.md)). The shim works in both cases. Without it, the model's `gh`
and `git push` can fall back to the owner's login, and `crewboard start` refuses the agent with
`agent-gh-not-crew`.

## Check before you rely on it

Run these once in `<work-dir>` with the wake environment (the env file loaded, shim on PATH):

```sh
GH_TOKEN=probe codex exec --sandbox workspace-write "Run: printenv GH_TOKEN | wc -c. Say the number only."
codex exec --sandbox workspace-write "Run: git ls-remote origin HEAD. Say OK or the error."
codex exec --sandbox workspace-write --add-dir ~/.config/crewboard/agents/<agent-dir> \
  "Run: crewboard --config-dir ~/.config/crewboard/agents/<agent-dir> mine. Say OK or the error."
```

A `0` or `1` in the first answer means Codex strips the token. A network error in the second
means the sandbox blocks the network. A write error in the third means the sandbox blocks the
config dir.

## Flags for an unattended run

`codex exec` runs one prompt and exits. Nobody can answer an approval prompt, so choose one:

1. **Sandbox with network (default of `crewboard wake`).**
   `--sandbox workspace-write -c sandbox_workspace_write.network_access=true`.
   Commands may write in the work dir and use the network. `crewboard wake` always adds
   `--add-dir <config-dir>` (crewboard writes its cursor and key state there) and
   `--add-dir <parent of each --host-lock dir>`. Writes elsewhere are blocked.
2. **No sandbox.** `--dangerously-bypass-approvals-and-sandbox`. Codex says: only for an
   environment that is already sandboxed (container or VM). Use it only there.
3. `--sandbox danger-full-access` removes the sandbox only. Same warning.

Do not use `--sandbox read-only`: the agent cannot edit or push. Pass your own flags with
`--runtime-flags '...'`; the `--add-dir` flags are added in both cases. `--allow` is for Claude
Code only. `--model <id>` picks the model (it works for every runtime). Other flag: `--ephemeral` keeps
no session file.

## Session: a fresh `codex exec` per wake

The same decision as for Claude Code: the board is the memory, so start a fresh `codex exec`
for every wake. `codex exec resume` exists, but a growing session costs more and one damaged
session blocks all wakes. `crewboard wake` does not build it.

## Before the install

The same as in [headless-claude.md](headless-claude.md#before-the-install-once-per-agent).
Codex must be logged in for the user that the scheduler runs as (`codex login`). Install the
skill for Codex too (`crewboard skill install --dry-run` shows the agent-skills directory it links; check that
Codex reads it, or write the rules into `AGENTS.md` of the work dir). The crew token reaches
the model's `crewboard` calls through the gh shim: `crewboard` finds no `GH_TOKEN`, runs
`gh auth token`, and the first `gh` on the `PATH` is the shim.

## Stop, pause, remove

Run `crewboard pause`, then `crewboard wake uninstall --agent-dir <agent dir>`.
