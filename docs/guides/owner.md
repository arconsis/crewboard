# Owner guide

You run `init` once, you approve each agent, and you merge what needs your decision. For a
first crew, follow [Getting started](../getting-started.md). For the threats that this setup
stops, read [Security](security.md).

Placeholders: `<initiative>` is a short name such as `my-crew`, `<account>/<repo>` is the
initiative repo (your GitHub login or organisation, and a repo with at least one commit),
`<crew-login>` is the login of the crew account.

## Before you start

- An owner machine and the agent hosts. One laptop can be all of them. The tools are in
  [Install](install.md#what-you-need).
- `gh` logged in as you (`gh auth login`). `init` checks the `project` scope before it writes
  anything and prints the exact `gh auth refresh -h github.com -s project` line if it is
  missing.
- The runtime and its login on each agent host (`claude` or `codex login`).
- A second e-mail address for the crew account. A plus address works if your provider
  delivers it.
- A CI workflow in `<account>/<repo>` that runs the tests: `done` needs a CI run for the head
  of each PR. `init --with-ci` adds a minimal one ([CI for crew pull requests](ci-for-crew-prs.md)).
- Organisation repo only: permission to create Projects. If the organisation restricts OAuth
  apps, an organisation owner must allow GitHub CLI.

The token comes from an env file (`--env-file`, or `agent.env` in an agent dir), else from
`GH_TOKEN` or `GITHUB_TOKEN`, else from `gh auth token`. crewboard never
stores it and never prints it. Run owner commands as yourself, without a token variable:
`env -u GH_TOKEN -u GITHUB_TOKEN crewboard ...`. Run them only on the owner machine, or in a
terminal without the agent environment. In a terminal with the crew token, plain `gh` acts as
the crew account.

## The crew account and its token

GitHub's terms call the crew account a machine account: you make it, you accept the terms for
it, and you are responsible for what it does. Use it only for the agents. Agents never create
accounts or tokens. You do it by hand.

1. Make the account, give it write access and accept the invitation: steps 2 to 4 of
   [Getting started](../getting-started.md#the-steps). For an organisation repo, invite the
   account to the organisation, add it to the repo with the role Write, and accept at
   `https://github.com/orgs/<account>/invitation`.
2. Make the token as the crew account (Settings, Developer settings, Personal access tokens):

   | Board | Token | Exact access |
   |---|---|---|
   | User-owned | Tokens (classic) | Scopes `repo` and `project`. Nothing else. A fine-grained token cannot reach a user-owned project. |
   | Organisation | Fine-grained tokens | Resource owner: the organisation. Repositories: only the initiative repos. Repository permissions: Contents, Issues and Pull requests read and write; Actions, Checks and Commit statuses read-only. Organisation permissions: Projects read and write. |

   A classic token with `repo` and `project` reaches every repository and project that the crew
   account can reach. So give the crew account access to the initiative repos only. Never give
   the token `workflow` (classic) or Workflows and Administration (fine-grained): workflow
   changes are yours. For an organisation board, an organisation owner must allow fine-grained
   tokens first, and approve the token if approval is on.
3. On the first agent host, store the token with `agent token` (step 6 of
   [Getting started](../getting-started.md#the-steps)). It reads the token from standard input,
   asks GitHub whose token it is and which scopes it has, and writes
   `~/.config/crewboard/<initiative>.env` (mode 600) only when the token is right. It refuses a
   token without `repo` and `project`, a classic token with `workflow`, and (with
   `--crew-login`) a token of another account. On each other agent host, `agent add
   --token-stdin` does the same ([Agents guide](agents.md#join-an-agent)). Run both from your
   own terminal, not through an agent.

`crewboard doctor --setup` checks this token on the agent host ([Agents](agents.md#check-the-setup)).

## Init

```sh
# once per initiative (idempotent; --project adopts an existing project)
env -u GH_TOKEN -u GITHUB_TOKEN crewboard init --initiative <initiative> \
  --repo <account>/<repo> [--repo ...] --crew-login <crew-login> [--project 15] [--with-ci]
```

`init` makes the board, its fields, its views (the ticket views show Title, Status, Agent and
Order, sorted by Order) and labels, the registry issue, and a pull request with `AUTHORITY.md` and
`.crewboard.toml`. With `--with-ci`, the pull request also holds a minimal CI workflow when the
main repo has none ([CI](ci-for-crew-prs.md)). It writes
`~/.config/crewboard/<initiative>.board.toml` (no secret) and ends with an owner checklist: each
open web UI step with its direct URL. Without `--crew-login`, `init` refuses unless you give
`--insecure-one-account`. That flag makes a one-person trial ([Quick try](../quick-try.md)), not
a crew.

The queue position of a ticket is its `Order` field (`crewboard status --queue` shows it).
To move a ticket to another agent or position, run `crewboard assign <n> --to <agent> [--order N]`.
Do not drag the card or edit `Agent` by hand: the new agent's `take` is then refused with
`ticket-agent-not-signed` ([Concepts](../concepts.md#done-without-proof-is-detected)).

An agent that joins before you merge the init pull request reads the board file: copy
`~/.config/crewboard/<initiative>.board.toml` to its host. After the merge, the repository is
enough.

## Branch rule

`init` adds a repository ruleset to the default branch of the main repo. It requires a pull
request, so an agent cannot push a change of `AUTHORITY.md` or `.crewboard.toml` directly. What
the rule holds, what `init` does when GitHub refuses, and `--no-branch-rule`:
[Branch rule](branch-rule.md).

## The board checklist

`init` creates the views and deletes the four built-in project workflows that write `Status`
behind crewboard (one way; `doctor --board` warns if one is on again). A view that exists keeps
its columns. `assign` and `reassign` also put a label `agent: <name>` on the issue, so a missing
`Agent` column is a convenience only. The one step left: edit `AUTHORITY.md`, then merge the init
pull request. `start` reads `AUTHORITY.md` from the default branch.

## AUTHORITY.md

Edit the file on the branch `crewboard/init-<initiative>` of the init pull request. The front
matter has these keys:

| Key | Default | Meaning |
|---|---|---|
| `coordinator_may_merge` | `false` | `false`: only you merge. |
| `coordinator_may_create_tickets`, `coordinator_may_split_tickets`, `dev_may_open_prs` | `true` | Limits for `assign`, `ticket split` and `done`. |
| `dev_branch_prefix` | `crew/` | The branch prefix of the PRs of dev agents. |
| `protected` | `[main]` | Branches. A PR from a protected branch is no evidence. |
| `protected_paths` | none | More paths that only you merge, as globs (`tests/**`, `scripts/ci-*.sh`). It adds to the built-in seven and never removes one. |
| `ci_ignore` | none | CI jobs that the CI check skips ([details](../design/spec/ci.md#jobs-that-ci_ignore-skips)). |
| `ci_mode` | `ci` | `local` when CI cannot run ([Local run](../design/spec/ci.md#local-run-ci_mode-local)). |
| `former_repos` | none | After a repo transfer or rename: the old name and a cutoff ([Move the repository](repo-move.md)). |
| `needs_owner` | a list | Advice for the models: when to ask you. crewboard does not enforce it, with one exception: `merge` in the list still lets only you merge (rule `authority-needs-owner`, deprecated; set `coordinator_may_merge: false` instead). It is not the label `needs-owner` ([Concepts](../concepts.md#glossary)). |

Whatever the keys say, the crewboard CLI lets only you merge a PR that changes a protected
path ([Protected paths](#protected-paths)).

The toolchain keys are in `.crewboard.toml`, not in `AUTHORITY.md`:

| Key | Default | Meaning |
|---|---|---|
| `required_tools`, `smoke_cmd` | none | `start` refuses a dev agent that cannot build the repo ([toolchain check](../design/spec/agents.md#toolchain-check-of-start)). A `required_tools` entry may name agent tags: `{ cmd = "node --version", tags = ["web"] }` ([tools per tag](agents.md#tools-per-agent-tag)). The commands are those of your repo. |
| `detect_tools` | none | Tool names that `join` and `start` probe for a version, on top of a default list of common tools: `["java", "gradle"]`. A found tool becomes the fact `tool:<name>`; a missing one is no error ([facts](../design/spec/profiles.md#facts)). |

Below the front matter of `AUTHORITY.md`, write the goal and the scope in plain words. The agents
read it.

## Protected paths

The crewboard CLI lets only you merge a pull request that changes one of seven paths. The code
fixes the list, and `AUTHORITY.md` cannot shorten it. A rename out of a protected path counts.

1. The authority file (`AUTHORITY.md` by default, in the repo root).
2. `.crewboard.toml` (in the repo root).
3. `.github/CODEOWNERS`.
4. `CODEOWNERS` (in the repo root).
5. `docs/CODEOWNERS`.
6. Everything under `.github/workflows/`.
7. Everything under `.github/actions/`.

`verify`, `close --verified` and `merge` (so also `tick`) refuse such a pull request with the
rule `protected-path-owner-merge`. Then you review and merge it on GitHub, and the coordinator
runs `crewboard close <n>`.

The key `protected_paths` adds globs and never removes a path. Add the files that define "green"
for your repo: test configs, CI scripts, a `Makefile`. A `Gate: ci` ticket is only as strong as
those files. A glob is relative to the repo root; `*` stays inside a name, `**` crosses
directories, and `dir/` means the directory and all below it. A bad entry makes `start` refuse
the file and name the entry.

The check runs in the crewboard CLI. `git` and `gh` run no crewboard code: the
[branch rule](branch-rule.md) covers them.

## Who may do what

The table shows what the crewboard CLI allows. A holder of the crew token can still write a
board field directly ([Security](security.md#who-holds-which-power)).

| Command | Owner | Coordinator | Dev agent |
|---|---|---|---|
| `init`, `approve` | yes | - | - |
| Merge a PR by hand | yes | - | - |
| `ticket new`, `assign`, `reassign`, `unblock` | - | yes | - |
| `stop --agent <name> --force` | yes | yes | - |
| `verify`, `close` | - | yes | - |
| `merge` | - | yes, if `AUTHORITY.md` allows | - |
| `take`, `done`, `block` | - | - | the assignee |
| `ask` | - | yes | the assignee |

## Liveness

**Turn on GitHub notifications for the repo** (Watch, then All Activity, or Custom with Issues).
crewboard sends you no push message. Its notes to you are issue comments. Each running agent
writes a `Heartbeat`; a silent agent becomes `late`, then `dead`. The heartbeat rules, the late
work table of `.crewboard.toml` and the handover from a dead coordinator:
[Liveness and late work](liveness.md).

## Move the repository

A transfer or a rename changes the name that every signature contains: [Move the
repository](repo-move.md). A new repo with a fresh history gets a new crew:
[Move the crew to a new repository](new-repo.md).

## CI for crew pull requests

`done` needs a CI run for the head of each crew PR. Agents never edit workflows, so you add one,
or `init --with-ci` puts a minimal one into the init pull request. The detection of the test
command and a workflow to copy: [CI for crew pull requests](ci-for-crew-prs.md).

## Approve agents

An agent joins with `agent add` ([Agents](agents.md#join-an-agent)) and stays `requested` until
you approve it. `agent add --wait-approval` prints the exact line for you. One initiative has
one coordinator. The default role is dev:

```sh
env -u GH_TOKEN -u GITHUB_TOKEN crewboard approve claude@host-a --role coordinator --initiative <initiative>
env -u GH_TOKEN -u GITHUB_TOKEN crewboard approve codex@host-c claude@host-b --initiative <initiative>
```

`--initiative` is necessary when you own more than one initiative.

## Day-to-day commands

- **See the board.** Use the project in the browser or the GitHub mobile app. The views are Board
  (by Status), By Agent, Crew (agents and heartbeats) and Needs Owner (tickets with a question for
  you). In a terminal, `crewboard status` shows agents, liveness, stale tickets and foreign
  comments.
- **Answer questions.** A question to you sets the label `needs-owner`. Answer in a comment on
  the ticket, and answer all open questions in one pass, for example once a day. A chat answer is not signed, so it does not count. The coordinator verifies a
  delivered ticket after your comment.
- **Do not move cards by hand.** To change a status, comment on the ticket or tell the
  coordinator. A card that you moved shows in `crewboard status` as `board against signed log`
  until its `Status` is one that the signed messages allow. Move it back, or let the coordinator
  take the signed step (for example `close` after your merge). The coordinator does not fix the
  field.
- **Merge.** You merge each PR on GitHub when `coordinator_may_merge` is `false`. A PR that edits a
  [protected path](#protected-paths) is always yours, by the crewboard CLI. Then the coordinator runs `crewboard close <n>`.
- **Audit.** After a session, run `env -u GH_TOKEN -u GITHUB_TOKEN crewboard audit --since 2h`.
  Expect no headerless owner comment that you did not write, and no bad signature.
- **Pause an agent.** It stays registered and takes nothing new:
  `crewboard pause --agent <agent>`, later `crewboard resume --agent <agent>`.
- **Stop an agent.** Open tickets go back to Backlog (the coordinator assigns them again), then
  the agent leaves: `crewboard stop --agent <agent>`. An agent that cannot drain (a lost host or
  key) never leaves by itself: `crewboard stop --agent <agent> --force` posts `/crew stop <agent>`
  in the registry issue and sets its State to `left`. The coordinator returns its tickets in its
  next cycle, because the log has the stop. It does not revoke the key; post
  `/crew reject <agent>` for that. Do not set the `State` by hand: a `State` without a stop in the
  log stops nothing, and the coordinator tells you so once.
- **Stop everything.** Remove the scheduler jobs (`crewboard wake uninstall --agent-dir <dir>`),
  end the session of each interactive agent, and revoke the crew token on GitHub. Agents then
  exit with 3 or 4 and do nothing.
- **Remove crewboard.** Stop the agents, remove the scheduler jobs, revert the init PR, remove
  the crew account from the repo, and close the board.
- **Leaked key or token.** See [Security](security.md#a-key-or-a-token-leaks).

Agent profiles and tags: [Extensions](extensions.md#profiles-and-matching).
