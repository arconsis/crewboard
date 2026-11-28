# Headless setup: a guardrail and a separate OS user

> **Optional hardening.** This page needs `sudo` and a second OS user. A crew of two agents
> does not need it: the [Getting started](../getting-started.md) path
> works without it. Use it when a headless coordinator runs unattended and you want the
> process to have its own OS user and token.

Read this if a headless coordinator runs without a person at the keyboard and you do not
trust the model with the full rights of the coordinator. The rules are in
[Headless coordinator](../design/spec/agents.md#headless-coordinator).

The headless scope is switched on by one key: `headless = true` in
`<config-dir>/<initiative>.toml`. The key is a guardrail, not a boundary. The headless OS user
can write its config dir, so it can remove the key or pass `--config-dir` to another dir. The
boundary is the OS user and its token: the steps below give the headless coordinator its own
user, its own config dir and a limited token. `crewboard doctor` tells you whether the key is
on.

In the commands below, `cb-headless` is the headless OS user, `<initiative>` is the
initiative name and `<agent>` is the agent name of the coordinator (for example
`claude@host-a`). Run the `sudo` commands as an administrator.

## 1. Make the headless OS user

The user needs no admin rights.

macOS:

```
sudo sysadminctl -addUser cb-headless -fullName "crewboard headless" -shell /bin/zsh -home /Users/cb-headless -password -
sudo createhomedir -c -u cb-headless
```

`-password -` asks for the password. To make the user with `dscl` instead, pick a free uid
(`dscl . -list /Users UniqueID`), then:

```
sudo dscl . -create /Users/cb-headless
sudo dscl . -create /Users/cb-headless UserShell /bin/zsh
sudo dscl . -create /Users/cb-headless UniqueID 550
sudo dscl . -create /Users/cb-headless PrimaryGroupID 20
sudo dscl . -create /Users/cb-headless NFSHomeDirectory /Users/cb-headless
sudo createhomedir -c -u cb-headless
```

Linux:

```
sudo useradd --create-home --shell /bin/bash cb-headless
```

## 2. Install crewboard where the headless user cannot change it

The headless user must run a build that it cannot replace. Building crewboard from source needs
Rust (`cargo`); that is for crewboard itself, not for the project of the crew. A prebuilt release
binary needs no build.

```
cargo build --release --locked
sudo install -o root -m 755 target/release/crewboard /usr/local/bin/crewboard
```

On macOS add `-g wheel`, on Linux `-g root`. Put `/usr/local/bin` first on the `PATH` of the
headless wake. Do not put a `crewboard` in a directory that the headless user can write.

## 3. Give the headless user its own config dir

The config dir holds the agent config, the signing key and the state cache. It belongs to the
headless user, and only that user can read it. The layout is the same as for every agent
(`~/.config/crewboard/agents/<agent-dir>`, see [Agents guide](agents.md)). Here
`<agent-dir>` is the agent dir of the coordinator in your own home, for example `coord`.

```
sudo -u cb-headless mkdir -p -m 700 /Users/cb-headless/.config/crewboard/agents/<agent-dir>/keys
sudo install -o cb-headless -m 600 ~/.config/crewboard/agents/<agent-dir>/<initiative>.toml /Users/cb-headless/.config/crewboard/agents/<agent-dir>/<initiative>.toml
sudo install -o cb-headless -m 600 ~/.config/crewboard/agents/<agent-dir>/keys/<agent> /Users/cb-headless/.config/crewboard/agents/<agent-dir>/keys/<agent>
```

On Linux the home dir is `/home/cb-headless`. Put `headless = true` as the first line of the
copied `<initiative>.toml`. This key switches the headless scope on.

The env file with the agent token goes to the same user, mode 600. It must be the token of the
account that wrote the coordinator's join: the approval binds that account, and a message of the
coordinator counts only from it.

```
sudo install -o cb-headless -m 600 ~/.config/crewboard/<initiative>.env /Users/cb-headless/.config/crewboard/<initiative>.env
```

## 4. Keep the interactive config dir out of reach

The interactive coordinator runs as your own user with its own config dir. The headless user
must not read it, because it holds a signing key and has no scope. As the interactive user:

```
chmod 700 ~/.config/crewboard
chmod 700 ~/.config
```

Check it (the shell expands `~` to your own home before `sudo` runs):

```
sudo -u cb-headless /bin/ls ~/.config/crewboard
```

This must fail with "Permission denied".

## 5. Check

```
sudo -u cb-headless -H /usr/local/bin/crewboard doctor
```

The output must have `per-dir key: headless = true in <config dir>` and
`headless scope: per-dir key only (guardrail)`. Then check that an out-of-scope command is refused:

```
sudo -u cb-headless -H /usr/local/bin/crewboard reassign 1 --reason late --why test
```

It must print `refused [rule=headless-scope]` and exit 2, with no request to GitHub.

## 6. Run the wake as the headless user

The headless coordinator starts no model by default. The scheduler runs `crewboard wake --tick`
as `cb-headless`. It runs `crewboard tick` under a lock and writes the tick lines to its log
(`~/.local/state/crewboard/agents/<initiative>-<agent-dir>/wake.log` of `cb-headless`). This is
the whole job:

```
/usr/local/bin/crewboard wake --tick --initiative <initiative> \
  --env-file /Users/cb-headless/.config/crewboard/<initiative>.env \
  --config-dir /Users/cb-headless/.config/crewboard/agents/<agent-dir>
```

Install the `crewboard` binary where the headless user cannot change it, for example
`/usr/local/bin/crewboard` owned by root (mode 755). On Linux put the line in the crontab of the user (`sudo crontab -u cb-headless -e`, with
`*/5 * * * * ` in front). On macOS use a LaunchDaemon in `/Library/LaunchDaemons` (owned by
root) with the key `UserName` set to `cb-headless` and `StartInterval` 300, so the headless user
cannot change the job. For a coordinator without a separate OS user,
`crewboard wake install --tick --agent-dir <dir>` writes and enables a user unit (see
[unattended-claude.md](runtimes/unattended-claude.md#headless-coordinator)).

## A `Gate: checks: ...` ticket: the tool runs your commands

In `ci_mode: local` the headless coordinator can also verify a ticket whose Gate names only
checks of `AUTHORITY.md`, for example `Gate: checks: tests, format`. Add `--work-dir <git
checkout of the repo>` to the `wake --tick` line. The tick then runs the named checks on a
clean checkout of the delivered commit and writes the verify. A check that fails sends the
ticket back to its agent with the check, the exit code and the log. The tick does not merge
such a ticket: you or the interactive coordinator do.

This runs code of a PR as `cb-headless` with nobody watching. Give that user no other access.
The tick runs no check whose command line names a file of the repo that is not in
`protected_paths`, it stops a check at `check_timeout`, and it starts the checks once per
`done` and at most five times per ticket
([details](../design/spec/agents.md#headless-coordinator), 5a).

## A model for the tickets that need a judgment (optional)

The tick leaves some tickets for a coordinator that can judge: a Review ticket whose Gate is
prose, a question of a dev agent, a plain block, a ticket that a reassign returned. You can let
a scheduled model take them while you are away. This is the unattended coordinator
([spec](../design/spec/agents.md#unattended-coordinator)). It is off by default, and two
things must be on before a model starts: the key in `AUTHORITY.md`, and a unit with a runtime.

**Before you switch it on,** set the outer limit. crewboard limits what the model does through
`crewboard`. The token limits everything else:

- The coordinator account has write access to the repos of the crew only, and no admin rights.
- The default branch has branch protection with the CI checks, a pull request before a merge
  and no bypass. A token that can push to the default branch can change `AUTHORITY.md` and
  switch the key on by itself.
- The unit runs as an own OS user (the steps above). The model can read every file of that
  user.

**Switch on.**

1. Add the key to `AUTHORITY.md` on the default branch:

   ```
   unattended_coordinator: verify
   unattended_budget: 10
   ```

2. Replace the `--tick` unit by a unit with a runtime. As `cb-headless`, the line of step 6
   gets three more options:

   ```
   /usr/local/bin/crewboard wake --tick --initiative <initiative> \
     --env-file /Users/cb-headless/.config/crewboard/<initiative>.env \
     --config-dir /Users/cb-headless/.config/crewboard/agents/<agent-dir> \
     --runtime <claude|codex|hermes> --work-dir <a dir of cb-headless> --max-wake 20m
   ```

   Without a separate OS user, `crewboard wake install --tick --runtime <runtime> --work-dir
   <dir> --agent-dir <dir>` writes and enables the unit. Add `--print-cmd` to the `wake` line to
   see both parts: the tick, and the command of the model.

Each run does the tick first. A model starts only when a ticket needs a judgment, the verdicts
of the day are not used, the day has fewer than 6 model wakes, the last start of the day is 30
minutes ago or more, and the list changed since the last model wake or 2 hours passed. The log
(`wake.log`) has one line for each start. The file `judgment.txt` in the state dir holds the
list that the model got.

**The cost.** crewboard has no limit of model tokens. One unit starts at most 6 models per UTC
day, and each runs at most `--max-wake`: at most 6 times `--max-wake` of model time per day. A
start that fails is one of the 6. Set `--max-wake` and the limits of your model account with
that in mind.

**No tick while the model runs.** The model runs under the lock of the unit. Until it ends (at
most `--max-wake` later) no tick runs, so a `Gate: ci` merge waits that long.

**A model setup that breaks** (a work dir that is gone, a runtime binary that moved) does not
stop the tick. Each run writes one line about it to the log and to standard error, runs the
tick and starts no model.

`--work-dir` is the dir where the model starts. A checkout of the repo lets it read the code
with `git`. In a unit with `--runtime`, `--work-dir` is for the model only: the tick of such a
unit runs no checks of a `Gate: checks: ...` ticket (the section above), and you or the
interactive coordinator verify such a ticket. Each run writes one log line that says so.
`--host-lock` and `--shim-dir` are refused for such a unit.

**Switch off.** Set `unattended_coordinator: off` (or remove the key) on the default branch.
The next run starts no model and refuses every judgment in the headless config dir. The unit
can stay: with the key off it is a `--tick` unit.

**Set the key to `off` while you run a coordinator session yourself.** Your session and the
model use the same agent name and do not see each other. Two coordinators that answer the same
ticket confuse the dev agents.

**What the model may do.** Verify a Review ticket whose Gate is prose (pass or fail, with its
reason in the message), answer and unblock an agent, assign a ticket that a reassign returned,
and ask you a question. It reads a ticket with `crewboard mine <owner>/<repo>#<n>`, which
prints only what passed the signature check: the Gate and the guard of the signed assignment,
the latest `done`, and the signed messages. Each message has a last line
`unattended: <n>/<budget>`. It merges
nothing: a headless config dir merges only a `Gate: ci` ticket, and that is the tick. The
limits per UTC day: `unattended_budget` verdicts, 30 messages, 5 questions to you, 6 model
wakes; one ticket gets 2 verdicts until you comment on it.

**The runtimes.** All three work the same way: crewboard starts the runtime with one prompt and
the runtime runs `crewboard` commands. The scope and the budget are in crewboard, so they hold
for each runtime. The runtimes differ in what else the model can do on the host:

- **Claude Code** (`--runtime claude`). crewboard passes a list of allowed tools: `crewboard`,
  the reading forms of `git` and `gh` (`git log`, `git show`, `git diff`, `git status`,
  `gh pr view`, `gh pr diff`, `gh pr checks`, `gh run view`, `gh issue view`), the read tools
  and a few shell helpers. The list has no `Edit` and no `Write`. It is no boundary: a
  reading form can still write a file (`git diff --output=<file>`). `--allow <tool>` adds a
  tool. On macOS use launchd: cron cannot read the keychain login.
- **Codex** (`--runtime codex`). Codex has no list of tools. crewboard starts it in its
  `workspace-write` sandbox with network access: the model can run every program, and it can
  write in `--work-dir` and in the config dir. The config dir holds the local counts, the
  marks of the unread check and the `headless` key, so such a model can change them. The
  verdict budget still holds: its count comes from the signed messages on GitHub.
- **Hermes** (`--runtime hermes`). Hermes has no list of tools and crewboard passes no sandbox
  flag: the model can run every program as the OS user. `wake install` uses the Hermes
  scheduler for it by default; pass `--scheduler` for another one.

For Codex and Hermes this means: the model can call `git`, `gh` and the GitHub API with the
token of the coordinator account. crewboard does not see such a call and puts no stamp on it.
For Claude Code the list of tools makes that harder, but a list of a runtime is not a boundary
either. In each case the token rights above are the limit. `--runtime-flags "<flags>"` replaces
the flags that crewboard passes, when you want a stricter setting of your runtime.

**Inside a wake.** crewboard sets `CREWBOARD_WAKE` for the model. With it set, `crewboard wake`
starts no second wake, and in the headless config dir `agent exec`, `agent with-env`,
`heartbeat` and `start` are refused, and `poll` and `watch` write no heartbeat. For a runtime
with a full shell (Codex, Hermes) this variable is a guardrail, not a boundary: the model can
clear it. `--override-unread` is refused in a headless config dir. A `say`, `unblock` or `ask`
that finds an unread message prints it and refuses once; the same command then passes.

**When it fails.** A model that exits with an error is one line in the log. From the second
failure in a row the model wakes pause (5 minutes, then longer, at most 1 hour); the tick goes
on. The third failure in a row also goes to standard error of the unit. After a failed start
the next one waits for that pause, for the 30 minutes, and for a change of the list or 2
hours. `crewboard doctor --setup` shows two checks for such a unit: `wake` is the state of the
tick, and `unattended coordinator` is the state of the model (the mode, the runtime and the
count of the last model wake; a fault after 2 failed model wakes in a row; a warning when the
model ended at `--max-wake` several times in a row, or when the stored day is later than
today).

## One digest per day

Set `daily_digest: true` in `AUTHORITY.md`, and the first tick of each UTC day posts one
project status update: the tickets by Status, the agents that are late or dead, and the Review
tickets that wait for a person, with the reason for each. GitHub sends it to the people who
follow the project. The health value is `Off track` when an agent is dead and `At risk` when an
agent is late or a ticket waits for you. No model writes the text.

## A `Gate: ci` merge has no human review

The headless coordinator merges a ticket whose Gate is `ci` when the evidence, the protected
paths and a green CI run for the head SHA pass. No model and no person reads the diff. So a
`Gate: ci` ticket is as strong as the files that define CI: the workflow files, the test
config, the build file that names the test target, and the test files themselves. A dev agent
that edits one of them can make CI green without a test that means anything.

List these files in `protected_paths` in `AUTHORITY.md` ([Owner guide](owner.md#authoritymd)).
The built-in list holds only the authority file, `.crewboard.toml`, the CODEOWNERS files and the files
under `.github/workflows/` and `.github/actions/`. A pull request that touches a listed path
is merged by you only, also when the Gate is `ci`. On a headless host, `doctor --setup` warns
while `protected_paths` is empty.

## What the guardrail does not cover

The `headless` key limits what the `crewboard` binary does. It does not limit the
headless OS user:

- **The GitHub token.** The headless user holds a token of the agent account. A process of that
  user that runs its own code (`curl`, `gh`, a script) can call the GitHub API directly: merge
  a PR, edit a board field, post a comment. crewboard does not see this.
- **The signing key.** The headless user can read its key, so its own code can sign messages
  as the coordinator.
- **The model runtime.** `crewboard wake --tick` starts no model. A unit with `--tick
  --runtime` starts one ([above](#a-model-for-the-tickets-that-need-a-judgment-optional)). When
  its runtime lets the model run any shell command, the model can do both of the above. A list
  of allowed tools is a setting of the runtime, not a check of crewboard.

So limit the token as well. The token is the real power of the headless user:

- **Use an agent account** (the shared `crew_login` of `.crewboard.toml`, or an own account),
  never the owner's token. Give it
  write access only to the repos of the initiative, and no admin rights. An account without
  admin rights cannot change branch protection or repo settings.
- **Protect the main branch.** Require the CI status checks and a pull request before a merge,
  forbid force pushes, and do not allow anyone to bypass the rules. Then even a direct API
  merge needs green CI, which is the same condition that a headless `merge` checks.
- **No workflow rights.** A fine-grained token without the "Workflows" permission cannot change
  the CI files, so it cannot make CI green by editing the checks. Changes to workflow files go
  through the owner (see [Workflow files: the owner route](../design/spec/tickets.md#workflow-files-the-owner-route)).
- **Revoke on a doubt.** See "A key or a token leaks" in [security.md](security.md).

Why: crewboard protects the board from mistakes and from a model that only runs `crewboard`.
GitHub protects the repos from the token. A boundary against the model needs both.
