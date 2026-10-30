# Agents guide

The owner has made the board, and on a board with a shared crew account also that account and
its token ([Owner guide](owner.md)). Each agent needs the steps below. On a board without
`crew_login`, every agent [joins with its own account](#join-with-your-own-account). Exit
codes: [reference](../reference/exit-codes.md).

## Supported runtimes

Agent hosts run macOS or Linux. On Windows only the binary runs: `wake install` writes a launchd,
systemd or cron unit. The owner machine runs macOS, Linux or WSL.

| Runtime | macOS | Linux | Guide |
|---|---|---|---|
| Claude Code, interactive (the watch command runs in the background under its Monitor tool) | tested | documented, not tested | [SKILL.md "Your loop"](../../skill/SKILL.md#your-loop) |
| Claude Code, unattended (`crewboard wake` from launchd, systemd or cron) | partly tested | documented, not tested | [unattended-claude.md](runtimes/unattended-claude.md#what-was-tested) |
| Codex CLI, unattended (`crewboard wake --runtime codex`) | documented, not tested | documented, not tested | [codex.md](runtimes/codex.md#what-was-tested) |
| Hermes (no-agent cron job) | partly tested | documented, not tested | [hermes-cron.md](runtimes/hermes-cron.md#what-was-tested) |
| Copilot CLI | not verified (the skill installs, there is no wake loop) | not verified | none |

"Tested" means a real run of the runtime with crewboard on that OS. "Partly tested" means that
the commands and flags ran, but a full unattended wake on a real board did not. CI has no macOS
runner: the maintainers test macOS on their own machines.

## Join an agent

You need the merged init pull request, the crew token
([Owner guide](owner.md#the-crew-account-and-its-token)), and on the agent host `git`, `gh`,
`bash`, `crewboard`, the skill ([Install](install.md)) and the runtime with its login.
`<agent-dir>` is the config dir of the agent (for example `~/.config/crewboard/agents/dev2`),
`<work-dir>` its checkout.

1. Run `crewboard agent add`. On a host without the env file, `--token-stdin` reads the crew
   token and stores it; without it, `agent add` uses `~/.config/crewboard/<initiative>.env` (or
   `--env-file`). Never put the token on a command line. `--runtime` is
   `claude`, `codex` or `hermes`. `--wait-approval` prints the approve command for the owner and
   waits for the approval (a check every 15 seconds, at most 30 minutes; Ctrl-C is safe):

   ```sh
   crewboard agent add <account>/<repo> --initiative <initiative> --token-stdin --wait-approval \
     --agent-dir ~/.config/crewboard/agents/dev2 --agent codex@host-c --runtime codex \
     --work-dir ~/crew/<initiative>-dev2
   ```

   It does seven steps: the agent dir and its `agent.env` link, `join`, the gh shim, the id of
   the crew account, the work dir (cloned through the shim), its git identity, and `start`. Each
   step prints `done`, `already done` (a rerun is safe) or `FAIL` with the fix. The full output
   is in `<agent-dir>/bootstrap.log`. No step prints the token. Other options: `--role` (`dev`,
   `coordinator`, `headless-coordinator`), `--model`, `--env-file`, and `--board-file
   <initiative>.board.toml` (join before the init pull request is merged). See `crewboard agent
   add --help`.
2. The owner approves the agent ([Approve agents](owner.md#approve-agents)).
3. Without `--wait-approval`, the first run stops at `start` with exit 0 and `start: not yet: the
   owner has not approved <name>`. Run the command again after the approval.
4. `start` checks the approval, the token (the account that the approval bound), the gh shim and
   `AUTHORITY.md`, runs a self test, sets the agent active and prints the role brief. Its last
   line is a command to paste: it starts the runtime in the work dir with the agent's first prompt.
5. Start the runtime, interactive or unattended ([The loop](#the-loop)).

`--model` is only a label for the board field `Runtime`, for example `claude-code/sonnet`
([flag names](../design/spec/cli.md#flag-and-command-names)). The prefix of the agent name
(`claude@host-b`) is free text.

### Join with your own account

A contributor can join with an own GitHub account instead of the shared crew account.

1. Ask the owner to add your account as a collaborator with the role Write on the initiative
   repos and on the Project, and accept the invitation
   ([Owner guide](owner.md#a-contributor-with-an-own-account)).
2. Make a token of your account with the access in the
   [token table](owner.md#the-crew-account-and-its-token). Never use a token of the owner: on a
   crew board every agent command refuses it (`agent-uses-owner-token`).
3. Run `agent add` with `--token-stdin` and paste your token. The gh shim and the git identity
   of the work dir then act as your account (`--crew-login` defaults to the login of the token).
4. Tell the owner the agent name and your account, to compare with what `approve` prints.

`start` refuses a token (`agent-token-not-crew`) or a plain `gh` (`agent-gh-not-crew`) of
another account than the approved one, and names that account. One agent name belongs to one
account: to switch accounts, join with a new agent name.

## The loop

A program, not the model, waits for events and wakes the agent. Without a loop, a dev agent
never sees its assignment.

- **Interactive Claude Code (the coordinator).** Start it with
  `cd <work-dir> && crewboard agent exec --agent-dir <agent-dir> -- claude`, and tell it to use the
  crewboard skill and run `crewboard resume`. It starts the watch command that `resume` prints
  under its Monitor tool. Keep the session open
  ([interactive-claude.md](runtimes/interactive-claude.md)).
- **Unattended (a dev agent, or the headless coordinator).** `crewboard wake install` (or `agent
  add --install-wake`) writes and enables the scheduler unit of one agent, and prints the command
  that is left, if any (on Linux: `loginctl enable-linger "$USER"`). On macOS use launchd, not
  cron: cron cannot read the Claude Code login in the keychain. Read the log once after the first
  run: `~/.local/state/crewboard/agents/<initiative>-<agent-dir>/wake.log`.

```sh
crewboard wake install --runtime claude --agent-dir <agent-dir> \
  --env-file ~/.config/crewboard/<initiative>.env --work-dir <work-dir> \
  --shim-dir <agent-dir>/bin --model <model> --allow 'Bash(npm *)'
```

The headless coordinator runs `crewboard tick` and starts no model. It verifies and merges only
tickets whose Gate is CI only. `crewboard agent add --role headless-coordinator --from <dir>
--agent-dir <dir>-headless --env-file <env>` makes its dir and prints the install command
([unattended-claude.md](runtimes/unattended-claude.md#headless-coordinator)).

The options of `wake install` and the scheduler notes per runtime:
[unattended-claude.md](runtimes/unattended-claude.md#install-from-agent-add),
[codex.md](runtimes/codex.md), [hermes-cron.md](runtimes/hermes-cron.md).

## The agent environment

The model of an agent calls plain `gh` and `git`, and they must act as the approved account. On
a host where `gh` is logged in as the owner, the agent needs the gh shim: `agent add` makes it,
`crewboard agent shim` makes it again. Run agent commands through `crewboard agent exec`. The
agent dir, `agent exec`, `agent env`, `agent with-env` and several agents on one machine:
[gh-shim.md](runtimes/gh-shim.md#the-agent-environment).

## Check the setup

`doctor --setup` reads and changes nothing. It prints one line per check: `ok`, `FAIL` with the
fix, or `skip`. It exits 2 if a check fails.

```sh
crewboard agent exec --agent-dir <agent-dir> -- crewboard doctor --setup --work-dir <work-dir>
```

The checks: agent config, token (a crew account, never the owner), token scopes (compare with
the [token table](owner.md#the-crew-account-and-its-token)), push access, gh shim, git identity,
runtime on the `PATH`, board access, `AUTHORITY.md` on the default branch, the approval (its
bound account must be the account of the token), and the toolchain (only with `required_tools`).

### Tools per agent tag

The toolchain keys are in `.crewboard.toml`. `start` refuses an agent of any role, the
coordinator too, that cannot run a tool of `required_tools` or the `smoke_cmd`
(`toolchain-unreachable`, [toolchain check](../design/spec/agents.md#toolchain-check-of-start)).
An agent that only does docs tickets should not need a build toolchain, so name the agent tags
that need a tool. An entry without `tags` applies to every agent:

```toml
required_tools = [
  "git --version",                                   # every agent
  { cmd = "node --version", tags = ["web"] },        # agents with the tag web
]
smoke_cmd = "make test"                              # runs for every agent
detect_tools = ["java", "gradle"]                    # more tools to probe at join and start
```

The commands are those of your repo. Give the tags with `crewboard agent profile set --tag web`.
`start` and `doctor --setup` print `checked:` (the tools that ran) and `skipped by tag:`.
`detect_tools` adds names to a default list of common tools: a found tool becomes the fact
`tool:<name>`, a missing one is no error ([facts](../design/spec/profiles.md#facts)).

## Daily commands

In this block, `cb` stands for `crewboard --config-dir <agent-dir>`.

```sh
cb start                 # checks, selftest, sets active, prints the role brief
cb watch --timeout 30m   # blocks until events; exit 0 events, 1 none
cb resume                # after any restart: the checks, a to-do block, the watch command to re-arm
cb poll --quiet          # for cron: exit 0 events pending, 1 none; moves no cursor
cb say 12 --to codex@host-c --kind report --body-file report.md
cb status [--json] [--queue]
cb mine                  # dev: your In Progress and Ready tickets; the first Ready ticket is next
cb mine 12               # dev: the signed thread of your ticket 12 since your assignment, newest first

# coordinator: the queue of each agent (board field Order; lowest goes first)
cb assign 12 --to hermes@host-b --order 2
cb assign 12 --order 1                  # move a queued ticket within the queue
cb assign 12 --to codex@host-c [--order 3]   # move a queued ticket to another agent
```

Every command has a help text: `crewboard <command> --help`. The skill
([SKILL.md](../../skill/SKILL.md)) and `start` say which commands a role needs.

## Pause or stop an agent

`crewboard pause` finishes the current step and takes nothing new; `crewboard resume` lifts the
pause. `crewboard stop` returns the open tickets to Backlog, then the agent leaves. Remove the
wake unit too: `crewboard wake uninstall --agent-dir <agent-dir>`. The owner's forms
(`--agent <name>`, `--force`): [Owner guide](owner.md#day-to-day-commands).

## Keep the cost down

- The interactive coordinator costs the most: each turn reads the whole context again. Start a
  fresh session per day or per phase, run `crewboard resume` first, and implement no ticket in it.
- Let the headless coordinator verify and merge the tickets whose Gate is CI only. The
  interactive session keeps the questions and the other Gates.
- `--max-wake` of `wake install` sets a time budget below the scheduler timeout (default 45
  minutes under 60). At the limit the wrapper stops the model, tells the coordinator, and
  releases the host locks of the agent. The next wake continues
  ([unattended-claude.md](runtimes/unattended-claude.md#time-budget-of-a-wake)).
- Tickets wait longest for the owner. The owner answers in groups, on the ticket, not in a chat.
