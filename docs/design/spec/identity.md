# Identity

Part of the [crewboard specification](README.md).

## Problem

The GitHub author of a comment does not tell which agent wrote it: several agents can share one
account. It must still separate the owner from the agents, and a contributor who brings an own
agent must be able to join with an own account.

## Accounts

An agent acts as one GitHub account, its **crew account**: a shared crew account of the owner,
or a contributor's own account. The account that writes the signed join comment is the account
of the agent. The owner's approval binds it to the agent together with the key: `crewboard
approve` posts `/crew approve <name> role=<r> key=<fingerprint> item=<id> login=<account>`. An
approval without `login=` binds the account of the join. An approval whose `login=` is not the
account of the join is void. A contributor's account needs the Write role on the repos and on
the Project; the owner adds it as a collaborator.

### Identity modes

`.crewboard.toml` sets the mode with two keys. They never come together (a config error).

| Mode | Keys | Who the agents act as | The owner's account |
|---|---|---|---|
| One-person trial | `insecure_one_account = true` | the owner's account | allowed: any token passes, nothing separates owner from agent |
| Shared crew account | `crew_login = "<account>"` | the shared crew account, or a contributor's own account | refused |
| Own accounts | neither key | each agent its own account | refused |

On both crew modes an agent command refuses the owner's token (`agent-uses-owner-token`), and
the approval of a join that the owner's account wrote is void (`approve` refuses it with
`approve-owner-account`): agents may not act as the owner. Only the board key makes a trial:
`join` or `start` with `--insecure-one-account` records the agent's consent on a trial board,
and changes nothing on a crew board. `init` writes the mode: `--insecure-one-account`,
`--crew-login <account>`, or neither (own accounts).

## Author and signature

| Layer | What it gives |
|---|---|
| **GitHub author** | The owner posts as the owner's account. An agent posts as its crew account: the shared `crew_login` or a contributor's own account (phase 1b, the crew path of [Getting started](../../getting-started.md), shared or own), or with the owner's token (phase 1, the one-person Quick try only). A signed message counts only from the account that the approval bound to its sender. crewboard drops a comment with a crew header whose author is neither the owner nor the account of an approved agent ([Untrusted input](messages.md#untrusted-input)). A join request is the one exception: any account may post it, and the owner decides. Only a headerless comment of the owner account is an owner message ([Owner messages](messages.md#owner-messages)). |
| **Ed25519 key per agent** | Each message carries an SSH signature (`sshsig` format, namespace `crewboard`). `from=hermes@host-b` is proven by the public key registered for that name. |

The signature is made in-process (Rust `ssh-key` crate), so a host needs no
`ssh-keygen`. Signatures can still be checked by hand with
`ssh-keygen -Y verify -n crewboard`.

**The token.** An agent command reads one GitHub token: the `GH_TOKEN` (else `GITHUB_TOKEN`) of the
env file of the agent (`<config-dir>/agent.env`, or `--env-file`), else `GH_TOKEN`, else
`GITHUB_TOKEN` of the environment, else the output of `gh auth token`. In an agent dir the
file wins and the token variables of the caller are dropped; an agent dir with no `agent.env`
and no token variable is refused with `owner-token-in-agent-dir` instead of using the login of
`gh`. It never reads a token from the config file. On a crew board (every mode but the trial),
every agent command refuses the owner's token, and `start` refuses a token of another account than the one
that the registry bound to the agent (phase 1b). See
[Token handling](security-details.md#token-handling).

**The owner token.** An owner command (`init`, `approve`, `audit`, and `pause`, `resume`, `stop`
and `agent profile set` with `--agent`) runs with the owner's own `gh` login. It reads the
`--env-file` of the call, else the output of `gh auth token`; it runs `gh` without the four token
variables. It ignores `GH_TOKEN` and `GITHUB_TOKEN`, and prints one note on standard error that
names a token variable that was set (never its value). So a crew token in the owner's terminal
never decides who an owner command acts as. The owner command then reads `GET /user` and refuses
a login that is not the board owner (`owner-command-not-owner`): the `owner_login` of the board
config, or for `init` the owner that it is about to write. In an agent dir without
`--env-file`, an owner command keeps the order of an agent command (`agent.env`, else a token
variable, else `owner-token-in-agent-dir`) and never runs `gh`: there the login of `gh` is the
owner's, and an agent must not reach it through crewboard. A plain dir has no `agent.env`, so no
agent env file decides an owner token. `stop --force` is an owner command in a config dir
without an agent config; in the config dir of an agent it keeps the order of an agent command. `agent token` reads its token
from standard input and `agent add` the token of the agent: neither is an owner command.

**Not supported: a GitHub App.** A GitHub App would let the agents post as a bot with an App
private key instead of a user token. crewboard has no App support: no App key, no App token,
no keychain.

## Trust at signing time

The registry issue is an ordered log (comment ids rise with time). crewboard replays it per
agent into *spans*. A span is a stretch of the log in which the agent may speak with one role.

| Log entry | Effect on the spans of the agent |
|---|---|
| owner `/crew approve <name> role=<r>` (valid) | A span with role `r` starts. The key and the account of the join are bound. |
| owner `/crew role <name> <r>` | The open span ends. A span with role `r` starts. |
| owner `/crew reject <name>` | The open span ends. |
| signed `info` of the agent in the registry issue with the line `lifecycle: left` | The open span ends. The marker counts itself. |

A message counts when a span covers its comment id (`from < id <= to`), with the role, the
key and the account of the join at that time. Consequences:

- **Handover.** After `/crew role <old> dev`, the earlier messages of the old coordinator
  still count as coordinator messages. A host with an empty cache still finds its tickets,
  and its answers still unblock.
- **Left agent.** `crewboard stop` posts the marker when the agent becomes `left`. Its old
  messages are history and no security event in `status`. A message signed after the marker
  is `unknown-sender`. If the post fails, the agent stays `draining` and the next `stop`
  tries again.
- **Reject.** `/crew reject` is the owner's hard cut-off: the old messages stay valid, the
  new ones are not.
- **`/crew stop` does not end a span.** A draining agent speaks until it leaves. The owner
  command is applied by the agent itself, so a hostile agent can ignore it. It is still a stop in
  the log: the coordinator returns the agent's tickets. Only `/crew reject` cuts.
- **Agents that left before this rule** have no marker, so their span is open. The owner
  runs `/crew reject <name>` for every retired agent name to close it.

The present tense, "may act now", comes from the same log: the agent is approved, its span is
open, and its item is readable on this board. The item `Key` and `State` are mirrors. A value
that contradicts the log is a `trust problem` line in `status`; it does not mute the agent, so
one board field write cannot silence it.

## Phases

- **Phase 1 (one-person trial, owner token):** agents use the owner's `gh` token. This is the
  [Quick try](../../quick-try.md): one person, one host, no crew account. A crew uses phase 1b. The mode
  is explicit: only `init --insecure-one-account` makes it. It writes
  `insecure_one_account = true` to `.crewboard.toml` and prints a `WARNING: one-account board`
  line. The key and `crew_login` together are a config error. `status` prints
  `identity: ONE ACCOUNT (insecure)` and `doctor --setup` uses the same words. Rule: a comment
  by the owner's account *without* a valid crewboard header is an owner message. An
  agent that posts without a header could impersonate the owner. Level: *stated* plus
  *loud* (crewboard never posts without a header, and `crewboard audit` lists every
  headerless comment by the owner account that was posted while an agent session ran).
- **Phase 1b (crew accounts):** a board without `insecure_one_account`. Each agent acts as a
  crew account with write access to the repos and the Write role on the board: the shared
  `crew_login` (for example `crew-bot`) if the board names one, or its own account
  ([Identity modes](#identity-modes)). A comment of an agent's account with a valid header is a message of
  that agent; a headerless comment of such an account is foreign, never an owner message; only
  the owner account writes owner messages. Every agent command checks the token's login
  (`GET /user`): the owner's token is refused (`agent-uses-owner-token`). `start` checks the
  token against the account that the registry bound to the agent (`agent-token-not-crew`).
  Level: *loud* for a host that has no owner token, *impossible* for the agent to post as the
  owner unless it holds the owner's token. A user-owned board needs a classic token with `repo`
  and `project` (fine-grained tokens cannot reach user-owned boards).
Honest limit: an agent on a host where the owner's token is present can always act as
the owner. Run agents with full autonomy only on hosts without that token.

## Agent names

`<runtime>@<host>`, lowercase, `[a-z0-9-]`, at most 40 characters. Examples:
`claude@host-a`, `hermes@host-b`, `codex@host-c`. A second instance on the same host adds a
suffix: `claude-2@host-a`. GitHub `@mentions` do not work for agents, so addressing is in
the header (`to=`) and in a visible first line ("-> hermes@host-b").

## Identity of the agent's own `gh`

crewboard checks its own token ([Where things live](overview.md#where-things-live), `crew_login`). The model also runs plain `gh` and `git`.
Hermes strips `GH_TOKEN` from those commands, so they can fall back to the owner's `gh` login
on the same host. A PR and a comment can be posted as the owner; a headerless
comment of the owner login is an owner message ([Owner messages](messages.md#owner-messages)).

On a board that names neither `crew_login` nor `insecure_one_account` (own accounts), every
agent command, `join` included, refuses the owner's token with `agent-uses-owner-token`, with or
without `--insecure-one-account`. `insecure_one_account` is an identity key: a refresh of the
agent's copy never applies a change of it (`board-identity-changed`). An old trial board
without the key is now an own-accounts board; to keep it a trial, the owner re-runs
`init --insecure-one-account` and every agent re-joins (`join` re-reads the board).

`agent selftest` (and so `start`) runs `gh api user --jq .login` the way the agent's shell runs
it: the first `gh` on the PATH, with the token variables (`GH_TOKEN`, `GITHUB_TOKEN`,
`GH_ENTERPRISE_TOKEN`, `GITHUB_ENTERPRISE_TOKEN`) removed. The agent's account is the account
that the registry bound to it (the author of its join). The one-person trial
(`insecure_one_account`) is not checked.

- the login is the agent's account: pass;
- any other login: refused, rule `agent-gh-not-crew` (exit 2); the refusal names the account;
- the bound account is the owner's on a crew board: refused, `agent-gh-not-crew`;
- no `gh` on the PATH: skipped, with a note;
- `gh` fails (not logged in): a warning only; a bare `gh` that fails is the safe state.

`CREWBOARD_GH_KEEP_TOKEN=1` keeps the token variables, for a runtime whose shell really holds
the token of the agent's account. `CREWBOARD_AGENT_GH=<path>` names another `gh` executable (tests).
`crewboard agent shim` (and `crewboard agent add`) makes the fix: a `gh` stub of three lines that
runs the real `gh` through `crewboard agent with-env`, an empty `GH_CONFIG_DIR`, and the git
settings of the agent's checkout.
