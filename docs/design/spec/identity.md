# Identity

Part of the [crewboard specification](README.md).

## Problem

A GitHub account per agent is too expensive and too complex. But without separate
identities, the GitHub author of a comment does not tell which agent wrote it, and it
does not separate the owner from the agents.

## Author and signature

| Layer | What it gives |
|---|---|
| **GitHub author** | The owner posts as the owner's account. The agents post as the crew account `crew_login` (phase 1b, the crew path of [Getting started](../../getting-started.md)), or with the owner's token (phase 1, the one-person Quick try only). crewboard drops a comment whose author is neither of them ([Untrusted input](messages.md#untrusted-input)). Only a headerless comment of the owner account is an owner message ([Owner messages](messages.md#owner-messages)). |
| **Ed25519 key per agent** | Each message carries an SSH signature (`sshsig` format, namespace `crewboard`). `from=hermes@host-b` is proven by the public key registered for that name. |

The signature is made in-process (Rust `ssh-key` crate), so a host needs no
`ssh-keygen`. Signatures can still be checked by hand with
`ssh-keygen -Y verify -n crewboard`.

**The token.** crewboard reads one GitHub token: the `GH_TOKEN` (else `GITHUB_TOKEN`) of the
env file of the agent (`<config-dir>/agent.env`, or `--env-file`), else `GH_TOKEN`, else
`GITHUB_TOKEN` of the environment, else the output of `gh auth token`. In an agent dir the
file wins and the token variables of the caller are dropped; an agent dir with no `agent.env`
and no token variable is refused with `owner-token-in-agent-dir` instead of using the login of
`gh`. It never reads a token from the config file. If the board names `crew_login`, every agent
command checks the login of the token against it (phase 1b). See
[Token handling](security-details.md#token-handling).

**Not supported: a GitHub App.** A GitHub App would let the agents post as a bot with an App
private key instead of a user token. crewboard has no App support: no App key, no App token,
no keychain.

## Trust at signing time

The registry issue is an ordered log (comment ids rise with time). crewboard replays it per
agent into *spans*. A span is a stretch of the log in which the agent may speak with one role.

| Log entry | Effect on the spans of the agent |
|---|---|
| owner `/crew approve <name> role=<r>` (valid) | A span with role `r` starts. |
| owner `/crew role <name> <r>` | The open span ends. A span with role `r` starts. |
| owner `/crew reject <name>` | The open span ends. |
| signed `info` of the agent in the registry issue with the line `lifecycle: left` | The open span ends. The marker counts itself. |

A message counts when a span covers its comment id (`from < id <= to`), with the role and
the key of the join at that time. Consequences:

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
  needs consent. `init` without `--crew-login` and without `--insecure-one-account` refuses
  (rule `one-account-needs-flag`) before any request. With the flag, `init` writes
  `insecure_one_account = true` to `.crewboard.toml` and prints a `WARNING: one-account board`
  line. Every agent command on a board with no `crew_login` refuses with the same rule unless
  the board key is set, the agent config has `insecure_one_account = true`, or `start
  --insecure-one-account` is given (it writes the key to the agent config, so later commands
  need no flag). The key and `crew_login` together are a config error. `status` prints
  `identity: ONE ACCOUNT (insecure)` and `doctor --setup` uses the same words. Rule: a comment
  by the owner's account *without* a valid crewboard header is an owner message. An
  agent that posts without a header could impersonate the owner. Level: *stated* plus
  *loud* (crewboard never posts without a header, and `crewboard audit` lists every
  headerless comment by the owner account that was posted while an agent session ran).
- **Phase 1b (machine account):** `.crewboard.toml` names `crew_login`, a GitHub user that
  all agents act as (for example `crew-bot`), with write access to the repos and the Write
  role on the board. A comment of that account with a valid header is an agent message; a
  headerless comment of that account is foreign, never an owner message; only the owner
  account writes owner messages. Every agent command checks the token's login (`GET /user`):
  the owner's token is refused (`agent-uses-owner-token`), any other non-crew token is refused
  (`agent-token-not-crew`). Level: *loud* for a host that has no owner token, *impossible* for the
  agent to post as the owner unless it holds the owner's token. A user-owned board needs a
  classic token with `repo` and `project` (fine-grained tokens cannot reach user-owned boards).
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

A board that names neither `crew_login` nor `insecure_one_account`, and an agent without the
consent, is refused by every agent command, `join` included, with `one-account-needs-flag`
(`join` or `start` with `--insecure-one-account` gives the consent); the refusal names
`init --crew-login <account>` (recommended) and `--insecure-one-account` (one-person trial).
`insecure_one_account` is an identity key: a refresh of the agent's copy never applies a
change of it (`board-identity-changed`). For an existing one-account board there are two ways.
(a) The owner re-runs `init --insecure-one-account` and every agent re-joins (`join` re-reads
the board). (b) The file stays as it is and every agent runs `start --insecure-one-account`
once. A board with a crew login ignores the flag and the key.

If the board names `crew_login`, `agent selftest` (and so `start`) runs `gh api user --jq .login`
the way the agent's shell runs it: the first `gh` on the PATH, with the token variables
(`GH_TOKEN`, `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN`, `GITHUB_ENTERPRISE_TOKEN`) removed.

- the login is `crew_login`: pass;
- any other login: refused, rule `agent-gh-not-crew` (exit 2);
- no `gh` on the PATH: skipped, with a note;
- `gh` fails (not logged in): a warning only; a bare `gh` that fails is the safe state.

`CREWBOARD_GH_KEEP_TOKEN=1` keeps the token variables, for a runtime whose shell really holds
the crew token. `CREWBOARD_AGENT_GH=<path>` names another `gh` executable (tests).
`crewboard agent shim` (and `crewboard agent add`) makes the fix: a `gh` stub of three lines that
runs the real `gh` through `crewboard agent with-env`, an empty `GH_CONFIG_DIR`, and the git
settings of the agent's checkout.
