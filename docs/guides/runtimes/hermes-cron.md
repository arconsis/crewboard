# Hermes cron job for crewboard

Goal: Hermes polls the board every 5 minutes with a program, and starts the model only
when events are pending. A quiet board costs zero tokens.

How Hermes runs a job:

- A job has `script` (a file under `~/.hermes/scripts/`), `no_agent`, and a `schedule`
  (`{"kind": "interval", "minutes": 15}` or `{"kind": "cron", "expr": "*/5 * * * *"}`).
- With `--no-agent` the script IS the job. Its stdout is delivered. Empty stdout is silent.
  `.sh` files run under bash.
- Hermes can run one prompt non-interactively: `hermes chat -Q -s <skill> -q "<prompt>"`
  (`-Q` quiet, `-s` preload a skill).

So the job is a no-agent job. The script is the wrapper of `crewboard wake --runtime hermes`:
it polls with `crewboard`, and only when there is work it runs the Hermes one-shot with the
same wake prompt as for Claude Code and Codex (one template, in `crewboard wake`).

## What was tested

On macOS with Hermes v0.22.2, 2026-10-16 and 2026-10-18:

- The job format above was read from `~/.hermes/cron/jobs.json` and `hermes cron create --help`.
  `hermes chat --help` lists `-Q`, `-s` and `-q`.
- `hermes chat -Q -s crewboard -q "Use the crewboard skill. ... run crewboard --version ..." </dev/null`
  printed `crewboard 0.1.0` and a `session_id:` line, exit 0, in 23 s.
- `crewboard wake --runtime hermes` with a fake `hermes` and a fake `crewboard`:
  `loops/tests/test-loops.sh`, case A9. `wake install --scheduler hermes` with a temporary HOME:
  `tests/component/wake_install.rs`.

Not tested: Linux, and a full cron wake of a real Hermes model on a real board.

Hermes cron stops a wake at its job timeout (3600 s). The wrapper stops a wake at its own
time budget first (`--max-wake`, default 45m), and posts the state to the coordinator itself.

## Steps on the Hermes host

Assumptions: the skill is installed with `crewboard skill install` (the loop scripts are
then in `~/.config/agent-skills/crewboard/loops/`), the env file is
`~/.config/crewboard/<initiative>.env` with mode 600, and the agent `<agent>` (for example
`hermes@host-b`) was bootstrapped and approved. `<work-dir>` is the checkout of the agent.

0. **Bootstrap the agent first.** One command makes the agent dir, the join, the gh shim, the
   work dir and its git identity (`crewboard agent add ... --runtime hermes`; see
   [Agents guide](../agents.md#join-an-agent), steps 2 to 4).
   Hermes needs the shim (the general rule is in [gh-shim.md](gh-shim.md)). Hermes strips
   `GH_TOKEN` (and `GITHUB_TOKEN`) from every command that the model runs, by a fixed rule with
   no passthrough. Without the shim, the model's own `gh pr create` and `git push` fall back to
   the owner's `gh` login on the same host, and its PRs and comments carry the owner's name. A
   headerless comment from the owner login is an owner message to the coordinator, so this is a
   security problem, not a style one. `crewboard start` refuses such an agent (rule
   `agent-gh-not-crew`).

   The shim runs the real `gh` through `crewboard agent with-env`, so the agent token is loaded for that call
   only. A `gh` that bypasses the shim finds the empty config dir and fails ("not logged in").
   Do not write "your token is in the environment" in a wake prompt: for Hermes it is not.

1. **Install the wrapper.** It goes to `~/.hermes/scripts/` (Hermes runs scripts only from
   there), with the absolute path of `hermes`:

```sh
crewboard wake install --scheduler hermes --runtime hermes \
  --agent-dir ~/.config/crewboard/agents/<agent-dir> \
  --env-file ~/.config/crewboard/<initiative>.env --work-dir <work-dir> \
  --shim-dir ~/.config/crewboard/agents/<agent-dir>/bin
```

   `--scheduler auto` also picks Hermes for `--runtime hermes`. The command prints the log path
   and the job command (the one command that you run next). Check the wake command first with
   `crewboard wake <the same options> --print-cmd` (with `--agent-dir` and `--env-file`)
   instead of `wake install`.

2. **Create the job** with the printed command (every 5 minutes, no model on a quiet board):

```sh
hermes cron create --name crewboard-<initiative>-<agent-dir> --no-agent \
  --script crewboard-<initiative>-<agent-dir>-wake.sh --deliver local \
  --failure-deliver origin "every 5m"
```

   `--deliver local` keeps stdout out of chat (it is always empty anyway).
   `--failure-deliver origin` sends a notice when the script exits non-zero (crewboard exit 2,
   3 or 4, or a failed wake). An exit 4 stops the wakes, so you get that notice once.

3. **Check the log once** after the first run:

```sh
hermes cron list
tail -n 20 ~/.local/state/crewboard/agents/<initiative>-<agent-dir>/wake.log
```

   A quiet board writes at most one line per hour: `quiet: <N> polls since <time>`. A wake
   writes `events pending; waking: ...` and `wake finished OK`. A failing `crewboard poll`
   writes its exit code and the first line of its error.

## Stop, pause, remove

```sh
hermes cron pause <job-id>      # stop polling; resume with: hermes cron resume <job-id>
hermes cron remove <job-id>     # delete the job, then:
crewboard wake uninstall --scheduler hermes --agent-dir ~/.config/crewboard/agents/<agent-dir>
```

Also run `crewboard pause` (agent state) so the coordinator assigns nothing while you are away.

## Notes

- **The token path of the model.** The wake sets `CREWBOARD_CONFIG_DIR`, `GH_CONFIG_DIR` and a
  `PATH` with the `gh` shim of step 0 first on the runtime process. A runtime that runs the
  model's commands in another process does not pass them on. Some Hermes setups run them in the
  gateway. The prompt therefore names the agent dir, and the model adds `--config-dir` itself.
  After the install, start one test wake and check that the model's `env | grep CREWBOARD`
  shows the agent dir. Start a long-running Hermes gateway from a clean shell, never from a
  wake: it would keep the environment of that wake.
- **What one wake does** (the events file, the work back-off, the heartbeat, the time budget,
  exit 3 and exit 4) is in [unattended-claude.md](unattended-claude.md#what-one-wake-does). It is
  the same wrapper for every runtime.
- **Host lock.** Add `--host-lock <lock-dir>` to the install command when the agent runs heavy
  jobs on a shared host. The prompt then tells the agent to use `crewboard lock run`, and the
  wrapper releases the lock of the agent at the end of the time budget. Test:
  `bash skill/loops/tests/test-locks.sh`.
- **No background reports into a wake.** `hermes chat -q` hands the prompt to the gateway, and
  the model turn runs there. A background process with `notify_on_complete` or `watch_patterns`
  that finishes after the turn makes the gateway start a new turn in that chat. The wake has
  then ended and released its lock, so the next wake works in the same checkout at the same
  time. The wake prompt and the skill forbid such processes. Also turn the reports off in
  `~/.hermes/config.yaml` of the gateway that runs wakes:

  ```yaml
  terminal:
    process_wait_auto_notify: false        # no automatic notify_on_complete after two waits
  display:
    background_process_notifications: off  # no report turns at all
  ```

  Both apply to every chat of that gateway. Hermes reads them at each use, so no restart is
  needed. A long job goes through `crewboard lock run ... --detach`: its end is an event of the
  next wake.
- Do not put tokens in the script or in `jobs.json`. The env file holds them (mode 600).
