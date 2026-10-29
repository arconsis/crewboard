# Headless Claude Code dev agent

Goal: an unattended Claude Code dev agent. A scheduler runs a small wake script every 5
minutes. The script polls the board with a program and starts `claude -p` only when there is
something to do. A quiet board costs zero tokens. This guide is the model for a runtime
guide, like [hermes-cron.md](hermes-cron.md). The Codex version is [codex.md](codex.md).

## The short path

```sh
crewboard wake install --runtime claude --agent-dir ~/.config/crewboard/agents/<agent-dir> \
  --env-file ~/.config/crewboard/<initiative>.env --work-dir <work-dir> \
  --shim-dir ~/.config/crewboard/agents/<agent-dir>/bin --allow 'Bash(<build tool> *)'
```

The agent name and the initiative come from the agent dir. `--scheduler auto` (default) picks
launchd on macOS and systemd or cron on Linux; name one with `--scheduler launchd|systemd|cron`.
The command writes two files and enables the unit:

- the wrapper `<agent-dir>/crewboard-<initiative>-<agent-dir>-wake.sh` It holds the absolute
  path of `claude` and of `crewboard`, and the `PATH` of the shell that ran `wake install` (an npm
  or nvm install is not on the `PATH` of a scheduler);
- the scheduler unit with the same name (a LaunchAgent plist, a systemd service and timer, or
  one crontab line).

It prints the log path and, if one more step is needed (on Linux `loginctl enable-linger
"$USER"`, so that the timer runs after logout), that one command. After the first run, read the
log once:

```sh
tail -n 20 ~/.local/state/crewboard/agents/<initiative>-<agent-dir>/wake.log
```

Run `wake install` again after a change (a new flag, a moved `claude`): it replaces the unit.
`--dry-run` prints the unit and the commands and changes nothing. If a job of another name already wakes the same agent (a
hand-made LaunchAgent with a `poll.sh`), `wake install` refuses with exit 2 and
names each job; add `--replace` to unload and remove them (a copy stays as `<file>.bak-<date>`). `crewboard wake status --agent-dir <dir>` shows the unit
and the last run. `crewboard wake uninstall --agent-dir <dir>` disables the unit and removes the
files. The log stays.

## Install from `agent add`

`crewboard agent add --install-wake [--scheduler auto|launchd|systemd|cron] [--every 5m]
[--allow <tool>]` runs `wake install` for a new dev agent after the start checks. It refuses when
another job already wakes this agent, and prints the fix line. It never passes `--replace` on its
own. For other options (`--model`) run `wake install` yourself: the last line of the second
`agent add` run (without `--install-wake`) prints the command with your values.

- `--scheduler` takes `auto` (default), `launchd` (macOS), `systemd` or `cron` (Linux) and
  `hermes`. `auto` picks launchd on macOS, systemd when `systemctl --user` works, else cron.
- `--shim-dir` is necessary. Without it, the wake runs plain `gh` as the owner
  ([gh-shim.md](gh-shim.md)).
- `--allow` adds one build tool to the allow list of Claude Code (repeatable). Codex has no
  `--allow`.

The scheduler starts a wake every 5 minutes with a minimal `PATH`. The scripts add `~/.local/bin`
and `~/.cargo/bin` (the install dirs of the `crewboard` binary, not of your project's tools) to
the end of it. One lock per agent stops two wakes of one agent from overlapping. A quiet board
starts no model, so a broken wake command shows only in the log.

## What was tested

Tested on macOS (Darwin, Claude Code 2.1.302), 2026-10-17 and 2026-10-18:

- The flags `-p`, `--session-id`, `--resume`, `--permission-mode dontAsk`, `--allowedTools`,
  `--add-dir`, `--permission-prompts none` and `--max-budget-usd` exist in `claude --help`.
- `--allowedTools <tools...>` takes a list, so it also takes the next word as a tool.
  `crewboard wake` puts `--` before the prompt.
- A fresh `claude -p` with `--permission-mode dontAsk` and `--allowedTools "Bash(mkdir *)"`
  ran `mkdir`. The same run without that rule denied it.
- `claude -p` keeps `GH_TOKEN` in the shell that the model uses, finds the installed skill
  without a flag, and continues a session with `--resume <uuid>`.
- A launchd user agent with `StartInterval` ran a script on a timer.
- Tests with fake `claude`, `crewboard`, `launchctl`, `systemctl`, `loginctl` and `crontab`:
  `loops/tests/test-loops.sh` and `tests/component/wake_install.rs`. No case installs a real unit.

Not tested: a real systemd or cron job on Linux, a full wake with a real board, and
`--permission-mode bypassPermissions`.

## What one wake does

1. `crewboard poll --quiet`. No events and no owned work: exit 0, no model.
2. Events, or owned work after the back-off (`--work-backoff`, default 15 min; `mine --quiet`
   exit 1 means that every owned ticket only waits): the wrapper takes the lock of the agent.
3. The wrapper runs `crewboard poll` (it moves the cursor, as `watch` does) and
   `crewboard mine`, and writes both to `<state-dir>/events.txt`. The prompt says "Your events
   are in <file>; handle them." So the model does not need a first `watch`.
4. It starts `claude -p` in the work dir, under the time budget (`--max-wake`, default 45m), and
   sends a heartbeat every 5 minutes.
5. At the time budget the wrapper stops the model. Then the wrapper, not the model, posts one
   `say --kind info` to the coordinator on the In Progress ticket (ticket, branch, head SHA,
   pushed or not, "stopped at the time budget") and releases each `--host-lock` dir of the
   agent. The branch and the head come from the work dir, or from a clone right under it that
   is on the ticket branch (`<prefix><agent>/<n>-<slug>`). The PR comes from `crewboard mine`
   or, else, from `gh pr list`. Before the `say`, the wrapper reads the unread events
   (`crewboard poll`) into `<state-dir>/events.txt.unhandled`, so the message needs no
   `--override-unread` and has no override line. The next wake gets those events first.
   After a wake with exit 0, an event that needs an action and got none (no signed message of
   the agent on its ticket) goes once to the same file (`crewboard acted`).
6. A crewboard exit 3 is retried (after 10 s, then after 30 s). Exit 3 in two runs in a row
   pauses the next runs (5 min, doubled, at most 1 h). Exit 4 stops the wakes: one line in the
   log and on stderr, and one `say` to the coordinator if crewboard can still post. The wakes
   start again when the env file or the agent config changes, or when you remove
   `<state-dir>/stopped`.

The state dir is per agent: `~/.local/state/crewboard/agents/<initiative>-<agent-dir>/`. It has
the lock, the back-off files, `events.txt` and the log `wake.log`. Two agents on one host never
share it.

## New events during a wake (the hook)

A Claude wake gets a `PostToolUse` hook for this wake only (`--settings`; it changes no settings
file of yours). Claude Code runs `crewboard hook post-tool-use` after each tool call. The hook
polls the board at most once per interval (`--events-every S`, default 300; `0` polls after every
call; `--no-events-hook` turns the hook off). The cursor moves with the poll, as in the wrapper.

- The hook reads the events file from `$CREWBOARD_EVENTS_FILE`. Outside a wake it does nothing.
- The last poll is the stamp `<events file>.hook-stamp`. Without a stamp, the age of the events
  file counts, so the poll of the wrapper at the wake start is not repeated at once.
- New events go to the end of the events file (`== Events delivered during the wake`), so the
  receipts see them, and to the model as `additionalContext` (cut at 9000 characters; the events
  file has the rest).
- No events, a failed poll or no wake: no output. The hook always exits 0 and never blocks a tool
  call. A failed poll is one line in the log (`wake-events-hook: crewboard poll failed with exit N`).
- A headless config dir may run it: it reads the board and appends to the events file of the wake.

## The wake command (`crewboard wake --runtime`)

The hidden `crewboard wake --runtime claude|codex|hermes --agent A --initiative I --work-dir D`
builds the wake command itself: the runtime, its permission flags, the writable
dirs, the prompt and (Claude) the events hook. It starts the runtime as a program with an argument
list, in the work dir, with no `sh -c` and no quoting. `--print-cmd` prints the command and starts
nothing: `cd <work dir> && <words>`, each word quoted only when a shell needs it. The options are
those of `wake install`: `--model`, `--allow` (claude), `--runtime-flags`, `--runtime-bin`,
`--prompt-extra`, `--host-lock`, `--shim-dir`, `--no-events-hook`, `--events-every`. The events
file is `<state dir>/events.txt`. The wake gets `CREWBOARD_WAKE=1` and
`CREWBOARD_LOCK_RUN_DIRS` (the `--host-lock` dirs). The prompt names `crewboard lock run`, not
the script. The same command keeps the heartbeat and the time budget. `--heartbeat-every S` (default 300,
or `$CREWBOARD_HEARTBEAT_EVERY`; 0 sends none) sends `crewboard heartbeat` from a thread of its
own while the wake runs. `--max-wake DUR|off` (default 45m, or `$CREWBOARD_MAX_WAKE`) stops the
wake at its budget: TERM to the process group and to every descendant, a grace of
`$CREWBOARD_KILL_GRACE` (10 s) while the group lives, then KILL. The KILL goes only to a process
that is still the one that got the TERM (same pid and start time): a pid that another process took
in the meantime is left alone. A job that `crewboard lock run
--detach` started has its own session and is not reached. Then `wake` does the end itself, not the
model: one `say` to the coordinator on the In Progress ticket (branch, head SHA, pushed or not,
the PR; with an events file the unread events go to `<events file>.unhandled` first), and the
release of each `--host-lock` dir that `--agent` holds, except a lock of a detached or running
job.
A wake that ends at its budget counts as a failed wake for its events: with an events file, the
file goes to `<events file>.unhandled`, so the next tick wakes the agent again. When the work dir
(or a checkout right under it) has uncommitted changes, the kept block starts with "The last wake
hit the time budget. Uncommitted work in <dir>: commit or continue it first." and the message to
the coordinator names the dirty work tree. The wrapper only reads the tree (`git status`): it never
commits, stages or pushes (`git --no-optional-locks`, 30 s per call; a call that cannot run is
a log line). A ticket that hits the budget at every wake would cost a wake per tick: the first
budget stop keeps the events and wakes again at the next tick; a second stop in a row on the same
ticket keeps them but waits for the normal `--work-backoff` before the next wake (new events still
wake at once), and its message to the coordinator adds "hit the time budget twice in a row: split
the ticket or raise --max-wake". The count is `<state dir>/budget-streak`; a wake that ends by
itself removes it. A TERM to the wrapper stops the wake group and releases the host locks too.
`crewboard wake install|uninstall|status` manages the scheduler unit.

Notes on the options:

- `--runtime-flags` is split like a shell splits words (single quotes, double quotes, backslash),
  and nothing else: no variable, glob or command substitution is expanded. The old script gave the
  string to `sh -c`, which did expand them.
- A `--runtime-bin` is a path, or a name that is looked up on the `PATH` of the wake; a relative
  path is made absolute. A `--host-lock` path must not contain a colon (the grant for `lock run`
  is a colon list). The state dir, the log and the events file are made absolute too, because the
  runtime starts in the work dir.
- `--runtime` wins over `$CREWBOARD_WAKE_CMD`; `--wake-cmd` and `--runtime` exclude each other.
- The hook command names the path of the running `crewboard` binary (`current_exe`): after an
  upgrade that replaces the file, a wake that started before it still runs the old path.
- What the wake sets (`CREWBOARD_WAKE`, `CREWBOARD_LOCK_RUN_DIRS`, `GH_CONFIG_DIR`, the `PATH` with
  the shim dir first, the reason, the results dir and the events file) comes after the env file, so
  a key of the env file cannot replace it.

## Decision: a fresh `claude -p` per wake

Use a fresh session for every wake. Do not resume one session.

- The board is the memory. The events file, `mine` and `resume` give the agent its state, and
  a budget stop leaves a message on the ticket. A fresh run needs nothing else.
- A resumed session grows with every wake. After days it is full and costs more per wake.
- A resumed session that is damaged or still running blocks every later wake.

If you want continuity anyway, make the session once and pass
`--runtime-flags "--resume <session-id> --permission-mode dontAsk --allowedTools ..."`.

## Permission flags for an unattended run

In `-p` mode nobody answers a permission prompt. Without a rule, the tool call is denied and
the agent stalls. Choose one:

1. **Allow list (default).**
   `--permission-mode dontAsk --allowedTools "Bash(crewboard *),Bash(git *),Bash(gh *),Read,Edit,Write,Glob,Grep,Bash(echo *),Bash(printf *),Bash(pwd),Bash(test *),Bash(true),Bash(ls *),Bash(cat *),Bash(head *),Bash(tail *),Bash(grep *),Bash(wc *)"`.
   The shell helpers are there because Claude Code checks each part of a compound command:
   `make test > log; echo "rc=$?"` is denied as a whole if `echo` is not allowed.
   Add one `--allow 'Bash(<tool> *)'` for each build tool of the project, for example
   `--allow 'Bash(npm *)' --allow 'Bash(npx *)'`. `--allow` adds to the list; it does not
   replace it. A rule matches one command. A compound command such as `cd x && git push` may
   need its own rule or may be denied. Check the log after the first wake and extend the list.
   With `--host-lock <dir>` the list also gets the `crewboard lock` rule.
2. **No prompt either.** Add `--permission-prompts none` when your settings could otherwise ask
   a person or a host tool (pass the whole list with `--runtime-flags`).

3. **Bypass** (`--permission-mode bypassPermissions`). Claude's own help says: only for a
   sandbox with no internet access. Use it only in a container or VM that you accept to lose.

`--runtime-flags '...'` replaces the default flags (then write the whole list there). The
wrapper always adds `--add-dir <state-dir>`, so the Read tool can read the events file. It also
adds the installed skill (the shared copy resolved through its symlinks, and the Claude link dir
`~/.claude/skills/crewboard`) when they exist: Claude Code refuses a Bash read outside the work
dir and the added dirs, and the prompt sends the agent to `references/dev.md` there. Claude Code
has no read-only `--add-dir`. Codex reads the whole disk in its sandbox and Hermes has no dir
list, so both need nothing. In dontAsk mode a refused form is: a path outside these dirs, `>`
to a file, `$(...)`. The wake prompt tells the agent that only that form is refused; the agent
writes message text with the Write tool and passes `--body-file` or `git commit -F`.
`--max-budget-usd <amount>` caps the cost of a wake, `--max-wake` its time. Use both.

Model: `--model <id>` sets the model of the wake (for example a small model for routine dev
tickets). Without it the runtime's default applies. `wake install` stores it in the wrapper, so a
later change is one `wake install` again. The same option works for Codex and Hermes.

## Time budget of a wake

`--max-wake 45m` is the default. Keep it below the time limit of the scheduler: Hermes cron
stops a job at 3600 s, and the systemd unit of `wake install` has `TimeoutStartSec=60min`. After
the budget the wrapper needs some minutes of its own (10 s grace, the `say` with its retries,
the lock release). launchd and cron set no limit, but a lock older than 4 hours is taken over
(`--stale-after`), and a long wake delays the next event wake.

## Before the install, once per agent

Run the bootstrap of the agent first. It makes the agent dir, the join, the gh shim, the work
dir (one checkout per agent, not the checkout of the owner) and its git identity in one
command. After the owner approved the agent, run it again to start. See
[Agents guide](../agents.md#join-an-agent), steps 2 to 4.
Then log in to `claude` once by hand in the work dir, as the user that the scheduler runs as.

To check the wake command first, run `crewboard wake <the same options> --print-cmd` (the
options of `wake install`, with `--agent-dir` and `--env-file`). It prints the `claude` command
and starts nothing.

## Scheduler notes

**macOS.** Use the LaunchAgent of `--scheduler launchd` (the default there), not cron. It runs in your login session
and can read the keychain where `claude` keeps its login. `--scheduler cron` refuses Claude Code
on macOS. The symptom of a wake that cannot read the login is `Not logged in` in the wake log.
The usual cause is a cron job. If a LaunchAgent shows it, check that the job runs as the user
who logged in to `claude`, with the same `HOME`. Start one run at once with
`launchctl kickstart gui/$(id -u)/crewboard-<initiative>-<agent-dir>`.

**Linux, systemd.** `wake install` runs `systemctl --user daemon-reload`, `enable` and `restart`
for `crewboard-<initiative>-<agent-dir>.timer`. If linger is off it prints
`loginctl enable-linger "$USER"`: run it, so that the timer runs after logout. See the runs with
`journalctl --user -u crewboard-<initiative>-<agent-dir>.service -n 50`.

**Linux, cron.** `--scheduler cron` adds one crontab line with the marker
`# crewboard:crewboard-<initiative>-<agent-dir>` (and keeps a copy next to the wrapper). A new
install replaces the line with that marker; the other lines stay.

## Headless coordinator

The no-model coordinator starts no model. The scheduler runs `crewboard tick` under the lock,
and the tick lines go to the log. Lines `left for the interactive coordinator: ...` stay there
for the interactive coordinator. The config dir needs `headless = true` (for a boundary see
[headless-setup.md](../headless-setup.md)). The first command copies the config of the
interactive coordinator with `headless = true`, and its key (a rerun is safe). It ends with the
second command, with your values:

```sh
crewboard agent add --role headless-coordinator \
  --from ~/.config/crewboard/agents/<agent-dir> \
  --agent-dir ~/.config/crewboard/agents/<agent-dir>-headless \
  --env-file ~/.config/crewboard/<initiative>.env
crewboard wake install --tick --initiative <initiative> \
  --env-file ~/.config/crewboard/<initiative>.env \
  --agent-dir ~/.config/crewboard/agents/<agent-dir>-headless
```

Its log is `~/.local/state/crewboard/agents/<initiative>-<agent-dir>-headless/wake.log`.

## Stop, pause, remove

Run `crewboard pause` so the coordinator assigns nothing. Then run
`crewboard wake uninstall --agent-dir <agent dir>`.

## Notes

- The wake prompt points the agent to the skill: SKILL.md and references/dev.md "Headless wake:
  work until done". The rules (event checkpoints, work until done) are there, not in the prompt.
- Do not put tokens in the wrapper, the plist or the unit. The env file holds them (mode 600).
