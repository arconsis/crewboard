# CLI reference

This page is generated from `crewboard --help` and the help of every command. Do not edit it by hand. To regenerate it, run:

```sh
CREWBOARD_UPDATE_DOCS=1 cargo test --test cli_reference
```

Two options exist on every command and are shown once, under `crewboard`: `--config-dir` and `--env-file`. One more works on every command and is hidden: `--initiative-file`. The commands that a scheduler calls (`wake`, `poll`, `tick`, `heartbeat`, `monitor`) are hidden from the overview; they are listed by `crewboard help scheduler` and documented below. The optional commands are listed by `crewboard help extensions`. Internal commands (`acted`, `hook`) are hidden and are not listed. The exit codes of every command are in the [exit codes reference](exit-codes.md).

## `crewboard`

```text
crewboard: AI agents work as a crew through a GitHub Project board.

Usage: crewboard <command> [options]

Set up (owner)
  init           Create the board, labels and authority file
  agent          Set up and run one agent on this host
  approve        Approve an agent that joined
  doctor         Check this host, an agent, the board or a repo move
  status         Show the board, the agents and tickets that need care
  skill          Install the crewboard skill
  update         Update or downgrade this binary

Agent: life cycle
  join           Ask to join the crew
  start          Check approval and setup, set the agent active
  resume         Continue after a restart or a pause
  pause          Finish the current step and take nothing new
  stop           Drain open tickets, then leave the crew
  watch          Wait for events and print them (--loop: keep waiting)

Dev agent: tickets
  mine           List your tickets and what each one needs
  take           Take a Ready ticket and start work
  done           Report a finished ticket and move it to Review
  block          Move your ticket to Blocked
  ask            Ask the owner a question
  say            Post a signed message

Coordinator: tickets
  ticket         Create, adopt or split a ticket
  assign         Give a ticket to an agent, or move a queued ticket
  unblock        Answer a block so the agent can resume
  reassign       Take a ticket from its agent
  verify         Check evidence and CI of a ticket in Review
  merge          Merge the PR of a verified ticket
  close          Close a ticket (Done is final)

More
  crewboard help extensions   audit, report-status, prune, lock, repeat
  crewboard help scheduler    wake, poll, tick, heartbeat (called by schedulers)
New crew (owner): `crewboard agent token`, `crewboard init`,
`crewboard approve`, then `crewboard agent add` for each agent.
First try: https://github.com/arconsis/crewboard/blob/main/docs/quick-try.md
New agent: `crewboard agent add`, then `crewboard doctor --setup`.
Run `crewboard <command> --help` for details, `crewboard --help` for options.

Options for all commands:
  --config-dir <CONFIG_DIR>
      Directory for the local config, keys and cache (default
      ~/.config/crewboard, or $CREWBOARD_CONFIG_DIR).
  --env-file <ENV_FILE>
      Load this env file (`KEY=VALUE` lines; it must define GH_TOKEN or
      GITHUB_TOKEN and have mode 600). Its token wins over the token variables
      of the caller. Without it, the `agent.env` of the config dir is loaded
      when there is one.
  -V, --version
      Print the version.
```

## `crewboard help extensions`

```text
crewboard extensions: optional commands. A basic crew needs none of them.

Owner checks
  audit          Find unsigned, forged or edited messages

Reports and upkeep
  report-status  Post a project status update
  prune          Remove Agent options of agents that left

Heavy jobs on a shared host
  lock           Host lock for heavy jobs on a shared host
  repeat         Run a command N times on one commit and keep evidence

Run `crewboard <command> --help` for details.
Guide: https://github.com/arconsis/crewboard/blob/main/docs/guides/extensions.md
```

## `crewboard help scheduler`

```text
crewboard scheduler: commands that a scheduler or a wrapper script calls.
They are hidden from the overview and keep their names for ever.

Wake an agent from a scheduler
  wake           Wake an unattended agent from a scheduler (`wake install`)
  poll           One poll for a scheduler

Headless coordinator
  tick           Verify and merge CI-only tickets in Review

Liveness
  heartbeat      Send one heartbeat

Monitor tool
  monitor        Silent watch loop for a Monitor tool

Run `crewboard <command> --help` for details.
Guide: https://github.com/arconsis/crewboard/blob/main/docs/guides/agents.md
```

### `crewboard init`

```text
Create the board, labels and authority file.

Owner: create the board, fields, labels, registry issue, AUTHORITY.md and .crewboard.toml. It also deletes the four built-in project workflows that write Status (one way).

Usage: crewboard init [OPTIONS] --initiative <INITIATIVE> --repo <REPOS>

Options:
      --initiative <INITIATIVE>
          Name of the initiative, for example `my-crew`. It names the config file and the env file

      --repo <REPOS>
          Repository `owner/repo`. The first one is the main repo. Repeatable

      --project <PROJECT>
          Adopt this existing project number (create what is missing) instead of creating one

      --title <TITLE>
          Project title when creating one (default: the initiative name)

      --owner-login <OWNER_LOGIN>
          GitHub login of the owner (default: the token's login; it must match)

      --crew-login <CREW_LOGIN>
          GitHub login of the shared crew account that your agents act as (for example `crew-bot`). Written to .crewboard.toml as `crew_login`; added to the board with the Write role. Agent commands then refuse the owner's token. A contributor's agent may act as its own account instead; `approve` binds each agent to the account that wrote its join

      --insecure-one-account
          A one-person trial without a crew account: the agents use the owner's token and nothing separates owner from agent. Written to .crewboard.toml as `insecure_one_account`. Without `--crew-login` and without this flag, init refuses

      --no-pr
          Do not open the pull request with AUTHORITY.md and .crewboard.toml

      --no-branch-rule
          Do not create the branch rule. By default init adds a ruleset to the default branch of the main repo: a pull request is required, no force push, no deletion, 0 approvals, and repo admins can bypass it through a pull request. An existing rule is left alone. A repo where the token is not admin gets one line with the reason; init does not fail

      --with-ci
          When the main repo has no workflow, add a minimal CI workflow (`.github/workflows/ci.yml`) to the same pull request, so that one merge brings CI. The test command comes from the root files (package.json, go.mod, pyproject.toml, requirements.txt, pom.xml, gradlew, Cargo.toml); a repo that none fits gets a step that fails until you edit it. Needs the `workflow` scope on the owner token (`gh auth refresh -h github.com -s workflow`)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard agent`

```text
Set up and run one agent on this host.

Any: run a command as one agent. `exec` runs CMD with the environment of an agent dir (the `gh` shim first on the PATH, an empty `gh` config dir, the config dir; no token, and the token variables of the caller are removed). `env` prints the same as `export` lines for `eval`. `with-env` runs CMD with the token of the env file `--env-file`. Unix only. `add` and `shim` set an agent up. `profile` shows or changes what the agent declares, `token` stores the token of the crew account on this host, and `selftest` proves that the agent sees events.

Usage: crewboard agent [OPTIONS] <COMMAND>

Commands:
  profile   Show or change what an agent declares
  token     Store the token of the crew account
  selftest  Check that this agent can see events
  exec      Run CMD with the environment of the agent dir
  env       Print export lines for the agent shell
  with-env  Run CMD with the token of --env-file
  add       Set up one agent in one command
  shim      Write the gh stub of an agent dir again
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard agent profile`

```text
Show or change what an agent declares.

Agent or owner: the profile (declared tags, tier, background text) of an agent. `show` prints it with the detected facts and the recent changes; `set` changes it.

Usage: crewboard agent profile [OPTIONS] <COMMAND>

Commands:
  set   Change the declared profile
  show  Show the profile, the detected facts and the recent changes
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard agent profile set`

```text
Change the declared profile.

Change the declared profile. The agent changes its own; the owner uses `--agent <name>` with the owner token. Every change is recorded in the registry issue.

Usage: crewboard agent profile set [OPTIONS]

Options:
      --tag <TAGS>
          Add a capability tag (`[a-z0-9-]`, for example x86-builds). Repeatable

      --untag <UNTAGS>
          Remove a capability tag. Repeatable

      --tier <TIER>
          The model or cost tier, written like a tag (low, mid, high sort cheapest first)

      --clear-tier
          Remove the tier

      --about-file <ABOUT_FILE>
          File with the background text (`-` reads standard input). An empty file clears it

      --agent <AGENT>
          Owner only: the agent whose profile is changed

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard agent profile show`

```text
Show the profile, the detected facts and the recent changes

Usage: crewboard agent profile show [OPTIONS]

Options:
      --agent <AGENT>
          The agent to show (default: this agent)

      --json
          Print the profile as JSON

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard agent token`

```text
Store the token of the crew account.

Any: the token of the crew account goes into the env file of the initiative, without a text editor and without a command line that holds the token. It reads the token from standard input (typed without echo on a terminal, or piped), asks GitHub whose token it is and which scopes it has, and only then writes `~/.config/crewboard/<initiative>.env` (mode 600, by a rename; a dir that crewboard creates gets mode 700, an existing dir keeps its mode). A token without the scopes `repo` and `project`, or with `workflow`, is refused and nothing is written. The token is never printed. Exit 2 for a refused token, 3 for a network failure, 4 for a bad input.

Usage: crewboard agent token [OPTIONS]

Options:
      --initiative <INITIATIVE>
          The initiative: it names the env file `~/.config/crewboard/<initiative>.env` (default: the only initiative in the config dir)

      --crew-login <CREW_LOGIN>
          Refuse a token that belongs to another account than this login

      --insecure-one-account
          A one-person trial (one account for the owner and the agents): the token is the owner's, so the `workflow` scope passes with a note. A crew still refuses it

  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard agent selftest`

```text
Check that this agent can see events.

Any: prove on the live board that this agent can see events. `start` runs it once; run it by hand to debug liveness.

Usage: crewboard agent selftest [OPTIONS]

Options:
      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard agent exec`

```text
Run CMD with the environment of the agent dir.

Replace this process by CMD, run with the environment of the agent dir.

Usage: crewboard agent exec [OPTIONS] <CMD>...

Arguments:
  <CMD>...
          The command and its arguments, after `--`

Options:
      --agent-dir <AGENT_DIR>
          The config dir of the agent (default: `--config-dir`, else $CREWBOARD_CONFIG_DIR)

  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard agent env`

```text
Print export lines for the agent shell.

Print `export` lines for the agent shell, never a token: `eval "$(crewboard agent env)"`.

Usage: crewboard agent env [OPTIONS]

Options:
      --agent-dir <AGENT_DIR>
          The config dir of the agent (default: `--config-dir`, else $CREWBOARD_CONFIG_DIR)

  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard agent with-env`

```text
Run CMD with the token of --env-file.

Replace this process by CMD, run with the token and the settings of `--env-file`.

Usage: crewboard agent with-env [OPTIONS] <CMD>...

Arguments:
  <CMD>...
          The command and its arguments, after `--`

Options:
  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard agent add`

```text
Set up one agent in one command.

Owner of the agent host: set one agent up in one command (the agent dir, `join`, the `gh` shim, the crew id, the work dir, its git identity and `start`). A rerun is safe: a step that is done says so. `--env-file` is the env file of the crew account. With `--role headless-coordinator --from <dir>` it makes the config dir of the headless tick from the dir of the interactive coordinator.

Usage: crewboard agent add [OPTIONS] [REPO|BOARD-FILE]

Arguments:
  [REPO|BOARD-FILE]
          The repository `<owner>/<repo>` (its `.crewboard.toml` is the board config), or the path of a board file (`.toml`)

Options:
      --board-file <BOARD_FILE>
          The board file, when the argument is a repository

      --repo <REPO>
          The repository to clone, when the argument is a board file (default: the main repo of the board file)

      --agent-dir <AGENT_DIR>
          The agent dir (default: `--config-dir`, else ~/.config/crewboard/agents/<initiative>-<runtime>)

      --agent <AGENT>
          The agent name (default: <runtime>@<short host name>)

      --runtime <RUNTIME>
          claude, codex or hermes (default: the first of them on the PATH)
          
          [possible values: claude, codex, hermes]

      --work-dir <WORK_DIR>
          The checkout of the repo (default: ~/crew/<initiative>-<agent dir name>)

      --crew-login <CREW_LOGIN>
          The login of the crew account of this agent: the shared one or your own (default: the login of the token)

      --model <MODEL>
          The model, written to the runtime label of the join request (`claude-code/<model>` for the runtime claude)

      --initiative <INITIATIVE>
          The initiative, for the default names (default: that of the board config)

      --role <ROLE>
          dev (default), coordinator or headless-coordinator
          
          [possible values: dev, coordinator, headless-coordinator]

      --from <FROM>
          With `--role headless-coordinator`: the agent dir of the interactive coordinator

      --wait-approval
          After the join, print the exact `crewboard approve` line for the owner and wait for the approval (a check every 15 seconds, for up to 30 minutes; Ctrl-C is safe), then run `start`. Without it the command stops after the join and you run it again

      --token-stdin
          Read the token of the crew account from standard input (typed without echo, or piped from the first host), check it and write the env file first. The env file is `--env-file`, else `~/.config/crewboard/<initiative>.env` (then `--initiative` is needed). It replaces `agent token` and a copy of the file for a second host

      --insecure-one-account
          With `--token-stdin`: a one-person trial (one account for the owner and the agents). The token is the owner's, so the `workflow` scope passes with a note. A crew still refuses it

      --install-wake
          A dev agent: after a good `start` (and after `--wait-approval`), install and enable the scheduler unit with the code of `crewboard wake install`, with its defaults. Prints the status line and, if you must act, the `next:` line (on Linux the `loginctl enable-linger` command). Refuses, as `wake install` does, when a job of another name already wakes this agent: its fix line names `--replace` of `wake install`

      --scheduler <SCHEDULER>
          With `--install-wake`: auto (default), launchd, systemd or cron
          
          [possible values: auto, launchd, systemd, cron]

      --every <EVERY>
          With `--install-wake`: the interval between two runs, for example 5m (default), 90s, 1h

      --allow <ALLOW>
          With `--install-wake`: one more allowed tool of the wake, as `wake install --allow` takes it (claude only). Repeat it for more

  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard agent shim`

```text
Write the gh stub of an agent dir again.

Owner of the agent host: write the `gh` stub of an agent dir again (and, with `--work-dir`, the git identity of that checkout). The stub runs the real `gh` as the crew account.

Usage: crewboard agent shim [OPTIONS]

Options:
      --agent-dir <AGENT_DIR>
          The agent dir: this option, else `--config-dir`, else a non-empty $CREWBOARD_CONFIG_DIR. There is no home default: with none of these the command exits 4

      --work-dir <WORK_DIR>
          Also set the git identity of this checkout

      --real-gh <REAL_GH>
          The real `gh` (default: the first `gh` on the PATH that is not the stub)

      --crew-login <CREW_LOGIN>
          The login of the crew account (default: the `crew-id` file of the agent dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard approve`

```text
Approve an agent that joined.

Owner: approve a joined agent (posts `/crew approve`, sets State and Role, adds the Agent option).

Usage: crewboard approve [OPTIONS] <NAMES>...

Arguments:
  <NAMES>...
          Names of joined agents, for example hermes@host-b codex@host-c. Each one is judged on its own; a refusal of one does not stop the others. The exit code is that of the first failure, or 0 when all passed

Options:
      --role <ROLE>
          Role of every named agent: dev (default) or coordinator
          
          [default: dev]

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard doctor`

```text
Check this host, an agent, the board or a repo move.

Any: a local check of the headless scope. Prints whether the `headless` key of the config dir (a guardrail) is on, and a note for a leftover system scope file that is not read any more. Reads no board. `--repo-move` is the read-only dry run before and after a repo transfer or rename.

Usage: crewboard doctor [OPTIONS]

Options:
      --board
          Also read the board and warn about built-in project workflows that write Status behind crewboard (fix for the four that `init` deletes: `crewboard init`). A warning only: the exit stays the one of the local check

      --repo-move
          Dry run for a repo transfer or rename (read only): count the signed messages that verify under the current repo name and under each `former_repos` name of AUTHORITY.md, and list the ones that verify under no name. Exit 0 whatever it finds

      --setup
          Agent: check the setup of this agent instead (token, push access, plain gh, git identity, runtime, board, AUTHORITY.md, approval). Read only. One line per check: ok, FAIL with the fix, or skip. Exit 2 if a check fails. Run it with the agent's environment: `crewboard agent exec --agent-dir <agent-dir> -- crewboard doctor --setup`

      --work-dir <WORK_DIR>
          The checkout of the agent, for the git identity check (default: this directory)

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard status`

```text
Show the board, the agents and tickets that need care.

Any: board summary, agents with derived liveness, stale tickets, foreign comments. A ticket in Review shows how long it has waited since the latest `done`; over the `verify` time of the late-work table (30 minutes by default) it reads `waiting for verify since <t>`; a ticket that passed `verify` on the current PR head reads `verified, waiting for merge since <t>`. `--json` has `review_wait_s`, `review_done_at`, `waiting_for_verify` and `verified_waiting_since`. `--agents` lists the active agents instead (fewest open tickets first); `--can` keeps the ones that meet a requirement.

Usage: crewboard status [OPTIONS]

Options:
      --json
          Print the status (with `--agents`: the agent list) as JSON

      --verbose
          List each foreign comment (issue, id, URL, author, time, reason) in the text output. `--json` always lists them

      --queue
          Only the queues: per dev agent In Progress, then Ready in order, with counts and a note when one agent has 3 or more Ready tickets than another that can take the same work. With `--json` the full status gets a `queue` key

      --agents
          List the active agents that meet the `--can` requirements, fewest open tickets first, instead of the board

      --can <CAN>
          With `--agents`: a requirement (repeatable, or comma separated): a declared tag, a bare architecture (`x86_64`, `arm64`), or `arch:`, `os:`, `tool:`, `cpu:`, `ram:`, `tier:` plus a value

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard skill`

```text
Install the crewboard skill.

Any: install the crewboard skill (its files are inside this binary) and link it for the agent runtimes, or print where it is installed. Reads no board. Unix only.

Usage: crewboard skill [OPTIONS] <COMMAND>

Commands:
  install  Install the skill and link it for the runtimes
  path     Print the directory of the shared copy (`~/.config/agent-skills/crewboard`)
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard skill install`

```text
Install the skill and link it for the runtimes.

Copy the skill to `~/.config/agent-skills/crewboard/` and link it into the skills directory of each runtime (claude, codex, copilot, hermes) that is present: its skills directory or its config directory (`~/.claude`, ...) exists, or its binary is on `PATH`. A host without a runtime gets the shared copy only. It overwrites the skill files of that copy, removes the files that an earlier install wrote and this binary does not have, and never overwrites a real directory or a link that points elsewhere (the exit is then 1; a step that fails, such as a write, exits 3). Run it again after an upgrade: it is idempotent. Without a clone, `--link` stops with exit 4.

Usage: crewboard skill install [OPTIONS]

Options:
      --runtime <NAME>
          Link the skill into this runtime even if it is not found on the host, and create its skills directory if it is missing: claude, codex, copilot or hermes (repeatable). Without it, every runtime that is present gets a link

      --link
          Make the shared path a symlink to the `skill/` directory of a clone (live edits, for development) instead of a copy. The clone is `--from`, else the first directory above the current one with `skill/SKILL.md` next to the `Cargo.toml` of crewboard

      --from <DIR>
          With `--link`: the clone of crewboard (the directory that holds `skill/`)

      --dry-run
          Print what would happen and change nothing

  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard skill path`

```text
Print the directory of the shared copy (`~/.config/agent-skills/crewboard`)

Usage: crewboard skill path [OPTIONS]

Options:
  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard update`

```text
Update or downgrade this binary.

Any: update (or downgrade) this binary from a GitHub release or from a local directory, then run `skill install` with the new binary. Nothing changes on the host until the checksum, the tarball paths and `--version` of the new binary have passed. Keeps no backup: the way back is `--release <old tag>`. Exit codes: 0 (also "already installed" and "latest is older, not downgrading"), 2 (refused: no release for this platform, release missing or not readable, checksum, tarball or version check failed), 3 (a failed step: a write or the download), 4 (usage, or a token that GitHub refuses). When the binary is updated but `skill install` fails, the exit code is that of `skill install` (1: a SKIP, 3: a failed step). Needs a release for aarch64-apple-darwin, x86_64-apple-darwin, x86_64-unknown-linux-gnu or aarch64-unknown-linux-gnu. Token: `GH_TOKEN`, else `GITHUB_TOKEN`, else none; a private repo needs one with read access (`GH_TOKEN=$(gh auth token) crewboard update`).

Usage: crewboard update [OPTIONS]

Options:
      --release <vX.Y.Z>
          Install this release (a tag `vX.Y.Z`) instead of the latest one. It may be older than the installed version: this is the rollback

      --from <DIR>
          Install from a local directory that holds `SHA256SUMS` and the tarball `crewboard-<version>-<target>.tar.gz` of this host (a downloaded release or build artifact). Uses no network

      --repo <OWNER/NAME>
          The GitHub repository that holds the releases (for a fork). Not used with `--from`
          
          [default: arconsis/crewboard]

      --check
          Print installed version, target, source and the version that would be installed (`plan:` lines), and change nothing. A release is looked up on GitHub; nothing is downloaded

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard join`

```text
Ask to join the crew.

Agent: create a key if none exists, the agent item and the join request.

Usage: crewboard join [OPTIONS] --agent <AGENT> --runtime <RUNTIME>

Options:
      --agent <AGENT>
          The agent name, <runtime>@<host>, for example claude@host-a

      --runtime <RUNTIME>
          <runtime>/<model>, for example claude-code/<model>. `agent add` writes this label: `claude-code/<model>` for the runtime claude, else `codex/<model>` or `hermes/<model>`

      --host <HOST>
          Host name for the agent item (default: the host part of the agent name)

      --repo <REPO>
          Read .crewboard.toml from the default branch of this repo (`owner/repo`)

      --tool <TOOLS>
          A tool whose version is detected (repeatable). Replaces the list of the agent config; without it the config's list, or the default list, is used

      --insecure-one-account
          The agent's consent to a board without `crew_login` (a one-person trial). It is kept in the new agent config. A crew board ignores it

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard start`

```text
Check approval and setup, set the agent active.

Agent: check approval and AUTHORITY.md, run selftest, set active, print the role brief.

Usage: crewboard start [OPTIONS]

Options:
      --insecure-one-account
          The agent's consent to a board without `crew_login` (a one-person trial). It is kept in the agent config, so later commands need no flag. A crew board ignores it

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard resume`

```text
Continue after a restart or a pause.

Agent: after any restart. Lifts a pause (not an owner pause), runs the checks of `start`, then prints the to-do block of the role and the command that re-arms the watch loop. Owner: `--agent <name>` posts `/crew resume <name>`.

Usage: crewboard resume [OPTIONS]

Options:
      --agent <AGENT>
          Owner only: the agent to resume (posts the owner command in the registry issue)

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard pause`

```text
Finish the current step and take nothing new.

Agent: finish the current step, take nothing new. Owner: `--agent <name>` posts `/crew pause <name>`.

Usage: crewboard pause [OPTIONS]

Options:
      --agent <AGENT>
          Owner only: the agent to pause (posts the owner command in the registry issue)

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard stop`

```text
Drain open tickets, then leave the crew.

Agent: drain (open tickets go back to Backlog through the coordinator), then leave. Owner: `--agent <name>` posts `/crew stop <name>`. With `--force` the owner or the coordinator ends an agent that cannot drain: its open tickets go back to Backlog and its State becomes left. The owner's forced stop also posts `/crew stop <name>`, because the registry log, not the State, stops an agent (the key stays valid; `/crew reject` revokes it).

Usage: crewboard stop [OPTIONS]

Options:
      --agent <AGENT>
          Owner or coordinator: the agent to stop (the owner posts the owner command in the registry issue)

      --force
          With --agent: stop an agent that cannot drain, at once

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard watch`

```text
Wait for events and print them (--loop: keep waiting).

Agent: block until there are new events for this agent, print them, exit. With `--loop` it keeps waiting, as a silent loop for a Monitor tool: it prints nothing while there are no events, prints the events once and exits 0 when there are some, and on an error prints ONE line `crewboard-watch: exit <code>: <reason> (<status>)` and exits with that code (2, 3 or 4). `--loop` is the old command `monitor`, which stays callable for ever. With `--loop`, `--json` prints the events and that last line as JSON.

Usage: crewboard watch [OPTIONS]

Options:
      --timeout <TIMEOUT>
          How long to wait (default: `timeout` of the `[loop]` table of the agent config, else 30m)

      --interval <INTERVAL>
          Time between polls (default: `interval` of `[loop]`, else 60s)

      --json
          Print the events as JSON

      --reset-cursor
          Replace a damaged cursor with "now"

      --loop
          Keep waiting: run `watch` again and again until there are events or an error

      --pause <PAUSE>
          With `--loop`: wait after a cycle without events, in seconds (default 2)

      --max-cycles <MAX_CYCLES>
          With `--loop`: stop silently with exit 0 after this many cycles without events (default: never)

      --log <LOG>
          With `--loop`: append one line per cycle to this file (never to standard output)

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard mine`

```text
List your tickets and what each one needs.

Dev: your In Progress and Ready tickets, best first, and what each owned ticket needs now. Exit 0: a ticket needs you now (In Progress without running CI, red or green CI, no PR yet, a failed verify, an answer after a block or a question, or a Ready or parked ticket while nothing is In Progress). Exit 1: nothing to do now, every owned ticket waits (CI of the PR head still runs, no answer yet, or Review), or you own none. An In Progress ticket whose PR head is behind its base shows a `hint:` line (`behind main by N commits; merge main before done`). The hint is advice and never changes the exit code.

Usage: crewboard mine [OPTIONS]

Options:
      --quiet
          Print nothing; only the exit code tells (0 work to do now, 1 nothing to do now), for `crewboard wake`

      --json
          Print one JSON object instead of the text: `agent`, `rows`, `now`, `needs_agent` and `first_ticket`. The exit code is the same

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard take`

```text
Take a Ready ticket and start work.

Dev: ack and move Ready -> In Progress (also resumes a Blocked ticket after an answer).

Usage: crewboard take [OPTIONS] <ISSUE>

Arguments:
  <ISSUE>
          The ticket: an issue number, or `<owner>/<repo>#<n>`

Options:
      --interrupt
          Park your In Progress ticket (it becomes Blocked: "parked for #N") and take this one, which must be strictly ahead of it in your queue (a lower `Order`). The ticket is Ready, or your own Blocked ticket with its answer. Needs `--why`

      --why <WHY>
          Why the current ticket is parked (with `--interrupt`)

      --park <PARK>
          Which In Progress ticket to park, when you have more than one (`<n>` or `<owner>/<repo>#<n>`). Needed only then

      --continue-pr <PR_URL>
          Continue on the open PR that an earlier agent of this ticket left (the URL that the reassign notice and `assign` name). It must be open, in the repo of the ticket, on a crew branch of this ticket, and name the ticket. See spec/tickets#work-that-an-earlier-agent-left

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard done`

```text
Report a finished ticket and move it to Review.

Dev: collect SHA and CI run, check them, post done, move to Review. Merge the base branch into yours before `done`. When the PR head is behind its base, `done` prints one warning line and goes on.

Usage: crewboard done [OPTIONS] --pr <PR> <ISSUE>

Arguments:
  <ISSUE>
          The ticket: an issue number, or `<owner>/<repo>#<n>`

Options:
      --pr <PR>
          URL of the pull request, for example https://github.com/<owner>/<repo>/pull/<n>

      --sha <SHA>
          The commit you worked on; refused if the PR head is another commit now

      --override-unread
          Deliver although events or messages for you are unread (messages, owner comments, and messages on this ticket). Without it, `done` refuses with `unread-events` or `unread-message-on-ticket`: run `watch --timeout 1m` first. Use it only after you have read and handled them; never to skip them

      --run <RUNS>
          A CI run that is part of the evidence (spec/ci#which-ci-runs-count), for a Gate that needs a by-hand run: a run id, or a run URL. Repeat for more runs. It must be a run of the head SHA. It never hides a red push or pull_request run

      --local-run <LOCAL_RUN>
          Only while AUTHORITY.md has `ci_mode: local` (the owner switched CI off): what the local run did, as one line of at most 400 bytes: "<host>: <command> rc=0, log <path>". Required then; the CI of the head is not judged. Refused with `local-gate-not-enabled` in `ci_mode: ci`

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard block`

```text
Move your ticket to Blocked.

Dev: move to Blocked.

Usage: crewboard block [OPTIONS] --why <WHY> <ISSUE>

Arguments:
  <ISSUE>
          The ticket: an issue number, or `<owner>/<repo>#<n>`

Options:
      --why <WHY>
          Why the ticket is blocked

      --override-unread
          Post although a message for you is unread on this ticket (spec/events#unread-messages-on-a-ticket). Use it only after you have read and handled the message; never to skip it

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard ask`

```text
Ask the owner a question.

Any: post a question to the owner and set the label needs-owner (an In Progress ticket of the assignee becomes Blocked).

Usage: crewboard ask [OPTIONS] <ISSUE>

Arguments:
  <ISSUE>
          The ticket: an issue number, or `<owner>/<repo>#<n>`

Options:
      --body-file <BODY_FILE>
          File with the question (`-` reads standard input)

      --body <BODY>
          The question as text

      --override-unread
          Post although a message for you is unread on this ticket (spec/events#unread-messages-on-a-ticket). Use it only after you have read and handled the message; never to skip it

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard say`

```text
Post a signed message.

Agent: post a signed message.

Usage: crewboard say [OPTIONS] --to <TO> --kind <KIND> <ISSUE>

Arguments:
  <ISSUE>
          The ticket: an issue number, or `<owner>/<repo>#<n>`

Options:
      --to <TO>
          Who should read it: an agent name, `coordinator`, `owner` or `all`

      --kind <KIND>
          Kind of the message: one of request, ack, report, done, blocked, question, answer, ping, pong, info.

      --re <RE>
          The message that this one answers: `#<issue>/c-<comment id>` or `none`

      --body-file <BODY_FILE>
          File with the body (`-` reads standard input)

      --body <BODY>
          The body as text

      --override-unread
          Post although a message for you is unread on this ticket (spec/events#unread-messages-on-a-ticket). Use it only after you have read and handled the message; never to skip it

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard ticket`

```text
Create, adopt or split a ticket.

Coordinator: create a ticket (`new`), put an existing issue on the board (`adopt`), or move the open part of a ticket to a new one (`split`).

Usage: crewboard ticket [OPTIONS] <COMMAND>

Commands:
  new    Create a work ticket and put it on the board
  adopt  Put an existing issue on the board
  split  Move the open part to a new ticket
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard ticket new`

```text
Create a work ticket and put it on the board.

Create the `crew:work` issue as the crew account, check its `Gate:` line, put it on the board (Kind Work, Status Backlog, Gate) and write the signed info comment, so that `status` lists it. Default: park it in Backlog.

Usage: crewboard ticket new [OPTIONS]

Options:
      --title <TITLE>
          Title of the new issue
          
          [default: ""]

      --body-file <BODY_FILE>
          File with the ticket text (`-` reads standard input). It needs a `Gate:` line
          
          [default: ""]

      --template
          Print the body template (the lines of a ticket body) and stop. It reads no config and calls no board

      --repo <REPO>
          Repo of the issue (default: the main repo of the initiative)

      --backlog
          Park the ticket in Backlog (the default)

      --assign <ASSIGN>
          Assign it to this agent at once (the checks of `assign` run before anything is created)

      --continue-pr <CONTINUE_PR>
          Continue the OPEN PR of a Done ticket (a PR URL), or the open PR that the reassign notice of a ticket in Backlog or Ready names: `done` then accepts its branch for this ticket. See spec/tickets#a-follow-up-ticket-continues-an-open-pr

      --gate-ci
          A ticket whose Gate is CI only: write the Gate line `Gate: ci`. The headless `tick` merges a ticket only when its signed Gate is CI only (`ci`); this flag writes it. Leave the `Gate:` line out of the body (a body with another Gate is refused)

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard ticket adopt`

```text
Put an existing issue on the board.

Put an existing issue (label `crew:work`, a `Gate:` line) on the board in the same way. A ticket that is on the board already keeps its Status.

Usage: crewboard ticket adopt [OPTIONS] <ISSUE>

Arguments:
  <ISSUE>
          The ticket: an issue number, or `<owner>/<repo>#<n>`

Options:
      --assign <ASSIGN>
          Assign it to this agent after it is on the board

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard ticket split`

```text
Move the open part to a new ticket.

Move the open part of an In Progress ticket to a new ticket for the same agent, so that the finished part can merge. Links both ways; one signed info note on the old ticket. A second identical split changes nothing. Refused if AUTHORITY.md has `coordinator_may_split_tickets: false`.

Usage: crewboard ticket split [OPTIONS] --title <TITLE> --body-file <BODY_FILE> <ISSUE>

Arguments:
  <ISSUE>
          The ticket: an issue number, or `<owner>/<repo>#<n>`

Options:
      --title <TITLE>
          Title of the new ticket

      --body-file <BODY_FILE>
          File with the text of the new ticket (`-` reads standard input). The `Gate:` line of the old ticket is copied unless the file has its own

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard assign`

```text
Give a ticket to an agent, or move a queued ticket.

Coordinator: check the gate and the agent state, move to Ready. On a ticket that is queued already, `--order <n>` (with or without `--to` its agent) sets its position in the queue, and `--to <other agent>` moves it to the queue of that agent.

Usage: crewboard assign [OPTIONS] <ISSUE>

Arguments:
  <ISSUE>
          The ticket: an issue number, or `<owner>/<repo>#<n>`

Options:
      --to <TO>
          Name of the dev agent that gets the ticket, for example claude@host-a. Only `assign <n> --order <number>` may leave it out (the queued ticket keeps its agent)

      --override-capabilities
          Assign although the agent lacks a capability that the ticket needs (`Needs:` line or `needs:` label). Needs --why; both are recorded in the signed assignment message

      --why <WHY>
          Why the override is right (only with --override-capabilities)

      --order <ORDER>
          Position in the queue of the agent: a whole number, lowest first (board field `Order`, created on first use). The next ticket of an agent is its lowest `Order` among Ready. Default: the end of the queue. On a queued ticket this moves it within the queue (or, with another `--to`, sets the position in the new queue; the default there is the end of it)

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard unblock`

```text
Answer a block so the agent can resume.

Coordinator: post the answer that lets the assignee resume a Blocked ticket with `take`. A ticket that waits for a question to the owner gets an answer that withdraws the question.

Usage: crewboard unblock [OPTIONS] <ISSUE>

Arguments:
  <ISSUE>
          The ticket: an issue number, or `<owner>/<repo>#<n>`

Options:
      --body-file <BODY_FILE>
          File with the answer (`-` reads standard input). Default: "resume allowed: run take <n>"

      --body <BODY>
          The answer as text

      --override-unread
          Post although a message for you is unread on this ticket (spec/events#unread-messages-on-a-ticket). Use it only after you have read and handled the message; never to skip it

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard reassign`

```text
Take a ticket from its agent.

Coordinator: with --to the ticket goes to Ready for that agent, without it to Backlog (a ticket in Ready always has an agent). A reason is needed: --reason dead, stopping, late or blocked-too-long (with --why for the text), or --derived.

Usage: crewboard reassign [OPTIONS] <ISSUE>

Arguments:
  <ISSUE>
          The ticket: an issue number, or `<owner>/<repo>#<n>`

Options:
      --reason <REASON>
          Why: dead, stopping, late or blocked-too-long. Needed unless --derived
          
          [possible values: dead, stopping, late, blocked-too-long]

      --why <WHY>
          The free text that goes with the reason (what you saw)

      --derived
          Derive the reason from the board: the agent of the ticket is dead (no heartbeat for 5 intervals) or draining or left. Refused if it is none of these

      --to <TO>
          The new agent: the ticket goes to Ready for it. Without --to the Agent field is cleared and the ticket goes to Backlog (`assign` sets an agent later)

      --override-capabilities
          With --to: the new agent lacks a capability that the ticket needs; the --reason of the reassign is the reason of the override and is recorded

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard verify`

```text
Check evidence and CI of a ticket in Review.

Coordinator: check evidence and CI of a ticket in Review.

Usage: crewboard verify [OPTIONS] <ISSUE>

Arguments:
  <ISSUE>
          The ticket: an issue number, or `<owner>/<repo>#<n>`

Options:
      --body <BODY>
          What you found when you checked the Gate yourself (recorded in the result)

      --fail
          Your own Gate check failed although the evidence holds: record a failed verify with the body as the failure (needs --body; not for a headless coordinator)

      --run <RUNS>
          A CI run that is part of the evidence (spec/ci#which-ci-runs-count); see `done --run`

      --local-run <LOCAL_RUN>
          What you checked for a local-run done (`ci_mode: local`): host, command, rc, log. Required then (not with --fail). Refused with `local-gate-not-enabled` in `ci_mode: ci`; not for a headless coordinator

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard merge`

```text
Merge the PR of a verified ticket.

Coordinator: merge the PR of a verified ticket at the verified head SHA, delete its branch, close the ticket. Only if AUTHORITY.md lets the coordinator merge.

Usage: crewboard merge [OPTIONS] <ISSUE>

Arguments:
  <ISSUE>
          The ticket: an issue number, or `<owner>/<repo>#<n>`

Options:
      --method <METHOD>
          How GitHub merges: squash (default), merge or rebase
          
          [default: squash]

      --run <RUNS>
          A CI run that is part of the evidence (spec/ci#which-ci-runs-count); see `done --run`

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard close`

```text
Close a ticket (Done is final).

Coordinator: close a ticket (Done is final). Without a flag crewboard looks for the owner's merge of the PR; `--why` closes by your judgment; `--verified` closes after a passed verify.

Usage: crewboard close [OPTIONS] <ISSUE>

Arguments:
  <ISSUE>
          The ticket: an issue number, or `<owner>/<repo>#<n>`

Options:
      --verified
          Confirm that a passed `verify` exists for this ticket (Review -> Done)

      --why <TEXT>
          Close by your judgment when no owner merge exists (no PR, or the ticket will not be done). The text goes into the note on the ticket; name a replacing ticket in it

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard audit`

```text
Find unsigned, forged or edited messages.

Owner: list headerless owner comments, bad signatures, edited messages, wrong writers.

Usage: crewboard audit [OPTIONS]

Options:
      --json
          Print the findings as JSON

      --since <SINCE>
          Only findings of comments created since this time (`2026-10-16T08:00:00Z`) or this long ago (`90m`, `2h`)

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard report-status`

```text
Post a project status update.

Coordinator: post a project status update (first line `from=<agent>`).

Usage: crewboard report-status [OPTIONS] --health <HEALTH>

Options:
      --health <HEALTH>
          on-track, at-risk, off-track, inactive or complete

      --body <BODY>
          The status text

      --body-file <BODY_FILE>
          File with the text (`-` reads standard input)

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard prune`

```text
Remove Agent options of agents that left.

Coordinator: remove Agent options of agents in state `left` that no ticket uses.

Usage: crewboard prune [OPTIONS]

Options:
      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard lock`

```text
crewboard lock - a host lock that a dead agent cannot hold forever.

Agents on a shared host take one lock before a heavy job (full build, stress gate, load
test). The lock is a directory (mkdir is atomic) with an owner file. If the owner dies,
the next taker sees that the lock is STALE and takes it over.

Usage:
  crewboard lock take    <dir> --holder AGENT [--reason TEXT] [--wait S] [--pid P] [--expect D]
  crewboard lock release <dir> --holder AGENT
  crewboard lock status  <dir>
  crewboard lock break   <dir> --if-dead [--log FILE]   (remove the lock only if it is stale)
  crewboard lock run     <dir> --holder AGENT [--reason TEXT] [--wait S] [--gate] -- CMD...
  crewboard lock run     <dir> --holder AGENT --detach --result FILE [--ticket N] [--sha HEAD] [--reason TEXT] [--wait S] [--gate] -- CMD...
  crewboard lock ci-wait <dir> --holder NAME      (a CI step: register in the waiting list)
  crewboard lock ci-done <dir> --holder NAME      (a CI step: leave the waiting list)
  crewboard lock ci-status <dir>                 (exit 0 and a line if CI runs or waits, else 1)

Options (all subcommands that take a lock):
  --holder AGENT   who holds the lock (the agent name, for example claude@host-b). Required.
  --reason TEXT    why (shown to the others), for example "make test".
  --wait D         how long to wait for a live lock (default 0 = do not wait; exit 75).
                   A duration D is whole seconds or a number with a unit: 90, 90s, 30m, 2h.
                   All time options (--wait --poll --max-age --empty-grace --ci-max-age
                   --expect) take this syntax.
  --poll D         time between two tries while waiting (default 5).
  --pid P          the process that holds the lock (take; ci-wait records it in its entry).
                   Default: the parent process of this process (ci-wait: none).
                   0 = no pid check (see "pid" below). It needs --expect (exit 64 without).
                   runner = the nearest `Runner.Worker` ancestor of this process (a GitHub
                   Actions job). It lives exactly as long as the job. Exit 64 if there is none.
  --max-age D      a lock older than D is stale (default 14400 = 4 h, or
                   $HOST_LOCK_MAX_AGE). 0 = no age limit.
  --expect D       how long the job should take (take, run). It goes in the owner file as
                   `expect=<seconds>`. Required with `--pid 0`. A lock whose pid cannot be
                   judged (pid=0, no pid, or a pid of another host) is stale after
                   D x 2 + 5 min, reason `overdue`. An owner file without `expect=` keeps the
                   --max-age rule only.
  --no-board       do not ask the board whether the owner agent is dead.
  --config-dir D   the config dir of the owner agent, for the board check (also the global
                   option `crewboard --config-dir D lock ...`). $CREWBOARD_CONFIG_DIR is its
                   default. An agent whose config is not in the default dir (for example
                   ~/.config/crewboard/agents/<agent-dir>) must set one of them, or the board
                   check cannot run. Without a config dir and without a default config,
                   `status` prints one line "board check unavailable: no agent config dir".
                   take and run stay silent about it, so a poll loop does not repeat it.
  --initiative I   the initiative of the board check, when the config dir has more than one.
  --log F          also append the log lines to F.
  --empty-grace D  an EMPTY lock directory counts as stale only when it is older than D
                   (default 30, or $HOST_LOCK_EMPTY_GRACE). A fresh empty directory is
                   a taker between mkdir and the owner file. A directory with a foreign file
                   is not judged by this (see "Foreign holder").
  --ci             this taker is a CI job: it ignores CI presence and waiting CI entries
                   (it is never blocked by a waiting agent, only by a held lock). While it
                   waits it is on the waiting list itself. For take and run.
  --ci-max-age D   a waiting-list entry older than D is ignored (default 21600).
  --gate           this job is a merge-deciding gate: skip the CI-first wait for a
                   running or waiting CI job. Every other rule stays: a held lock is waited for.
                   The caller chooses it, for example under `ci_mode: local`. For take and run.
  --detach         (run only, needs --result) wait for and take the lock as `run` does, then start
                   CMD in its own session and return at once with exit 0. Stdout:
                   `detached: pid=<job pid> log=<file> result=<file>`. The job holds the lock until
                   it ends, then writes the result file and releases the lock. A kill of the
                   caller (for example the time budget of a wake) neither stops the job nor
                   releases its lock.
  --result FILE    with --detach: one line `rc=<n> end=<utc> log=<path>` appears here when the job
                   ends (written to a temp file, then renamed). Removed at the start of the run.
                   The job output goes to FILE.log.
  --ticket N       with --detach: the result file gets a second line `ticket=N`. The wake
                   wrapper (poll-and-wake.sh) names it in the events file when the job has ended.
  --sha HEAD       with --detach: the result file gets a line `sha=HEAD` (the PR head; optional).

Stale lock: any ONE of these named reasons. There is no other way to take a lock over.
  overdue:           the owner file has `expect=`, the pid cannot be judged (pid=0, no pid, or
                     another host) and the lock is older than expect x 2 + 5 min.
  pid-dead:          the lock was made on THIS host and its pid is gone. A pid that another
                     user owns (the signal check gives EPERM) is alive. On another host the
                     pid says nothing, so it is never judged.
  too-old:           started longer ago than --max-age (any host).
  board-dead / board-left: the board says the owner agent is dead, or its State is
                     left. The check runs in this process (the same answer as `crewboard
                     status`). If the board or the config is not reachable, the
                     check is skipped, and the lock is not judged by the board. The note on
                     stderr names the cause (the first line of the error),
                     once per cause; a working check after a failure gives one line "board
                     check works again". A failure never stops a wait: the pid and age
                     checks go on. The answer is cached for 60 s (also a failed one; set
                     $HOST_LOCK_BOARD_TTL for tests), so a wait polls the board once a minute,
                     not at every poll. An agent that the board does
                     not list is not judged.
  empty-after-grace: the lock directory is EMPTY (no owner file, no other file) AND is older
                     than --empty-grace. A fresh empty directory is NEVER taken over.
Foreign holder: a directory with no owner file but WITH another file is a lock
that a project tool or CI step made with its own mkdir and info file. It is never
`empty-after-grace`. It is stale only by `pid-dead` (a line `pid=<n>` in any regular file of the
directory, an optional line `host=<name>` in the same file; the pid is judged only on this host,
a missing host line means this host, `pid=0` means none) or by `too-old` (--max-age, counted from
the mtime of the directory; --max-age 0 = never). status, the wait message and the takeover line
say `foreign holder: <file>: <first line>` (control characters removed, 120 characters at most).
A tool should write an `owner` file instead (see docs/guides/ci-and-host-lock.md).
Invalid owner file: an `owner` file that is not empty but has no non-empty `agent=`
line or no numeric `started_epoch=` line is NOT a valid owner file (for example one line
`<agent>@<host>` written by hand). It is judged as a foreign holder, with two changes: the
owner file is the first file of the pid search, and `too-old` counts from the mtime of the
owner file (not of the directory). No board, overdue or empty-after-grace rule applies. The
text is `foreign holder: owner (<what is wrong>): <first line>`, for example
`foreign holder: owner (no key=value fields): agent-a@host-b`. An empty `owner` file (0 bytes)
counts as no owner file, as before.
CI first: an AGENT take (also a takeover) waits, or exits 76 without --wait, while
a CI job runs or waits on this host. CI is seen in two ways: a process `Runner.Worker` (the
GitHub Actions runner; $HOST_LOCK_PS_CMD replaces `ps -axo command=` for tests), and a file
in the waiting list `<dir>.waiting/<name>`. A CI step registers with `ci-wait` BEFORE it waits
for the lock and leaves with `ci-done` (use `if: always()`); `take --ci`/`run --ci` do both
by themselves while they wait. A CI take is blocked only by a held lock.
A stale lock is taken over atomically: only one taker passes the takeover guard
(`<dir>.takeover`, a mkdir), re-checks that the lock is still stale, renames it to a unique
name (mv) and then makes its own. The takeover prints ONE log line with the old owner and
the reason. Nobody does rm-then-mkdir on the live name.

pid: `take` records a pid so that others can see a dead owner. Each Bash call of an agent
is a short-lived shell, so its parent pid is gone when the call ends. For take and
release in two separate calls use `--pid 0 --expect <d>` (the age, overdue and board checks
still work; a dead holder cannot be detected), or better use `run`, which holds the lock for exactly as long as its own process lives.
In CI use `--pid runner` for `take` and `ci-wait`: Runner.Worker lives as long
as the job, so a job that dies frees its lock and its waiting entry (same host only).
The walk uses `ps -o ppid= -p P` and `ps -o comm= -p P`. For tests, $HOST_LOCK_PARENT_CMD
replaces both: it is run as `sh -c "$HOST_LOCK_PARENT_CMD" sh P` and prints "PPID COMM".
A waiting entry (`<dir>.waiting/<name>`, key=value lines) has a `pid=` line when a pid is
known. An entry whose pid is dead on its own host is ignored and removed. An entry without
a `pid=` line (an older version wrote it) is judged by its age only.

Release removes the lock only if the owner file is valid and names the same agent and host.
It is atomic: the lock dir is renamed to a unique name first and removed after, so no
other process sees an empty lock dir during a release.
Anyone else gets exit 3 and the lock stays. An invalid owner file is never released, whatever
agent and host it names: only a stale takeover removes it.

Owner file (`<dir>/owner`, key=value lines): agent, host, pid, started (UTC),
started_epoch, reason, expect (seconds; only when --expect was given), detached=1 (a detached job;
its pid is the pid of the job supervisor, which lives as long as the job). It is valid only with a
non-empty `agent=` and an all-digit `started_epoch=` (see "Invalid owner file").

Re-entrancy: `run` exports CREWBOARD_HOST_LOCK=<the lock dir, resolved with
`pwd -P`> and CREWBOARD_HOST_LOCK_PID=<the holder pid> to CMD (not set anywhere else). A
`take` or `run` for that dir, in a process whose ANCESTOR is the recorded holder (same host),
and that carries both variables with the same dir and pid, gets the lock at once. It prints
one line, writes no second owner file, and its `release` leaves the outer lock alone. An
ancestor holder WITHOUT that grant (for example a `take --pid <your shell>`, or an env that a
wrapper stripped) is not re-entered: take/run exit 77 at once, with no wait, and say "held by
your own parent process". A tool that takes the lock itself must not be wrapped in `run`.

Exit codes: take/run: 0 got the lock (run: the exit code of CMD), 75 held by a live owner
(and --wait over), 76 CI runs or waits (agents only, and --wait over), 77 held by your own
ancestor process and no re-entry (no wait), 64 usage. release: 0 released or nothing to release
(also a nested release, which leaves the outer lock), 3 not the owner.
status: 0 held by a live owner, 1 free, 2 stale (it prints the reason). A held line ends with
alive=yes|no|unknown (kill -0, only for a pid on this host) and age=<s>.
break: 0 free, or stale and removed; 3 held by a live owner (refused, lock untouched); 64 no --if-dead.
Unix only (macOS, Linux).
```

### `crewboard repeat`

```text
Run a command N times on one commit and keep evidence.

Any: run a command N times in a row (a flake check: the same command, the same commit) and keep honest evidence. Every run writes its own log, is judged by its own exit code and result lines, and must run on one commit. `crewboard repeat --help` prints the full text.

Usage: crewboard repeat [OPTIONS] [-- <CMD>...]

Arguments:
  [CMD]...
          The command of one run, after `--`

Options:
      --times <TIMES>
          Number of runs in a row (a whole number, at least 1)

      --lock <LOCK>
          Take the host lock in this directory around EACH run

      --holder <HOLDER>
          The lock holder (default `$CREWBOARD_AGENT`, else `gate-loop`)

      --lock-wait <LOCK_WAIT>
          How long to wait for a live lock per run (default 3600 seconds; 90, 90s, 30m, 2h)
          
          [default: 3600]

      --log-dir <LOG_DIR>
          Where `run-<n>.log` go (default: a new temp directory). Old files of that name are removed before the run, so they can never count

      --work-dir <WORK_DIR>
          The git checkout whose HEAD is recorded; CMD runs there (default: this directory)

      --result-re <RESULT_RE>
          Regular expression (the `regex` crate, per line) of a result line of the runner

      --fail-re <FAIL_RE>
          Regular expression that makes a result line red (default: `FAILED` or a non-zero failed count). Used only with --result-re

      --no-ci-check
          Do not look for CI before each run

      --ci-poll <CI_POLL>
          Seconds between two CI checks while paused (default 10)
          
          [default: 10]

      --ci-max-wait <CI_MAX_WAIT>
          The longest pause for CI per run (default 3600 seconds). After that: exit 3
          
          [default: 3600]

  -h, --help
          Print help (see a summary with '-h')

A run is green only if the command exits 0. With --result-re it must also have a result line in its own log, and no result line may match --fail-re. A missing result line is red. The head commit (git rev-parse HEAD) is read before and after every run; a change ends the loop with exit 2. The loop stops at the first red run. The command runs without a shell, in --work-dir.

Result lines of common runners (--result-re). The command is the one your repo uses; the exit code alone is enough for any runner, and --result-re only adds proof that tests ran:
  pytest:   --result-re '^=+ .*(passed|failed)'  (===== 5 passed in 0.12s =====)
  Jest:     --result-re '^Tests: '               (Tests: 1 failed, 4 passed, 5 total)
  Vitest:   --result-re '^\s*Tests\s'            (     Tests  5 passed (5))
  go test:  --result-re '^(ok|FAIL)\s'           (ok  example.org/app  0.12s)
  Maven:    --result-re '^Tests run:'            (Tests run: 12, Failures: 0, Errors: 0)
  Gradle:   --result-re '^BUILD (SUCCESSFUL|FAILED)'
  xcodebuild: --result-re '^\*\* TEST (SUCCEEDED|FAILED) \*\*'
  cargo:    --result-re '^test result: '

Before each run `repeat` waits while a CI job runs or waits on this host (the host lock check).

Exit codes: 0 all N runs green on one commit; 1 a red run; 2 the commit changed (or --lock on a host without Unix process control); 3 CI did not finish within --ci-max-wait; 4 setup error (not a git checkout, cannot write the logs); 64 usage; 143 a TERM, INT or HUP stopped `repeat` (the command got it too, the lock is released).
```

### `crewboard wake`

```text
Wake an unattended agent from a scheduler (`wake install`).

One run polls the board and starts the wake command only when there is something to do; a quiet board starts no model. `wake install --agent-dir DIR` writes and enables the unit that runs it every few minutes; `wake uninstall` and `wake status` undo and show it. `agent add --install-wake` is the normal way to install it.

Tuning options that work but are not listed below: --work-backoff, --retry-delays, --stale-after, --empty-grace, --state-dir, --lock-dir, --log, --events-file, --max-wake, --heartbeat-every, --print-state-dir, --shim-dir, --allow, --runtime-flags, --runtime-bin, --prompt-extra, --host-lock, --no-events-hook, --events-every and --print-cmd.

Usage: crewboard wake [OPTIONS]
       crewboard wake <COMMAND>

Commands:
  install    Install and enable the scheduler unit of one agent
  uninstall  Disable and remove the scheduler unit of one agent
  status     Show the unit of one agent, if it is enabled, and its last run
  help       Print this message or the help of the given subcommand(s)

Options:
      --agent-dir <AGENT_DIR>
          The config dir of the agent: reads `<dir>/<initiative>.toml` for the agent name and the initiative, and uses the dir as `--config-dir`

      --initiative <INITIATIVE>
          The initiative (default: the only one of the agent dir)

      --tick
          Run `crewboard tick` under the lock on every run and start no model

      --agent <AGENT>
          The agent name

      --runtime <RUNTIME>
          The runtime of the wake: claude, codex or hermes. The wake command, its prompt and its permission flags come from it (`--print-cmd` shows them)

      --work-dir <WORK_DIR>
          The checkout of the agent. The wake command starts there

      --model <MODEL>
          The model of the wake, given to the runtime as `--model ID`

  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard wake install`

```text
Install and enable the scheduler unit of one agent.

The unit (launchd, systemd or cron) runs `crewboard wake` for the agent every few minutes. It writes a wrapper script next to the agent config and the unit, then enables it. An install of an earlier version (also of `agent-wake.sh`) is replaced, not doubled. Prints the one command that you still have to run, if there is one. With `--dry-run` it prints the files and the commands and changes nothing.

Usage: crewboard wake install [OPTIONS]

Options:
      --scheduler <SCHEDULER>
          The scheduler of the unit

          Possible values:
          - auto:    launchd on macOS; systemd --user on Linux when it works, else cron; hermes for a Hermes agent
          - launchd
          - systemd
          - cron
          - hermes
          
          [default: auto]

      --every <EVERY>
          The interval between two runs, for example 5m, 90s or 1h
          
          [default: 5m]

      --dry-run
          Print the files and the commands, and change nothing

      --replace
          Remove the wake jobs of other names that run this agent (a copy of each file stays as `<file>.bak-<date>`). Without it, such a job makes the install refuse with exit 2

      --agent-dir <AGENT_DIR>
          The config dir of the agent: reads `<dir>/<initiative>.toml` for the agent name and the initiative, and uses the dir as `--config-dir`

      --initiative <INITIATIVE>
          The initiative (default: the only one of the agent dir)

      --tick
          Run `crewboard tick` under the lock on every run and start no model

      --agent <AGENT>
          The agent name

      --runtime <RUNTIME>
          The runtime of the wake: claude, codex or hermes. The wake command, its prompt and its permission flags come from it (`--print-cmd` shows them)

      --work-dir <WORK_DIR>
          The checkout of the agent. The wake command starts there

      --model <MODEL>
          The model of the wake, given to the runtime as `--model ID`

  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard wake uninstall`

```text
Disable and remove the scheduler unit of one agent.

Removes the unit and the wrapper. The state and the log stay.

Usage: crewboard wake uninstall [OPTIONS]

Options:
      --agent-dir <AGENT_DIR>
          The config dir of the agent (the names of the unit come from it)

      --initiative <INITIATIVE>
          The initiative (default: the only one of the agent dir)

      --scheduler <SCHEDULER>
          Only this scheduler (default: every scheduler that has a unit of this agent)

          Possible values:
          - auto:    launchd on macOS; systemd --user on Linux when it works, else cron; hermes for a Hermes agent
          - launchd
          - systemd
          - cron
          - hermes

  -h, --help
          Print help (see a summary with '-h')
```

#### `crewboard wake status`

```text
Show the unit of one agent, if it is enabled, and its last run

Usage: crewboard wake status [OPTIONS]

Options:
      --agent-dir <AGENT_DIR>
          The config dir of the agent (the names of the unit come from it)

      --initiative <INITIATIVE>
          The initiative (default: the only one of the agent dir)

      --scheduler <SCHEDULER>
          Only this scheduler (default: every scheduler that has a unit of this agent)

          Possible values:
          - auto:    launchd on macOS; systemd --user on Linux when it works, else cron; hermes for a Hermes agent
          - launchd
          - systemd
          - cron
          - hermes

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard poll`

```text
One poll for a scheduler.

Scheduler: one poll. Start the model only on exit 0.

Usage: crewboard poll [OPTIONS]

Options:
      --quiet
          Only report whether events are pending (exit 0 / 1 / >=2). Prints no events and does not move the cursor

      --json
          Print the events as JSON

      --reset-cursor
          Replace a damaged cursor with "now"

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard tick`

```text
Verify and merge CI-only tickets in Review.

Headless coordinator: verify, and merge when allowed, every Review ticket whose Gate is CI only. Needs no event. Refused in a config dir that is not headless.

Usage: crewboard tick [OPTIONS]

Options:
      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard heartbeat`

```text
Send one heartbeat.

Agent: one heartbeat.

Usage: crewboard heartbeat [OPTIONS]

Options:
      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

### `crewboard monitor`

```text
Silent watch loop for a Monitor tool.

Agent: a silent loop around `watch` for a Monitor tool. It prints nothing while there are no events, prints the events once and exits 0 when there are some, and on an error prints ONE line `crewboard-watch: exit <code>: <reason> (<status>)` and exits with that code (2, 3 or 4). Arguments after `--` go to `watch`. `watch --loop` is the same command. The name `monitor` stays for ever, because Monitor tool configs call it.

Usage: crewboard monitor [OPTIONS] [-- <WATCH_ARGS>...]

Arguments:
  [WATCH_ARGS]...
          Arguments for `watch`, after `--`

Options:
      --timeout <TIMEOUT>
          How long one `watch` waits (default: `timeout` of the `[loop]` table, else 30m)

      --interval <INTERVAL>
          Time between polls inside one `watch` (default: `interval` of `[loop]`, else 60s)

      --pause <PAUSE>
          Wait after a cycle without events, in seconds (default 2)

      --max-cycles <MAX_CYCLES>
          Stop silently with exit 0 after this many cycles without events (default 0 = never)
          
          [default: 0]

      --log <LOG>
          Append one line per cycle to this file (never to standard output)

      --json
          Print the final error line as JSON: `{"monitor":"exit","code":..,"reason":"..","status":".."}`

      --reset-cursor
          Replace a damaged cursor with "now"

      --initiative <INITIATIVE>
          Initiative name (default: the only one in the config dir)

  -h, --help
          Print help (see a summary with '-h')
```

