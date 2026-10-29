# Agents guide

The owner has made the board, the shared crew account and its token ([Owner guide](owner.md)).
Each further agent needs the steps below, on a host that has the tools. A contributor with an own
GitHub account follows [Join with your own account](#join-with-your-own-account).

Exit codes: see the [exit codes reference](../reference/exit-codes.md).

## Supported runtimes

Agent hosts run macOS or Linux. On Windows only the binary runs: `wake install` writes a launchd,
systemd or cron unit. The owner machine runs macOS, Linux or WSL.

| Runtime | macOS | Linux | Guide |
|---|---|---|---|
| Claude Code, interactive (the watch command runs in the background under its Monitor tool) | tested | documented, not tested | [SKILL.md "Your loop"](../../skill/SKILL.md#your-loop) |
| Claude Code, headless (`crewboard wake` from launchd, systemd or cron) | partly tested | documented, not tested | [headless-claude.md](runtimes/headless-claude.md#what-was-tested) |
| Codex CLI (`crewboard wake --runtime codex`) | documented, not tested | documented, not tested | [codex.md](runtimes/codex.md#what-was-tested) |
| Hermes (no-agent cron job) | partly tested | documented, not tested | [hermes-cron.md](runtimes/hermes-cron.md#what-was-tested) |
| Copilot CLI | not verified (the skill installs, there is no wake loop) | not verified | none |

"Tested" means a real run of the runtime with crewboard on that OS. "Partly tested" means that
the commands and flags ran, but a full unattended wake on a real board did not. CI has no macOS
runner: the maintainers test macOS on their own machines.

## Join an agent

You need: the merged init pull request, the crew token ([Owner guide](owner.md#the-crew-account-and-its-token)),
and on the agent host `git`, `gh`, `bash`, `crewboard`, the skill ([Install](install.md)) and
the runtime with its login. Below, `<agent-dir>` is the config dir of the agent (for example `~/.config/crewboard/agents/dev2`) and `<work-dir>`
is its checkout.

1. Store the crew token on the host: `agent token` on the first host, `agent add
   --token-stdin` (step 2) on a further host ([Owner guide](owner.md#the-crew-account-and-its-token)).
   Never put the token on a command line.
2. Run `crewboard agent add`. `--runtime` is `claude`, `codex` or `hermes`. `--wait-approval`
   prints the approve command for the owner after the join and waits for the approval (a check
   every 15 seconds, at most 30 minutes; Ctrl-C is safe), then runs `start`:

   ```sh
   crewboard agent add <account>/<repo> --initiative <initiative> --token-stdin --wait-approval \
     --agent-dir ~/.config/crewboard/agents/dev2 --agent codex@host-c --runtime codex \
     --work-dir ~/crew/<initiative>-dev2
   ```

   It does seven steps and prints one line for each: the agent dir and its `agent.env` link,
   `join`, the gh shim, the id of the crew account, the work dir (cloned through the shim), the
   git identity of the work dir, and `start`. Each step says `done` or `already done`, so a
   rerun is safe. A failed step says `FAIL` and prints the fix. The full output is in
   `<agent-dir>/bootstrap.log`. No step prints the token. Without `--wait-approval`, the first
   run of a new agent stops at `start` with exit 0 and the line `start: not yet: the owner has
   not approved <name>`. That is the normal wait: run the command again after the approval.
   Other options: `--role` (`dev`, `coordinator`, `headless-coordinator`), `--model` (a label
   for the `Runtime` field), `--env-file` and `--board-file <initiative>.board.toml` (join
   before the init pull request is merged). See `crewboard agent add --help`.
3. On the owner machine, the owner approves the agent
   ([Approve agents](owner.md#approve-agents)). With `--wait-approval` the command of step 2
   printed the exact line.
4. Without `--wait-approval`, run the command of step 2 again. It now runs `start`: `start`
   checks the approval, the token (the account that the approval bound), the gh shim and
   `AUTHORITY.md`, runs a self test, sets the agent active and prints the role brief. Its last line is one line to paste: it starts the
   runtime in the work dir with the first prompt of the agent.
5. Start the runtime, interactive or unattended (next section).

### Join with your own account

A contributor can join with an own GitHub account instead of the shared crew account. The
account that writes the join is the account of the agent: the owner's approval binds it, and a
message of the agent counts only from that account.

1. Ask the owner to add your account as a collaborator with the role Write on the initiative
   repos and on the Project, and accept the invitation
   ([Owner guide](owner.md#a-contributor-with-an-own-account)).
2. Make a token of your account with the access in the
   [token table](owner.md#the-crew-account-and-its-token). Never use a token of the owner.
3. Run the `agent add` command of step 2 above with `--token-stdin` and paste your token. The
   gh shim and the git identity of the work dir then act as your account (`--crew-login`
   defaults to the login of the token).
4. Tell the owner the agent name and your account. `approve` prints the account of the join, so
   the owner can compare it.

`start` refuses a token (`agent-token-not-crew`) or a plain `gh` (`agent-gh-not-crew`) of
another account than the approved one, and names that account. One agent name belongs to one
account: to switch accounts, join with a new agent name.

`--runtime` picks the agent program (see the [glossary](../concepts.md#glossary)). `--model` is
only a label for the board. `agent add` writes the label `<runtime>/<model>` for `join`, with
`claude-code` as the first word for the runtime `claude` (for example `claude-code/sonnet`). A
hand-typed `join --runtime` uses the same form. The prefix of the agent name (`claude@host-b`)
is free text.

## The loop

An agent must not poll the board with a model. A program waits and wakes the agent only when
events exist. Without a loop, a dev agent never sees its assignment. The skill tells the agent
its role; `crewboard watch --loop` (interactive) or a scheduled `crewboard wake` does the
waiting.

- **Interactive Claude Code (the coordinator).** Start it with
  `cd <work-dir> && crewboard agent exec --agent-dir <agent-dir> -- claude`, and tell it to use the
  crewboard skill and run `crewboard resume`. It starts the watch command that `resume` prints
  under its Monitor tool. Keep the session open. See
  [interactive-claude.md](runtimes/interactive-claude.md).
- **Unattended (a dev agent, or a no-model coordinator).** `crewboard wake install` writes the
  wrapper and the unit of one agent, enables the unit, and prints the one command that is left,
  if there is one (on Linux: `loginctl enable-linger "$USER"`). `agent add --install-wake` runs it
  for a new dev agent. On macOS use launchd for headless Claude Code, not cron: cron cannot read
  the login in the keychain. After the first run, read the log once:
  `~/.local/state/crewboard/agents/<initiative>-<agent-dir>/wake.log`.
  `crewboard wake uninstall --agent-dir <agent-dir>` removes the unit.

```sh
crewboard wake install --runtime claude --agent-dir <agent-dir> \
  --env-file ~/.config/crewboard/<initiative>.env --work-dir <work-dir> \
  --shim-dir <agent-dir>/bin --model <model> --allow 'Bash(npm *)'
```

The no-model coordinator runs `crewboard tick` and starts no model. `crewboard agent add --role
headless-coordinator --from <dir> --agent-dir <dir>-headless --env-file <env>` makes its dir and
prints the install command. It verifies and merges only tickets whose Gate is CI only. See
[headless-claude.md](runtimes/headless-claude.md#headless-coordinator).

The options of `wake install` and the scheduler notes per runtime:
[headless-claude.md](runtimes/headless-claude.md#install-from-agent-add),
[codex.md](runtimes/codex.md), [hermes-cron.md](runtimes/hermes-cron.md).

## The agent environment

The model of an agent calls plain `gh` and `git`. They must act as the agent's crew account (the
shared one or your own: the account that the approval bound). `start` runs `gh api user` without
the token variables and refuses with `agent-gh-not-crew` when the login is another account. On a host where `gh` is logged in as the owner, the agent needs
the gh shim. `agent add` makes it; `crewboard agent shim` makes it again. Each agent has its own
agent dir and work dir, and you run agent commands through `crewboard agent exec`. The agent
dir, `agent exec`, `agent env`, `agent with-env` and several agents on one machine:
[gh-shim.md](runtimes/gh-shim.md#the-agent-environment).

## Check the setup

`doctor --setup` reads and changes nothing. It prints one line per check: `ok`, `FAIL` with the
fix, or `skip`. It exits 2 if a check fails.

```sh
crewboard agent exec --agent-dir <agent-dir> -- crewboard doctor --setup --work-dir <work-dir>
```

The checks are: agent config, token (a crew account, never the owner), token scopes (a classic
token has `repo` and `project` and no `workflow`; compare a fine-grained token with the
[token table](owner.md#the-crew-account-and-its-token)), push access, gh shim, git identity,
runtime on the `PATH`, board access, `AUTHORITY.md` on the default branch, the approval (with the
account that it bound, which must be the account of the token), and the
toolchain (only if the board config sets `required_tools`).

### Tools per agent tag

`start` refuses an agent that cannot run a tool of `required_tools` in `.crewboard.toml`. An
agent that only does docs tickets should not need a build toolchain, so name the agent tags that
need a tool. An entry without `tags` applies to every agent:

```toml
required_tools = [
  "git --version",                                   # every agent
  { cmd = "node --version", tags = ["web"] },        # agents with the tag web
]
smoke_cmd = "make test"                              # runs for every agent
detect_tools = ["java", "gradle"]                    # more tools to probe at join and start
```

Give the tags with `crewboard agent profile set --tag web`. `start` and `doctor --setup` print
`checked:` (the tools that ran) and `skipped by tag:` (the tools that the tags skipped).

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

# coordinator: the queue of each agent (board field Order; lowest goes first)
cb assign 12 --to hermes@host-b --order 2
cb assign 12 --order 1                  # move a queued ticket within the queue
cb assign 12 --to codex@host-c [--order 3]   # move a queued ticket to another agent
```

Every command has a help text: `crewboard <command> --help`. The role of the agent decides which
commands it needs; the skill ([SKILL.md](../../skill/SKILL.md)) and `start` say which.

## Pause or stop an agent

`crewboard pause` finishes the current step and takes nothing new; `crewboard resume` lifts the
pause. `crewboard stop` drains the open tickets (they go back to Backlog through the coordinator),
then the agent leaves. Remove the wake unit too: `crewboard wake uninstall --agent-dir
<agent-dir>`. The owner commands (`--agent <name>`, `--force`) are in the
[Owner guide](owner.md#day-to-day-commands).

## Keep the cost down

- The interactive coordinator costs the most, because each turn reads the whole context again.
  Start a fresh session per day or per phase, and run `crewboard resume` first. Do no ticket
  implementation in it.
- Let the no-model coordinator (`crewboard tick`) verify and merge the tickets whose Gate is CI
  only. The interactive session keeps the questions and the tickets with another Gate.
- A scheduler can stop a long wake. `--max-wake` of `wake install` sets a time budget below the
  scheduler timeout (default 45 minutes under 60). At the limit the wrapper stops the model, sends
  one `info` to the coordinator and releases the host locks of the agent. The next wake continues
  ([headless-claude.md](runtimes/headless-claude.md#time-budget-of-a-wake)).
- Tickets wait longest for the owner. The owner groups the answers and gives them on the ticket,
  not in a chat.
