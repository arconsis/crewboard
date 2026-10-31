# Owner guide

You run `init` once, you approve each agent, and you merge what needs your decision. A first
crew: [Getting started](../getting-started.md). Threats and limits: [Security](security.md).

Placeholders: `<initiative>` is a short name such as `my-crew`. `<account>/<repo>` is the
initiative repo, with at least one commit. `<crew-login>` is the optional [shared agent account](#a-shared-agent-account).

## Before you start

- An owner machine and the agent hosts (one laptop can be all of them), with the tools in
  [Install](install.md#what-you-need).
- `gh` logged in as you. `init` checks the `project` scope before it writes anything and prints
  the `gh auth refresh` line if it is missing.
- The runtime and its login on each agent host (`claude` or `codex login`).
- To run agents yourself: a second GitHub account of yours for them, with its own e-mail address
  (a plus address works if your provider delivers it). Agents never act as your account
  ([Agent accounts](#agent-accounts)).
- A CI workflow that runs the tests: `done` needs a CI run for the head of each PR. `init
  --with-ci` adds a minimal one ([CI for crew pull requests](ci-for-crew-prs.md)).
- Organisation repo only: permission to create Projects, and GitHub CLI allowed if the
  organisation restricts OAuth apps.

**Owner commands** (`init`, `approve`, `audit`, and `pause`, `resume`, `stop` and `agent profile
set` with `--agent`) use your own `gh` login or `--env-file`, never `GH_TOKEN` or `GITHUB_TOKEN`.
They refuse any account but the board owner (`owner-command-not-owner`). Run them in your own
terminal, not in an agent's config dir. Token order: [Token handling](../design/spec/security-details.md#token-handling).

## Agent accounts

An agent acts on GitHub as one account, its agent account. Each person who runs agents uses a
GitHub account that they already have. You give that account access to the repos and the
Project in the normal GitHub way ([Give an account access](#give-an-account-access)), and
`approve` binds it to the agent. Several agents may act as the same account.

One rule: no agent acts as your account. Only you merge protected changes and approve agents,
and GitHub sees one login for everything that an agent does. To run agents yourself, use a
second account of yours for them. In both crew modes agent commands refuse your token
(`agent-uses-owner-token`), and `approve` refuses a join of your account
(`approve-owner-account`). Only the [Quick try](../quick-try.md) allows it.

`init` sets one of three identity modes:

| `init` flag | Mode | The agents act as |
|---|---|---|
| neither flag | own accounts (the default) | the account of the person who runs the agent |
| `--crew-login <account>` | one shared account | the account that you name ([A shared agent account](#a-shared-agent-account)); a contributor can still join with an own account |
| `--insecure-one-account` | one-person trial | your account; nothing separates you from the agents ([Quick try](../quick-try.md)) |

A rerun of `init` on an existing board keeps its mode. Without a flag, it keeps the identity keys
of the board config (the local `<initiative>.board.toml` and `.crewboard.toml` on the default
branch) and prints `identity: ... kept`. A flag of the same mode works as on the first run
(`--crew-login` may name another account). A flag of another mode than the board config
names is refused before any write (`init-identity-mode-change`): every approved agent is bound to
its account. To switch the mode on purpose, edit `crew_login` and `insecure_one_account` in both
files by hand (the repo file through a pull request), run `init` again, and let every agent join
again. A board config without either key (own accounts, or a trial board of an old build) takes
the mode of a flag.

The account that writes the signed join is the account of the agent. `approve` binds it to the
agent with the key: the agent's messages count only from that account.

## Give an account access

The person who runs the agent tells you the account. You give it access; the person makes the
token.

1. Add the account as a collaborator with the role Write on each initiative repo (for an
   organisation repo, invite it to the organisation first). The person accepts the invitation
   (from the e-mail, or on the repo page while logged in as that account).
2. Add the account to the Project with the role Write (the project page, Settings, Manage access).
3. The person makes a token with the access in [The agent token](#the-agent-token) and joins
   with `agent add --token-stdin` ([Agents guide](agents.md#join-with-your-own-account)).
4. You approve the agent ([Approve agents](#approve-agents)). `approve` prints the account of the
   join: check that it is the account that the person told you.

Give the account access to the initiative repos only, if you can: a classic token reaches every
repo that the account can reach.

**When a person leaves,** post `/crew reject <name>` for each of their agents in the registry
issue, then remove the access of the account on the repos and on the Project. The reject ends
the key; the removed access ends the account.

## The agent token

The person who runs the agent makes the token as the agent account (Settings, Developer
settings, Personal access tokens):

| Board | Token | Exact access |
|---|---|---|
| User-owned | Tokens (classic) | Scopes `repo` and `project`. Nothing else. A fine-grained token cannot reach a user-owned project. |
| Organisation | Fine-grained tokens | Resource owner: the organisation. Repositories: only the initiative repos. Repository permissions: Contents, Issues and Pull requests read and write; Actions, Checks and Commit statuses read-only. Organisation permissions: Projects read and write. |

Never give the token `workflow` (classic) or Workflows and Administration (fine-grained):
workflow changes are yours. For an organisation board, an organisation owner must allow
fine-grained tokens first, and approve the token if approval is on.

Store the token with `agent token` or with `agent add --token-stdin`
([Agents guide](agents.md#join-an-agent)). Both read standard input, check the account and the
scopes, and write `~/.config/crewboard/<initiative>.env` (mode 600) only for a right token. Run
them in your own terminal, not through an agent. `doctor --setup` checks the token later
([Agents](agents.md#check-the-setup)).

## A shared agent account

Optional. With `init --crew-login <account>`, `agent add` uses that account by default, so all
agents act as one account that you made for them. Use it when the people who run agents have no
GitHub account of their own for this work. A contributor can still join with an own account.

GitHub's terms call such an account a machine account: you make it by hand, accept the terms for
it, and are responsible for what it does. Use it only for the agents.

1. In a private browser window, sign up at `https://github.com/signup` with a second e-mail
   address (a plus address works if your provider delivers it).
2. Give the account access and accept the invitation as that account
   ([Give an account access](#give-an-account-access), steps 1 and 2).
3. Make [the agent token](#the-agent-token) as that account. Store it on each agent host with
   `agent token --crew-login <account>` or `agent add --token-stdin`.

## Init

```sh
# once per initiative (idempotent; --project adopts an existing project)
crewboard init --initiative <initiative> \
  --repo <account>/<repo> [--repo ...] [--crew-login <crew-login>] [--project 15] [--with-ci]
```

`init` makes the board (fields, labels, views sorted by `Order`), the pinned and locked registry
issue, the [branch rule](#branch-rule), `~/.config/crewboard/<initiative>.board.toml` (no
secret) and a pull request with `AUTHORITY.md`, `.crewboard.toml` and, with `--with-ci`, a
minimal CI workflow ([CI](ci-for-crew-prs.md)). It deletes the four built-in project workflows
that write `Status` behind crewboard. It prints the identity mode and a checklist of the web
steps that are left. Then edit `AUTHORITY.md` and merge the init pull request: `start` reads it
from the default branch. Details: [views](../design/spec/board.md#views),
[built-in workflows](../design/spec/board.md#built-in-project-workflows),
[registry issue](../design/spec/board.md#registry-issue).

If `init` printed `registry lock: NOT locked`, or the board is older than this release, lock the
registry issue yourself (Lock conversation, on the issue page). An agent that joins before the
merge needs a copy of the board file on its host.

**The queue.** The position of a ticket is its `Order` field (`crewboard status --queue`). To
move a ticket to another agent or position, run `crewboard assign <n> --to <agent> [--order N]`.
`assign` and `reassign` also put a label `agent: <name>` on the issue. Do not drag the card or
edit `Agent` by hand: the new agent's `take` is then refused with `ticket-agent-not-signed`
([Concepts](../concepts.md#done-without-proof-is-detected)).

## Branch rule

`init` adds a repository ruleset to the default branch of the main repo. It requires a pull
request, so an agent cannot push a change of `AUTHORITY.md` or `.crewboard.toml` directly. What
the rule holds, what `init` does when GitHub refuses, and `--no-branch-rule`:
[Branch rule](branch-rule.md).

## AUTHORITY.md

Edit the file on the branch `crewboard/init-<initiative>` of the init pull request. The front
matter has these keys:

| Key | Default | Meaning |
|---|---|---|
| `coordinator_may_merge` | `false` | `false`: only you merge. |
| `coordinator_may_create_tickets`, `coordinator_may_split_tickets`, `dev_may_open_prs` | `true` | Limits for `assign`, `ticket split` and `done`. |
| `dev_branch_prefix` | `crew/` | The branch prefix of the PRs of dev agents. |
| `protected` | `[main]` | Branches. A PR from a protected branch is no evidence. |
| `protected_paths` | none | More paths that only you merge, as globs ([Protected paths](#protected-paths)). |
| `ci_ignore` | none | CI jobs that the CI check skips ([details](../design/spec/ci.md#jobs-that-ci_ignore-skips)). |
| `ci_mode` | `ci` | `local` when CI cannot run ([Local run](../design/spec/ci.md#local-run-ci_mode-local)). |
| `former_repos` | none | After a repo transfer or rename: the old name and a cutoff ([Move the repository](repo-move.md)). |
| `needs_owner` | a list | Advice for the models: when to ask you. Not enforced, with one exception: `merge` in the list lets only you merge (rule `authority-needs-owner`, deprecated: use `coordinator_may_merge: false`). |

Below the front matter, write the goal and the scope in plain words. The agents read it.

The toolchain keys `required_tools`, `smoke_cmd` and `detect_tools` are in `.crewboard.toml`
([Tools per agent tag](agents.md#tools-per-agent-tag)).

## Protected paths

The crewboard CLI lets only you merge a pull request that changes one of seven paths, whatever
the keys say: the authority file (`AUTHORITY.md` by default) and `.crewboard.toml` in the repo
root, `.github/CODEOWNERS`, `CODEOWNERS`, `docs/CODEOWNERS`, and everything under
`.github/workflows/` and `.github/actions/`. The code fixes the list, and `AUTHORITY.md` cannot
shorten it. A rename out of a protected path counts.

`verify`, `close --verified` and `merge` (so also `tick`) refuse such a pull request with the
rule `protected-path-owner-merge`. You review and merge it on GitHub, then the coordinator runs
`crewboard close <n>`.

The key `protected_paths` adds globs: add the files that define "green" for your repo (test
configs, CI scripts, a `Makefile`). A `Gate: ci` ticket is only as strong as those files. A glob
is relative to the repo root: `*` stays inside a name, `**` crosses directories, `dir/` means the
directory and all below it. `start` refuses a bad entry and names it. `git` and `gh` run no
crewboard code: the [branch rule](branch-rule.md) covers them.

## Who may do what

The table shows what the crewboard CLI allows. The holder of any agent token can still write a
board field directly ([Security](security.md#residual-risks)).

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

## Approve agents

An agent joins with `agent add` ([Agents](agents.md#join-an-agent)) and stays `requested` until
you approve it. `agent add --wait-approval` prints the exact line for you. One initiative has
one coordinator. The default role is dev:

```sh
crewboard approve claude@host-a --role coordinator --initiative <initiative>
crewboard approve codex@host-c claude@host-b --initiative <initiative>
```

`--initiative` is necessary when you own more than one initiative. `approve` prints the account
of each join and writes it into the approval (`login=<account>`). Compare it with the account
that the agent's owner told you.

## Day-to-day commands

- **See the board** in the browser or the GitHub mobile app: the views Board, By Agent, Crew
  (agents and heartbeats) and Needs Owner. In a terminal, `crewboard status` shows agents,
  liveness, stale tickets and foreign comments.
- **Answer questions.** A question to you sets the label `needs-owner`. Answer in a comment on
  the ticket, all open questions in one pass. A chat answer is not signed and does not count.
- **Move a ticket with the command, not with a drag.** Run
  `crewboard move <n> <status> --why "<reason>"`, for example to send a Done or Dropped ticket
  back to Ready.
  It posts `/crew move <status>` on the ticket and then writes the board, so the move is in the
  signed log. A card that you drag shows in `crewboard status` as `board against signed log`;
  run the same command afterwards to record it. `move` never writes Done or Dropped. To end a
  ticket, merge its PR, and the coordinator runs `crewboard close <n>` (Done). For a ticket that
  ends without a merge, ask the coordinator to drop it: `crewboard close <n> --why "<reason>"`
  (Dropped) ([Owner move](../design/spec/tickets.md#owner-move)).
- **Merge.** You merge each PR on GitHub when `coordinator_may_merge` is `false`, and always a PR
  that edits a [protected path](#protected-paths). Then the coordinator runs `crewboard close <n>`.
  Only your merge or a verified merge counts: a PR that the agent account merged without a verify
  gives no Done, and the coordinator can only drop that ticket.
- **Audit.** After a session, run `crewboard audit --since 2h`. Expect no headerless owner
  comment that you did not write, and no bad signature.
- **Pause an agent.** It stays registered and takes nothing new: `crewboard pause --agent
  <agent>`, later `crewboard resume --agent <agent>`.
- **Stop an agent.** `crewboard stop --agent <agent>`: its open tickets go back to Backlog, then
  the agent leaves. For an agent that cannot drain (a lost host or key), add `--force`. It does
  not revoke the key: post `/crew reject <agent>` for that ([lifecycle](../design/spec/board.md#lifecycle)).
  Do not set the `State` by hand: without a stop in the log it stops nothing.
- **Stop everything.** Remove the scheduler jobs (`crewboard wake uninstall --agent-dir <dir>`),
  end the session of each interactive agent, and revoke the agent tokens on GitHub (ask each
  contributor, or remove the access of the account). Agents then exit with 3 or 4 and do nothing.
- **Remove crewboard.** Stop everything, revert the init PR, remove every agent account from the
  repo, and close the board.
- **Leaked key or token.** See [Security](security.md#a-key-or-a-token-leaks).

Agent profiles and tags: [Extensions](extensions.md#profiles-and-matching).

## Liveness

**Turn on GitHub notifications for the repo** (Watch, then All Activity, or Custom with Issues).
crewboard sends no push message: its notes to you are issue comments. Heartbeats, late work and
the handover from a dead coordinator: [Liveness and late work](liveness.md).

## Add a repository

An initiative can span several repos of the same owner, for example an app and its website. A
pull request is evidence only in a repo of the initiative (`evidence-pr-foreign-repo`), so add
the repo before a ticket delivers there:

1. In a pull request, add the repo to `repos` in `.crewboard.toml` of the main repo. Keep the main
   repo first. The file is a protected path, so you merge the pull request.
2. Add the same entry to `repos` in your board file
   `~/.config/crewboard/<initiative>.board.toml`. An `init` rerun leaves an existing
   `.crewboard.toml` as it is.
3. Give the agent tokens access to the repo: the shared agent account, each contributor's account,
   and a fine-grained token's repository list.
4. The agents read the new list at their next `start`.

## Move the repository

A transfer or a rename changes the name that every signature contains: [Move the
repository](repo-move.md). A new repo with a fresh history gets a new crew:
[Move the crew to a new repository](new-repo.md).

## CI for crew pull requests

Agents never edit workflows, so you add one, or `init --with-ci` puts a minimal one into the init
pull request. The detection of the test command and a workflow to copy:
[CI for crew pull requests](ci-for-crew-prs.md).
