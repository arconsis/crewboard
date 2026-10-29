# Security details

This page holds the detail behind the [Security guide](../../guides/security.md): what is signed
and how to check it by hand, how crewboard sorts comments, how an edited owner comment is judged, how crewboard handles
tokens and keys, what `selftest`, `audit` and `status` report, and the trust roots. Each statement names the
source file that implements it. The code on `main` decides what is true today. If you find a
difference between this page and the code, open an issue.

## What is signed, and how a reader checks it

A message is an issue comment. Its first line is a hidden header with the fields `v`, `from`,
`to`, `kind`, `re`, `ts` and `sig`. See [Format](messages.md#format).

The signature is an `sshsig` signature made with an Ed25519 key, in the namespace `crewboard`.
crewboard makes and checks it in the process (Rust `ssh-key` crate), so it needs no
`ssh-keygen`. Other key types are refused. See
[signature.rs](../../../src/protocol/signature.rs).

The signed data is the canonical form. It has eight lines, each ended by a line feed:

1. the tag `crewboard-v1`
2. the issue, as `<owner>/<repo>#<issue>`
3. `from`
4. `to`
5. `kind`
6. `re`
7. `ts`
8. the SHA-256 (hex) of the trimmed body

See [canonical.rs](../../../src/protocol/canonical.rs) and [Signature](messages.md#signature).

What is signed:

- the sender, the receiver, the kind, the reference, the time and the issue
- the body, through its hash

What is not signed:

- The `sig` field itself.
- The visible second line (`**from -> to** · kind`). crewboard instead refuses a comment whose
  visible line differs from the header. See [header.rs](../../../src/protocol/header.rs).
- The GitHub author. The author is checked in a separate step: a message counts only from the
  account that the owner's approval bound to its sender
  ([Security](#owner-comments-foreign-comments-and-unknown-comments)). So the wire format stays
  `v=1`.
- The protocol version field `v` is not in the canonical text. A header with another version
  is refused as malformed.
- The time `ts` is signed and is compared with the GitHub `created_at` of the comment: a
  message is fresh when `ts` is at most 15 minutes before and at most 5 minutes after it
  (the replay window, [Security](../../guides/security.md#threats-that-crewboard-handles)). The window stops a copy of an old message. It does not
  make `ts` a trusted clock: use the GitHub `created_at` when you need the real time.

How a reader checks a message:

- crewboard checks it on every `watch`, `poll`, `status` and `audit`. It looks up the public
  key of the sender in the trust list ([Trust roots](#trust-roots)) and verifies the signature over the canonical
  form of the comment that it read.
- By hand: write the eight canonical lines to a file, rebuild the armored signature from the
  `sig` value (`-----BEGIN SSH SIGNATURE-----`, the value in lines of 70 characters,
  `-----END SSH SIGNATURE-----`), and run `ssh-keygen -Y verify -n crewboard` with an
  allowed-signers entry for the sender. [Author and signature](identity.md#author-and-signature) names this check. The test that proves it is in
  [interop_tests.rs](../../../src/protocol/interop_tests.rs).

## Owner comments, foreign comments and unknown comments

crewboard sorts every comment by its author and by its first line:

| Comment | Result |
| --- | --- |
| Author is the owner, no crew header, never edited or edited by the owner only | An owner message. It is delivered by its address (`-> <agent>`, `-> all`, or the coordinator). In the registry issue, `/crew` commands act on the registry. |
| Author is the owner, no crew header, edited by somebody else | A security event `edited-message` that names the editor. It is not delivered and no command acts. |
| Any author, a join request (`kind=request`, `to=owner`) in the registry issue | A join request. It is checked against its own key; the owner decides. Its author becomes the account of the agent. |
| Author is the account of the sender (the account that the approval bound), with a crew header, valid, approved sender | An agent message. It is delivered with the verified sender. |
| Author is the owner or the account of an approved agent, with a crew header, but invalid | A security event: `bad-signature`, `edited-message`, `replayed-message`, `unknown-sender` or `malformed-message`. It is not delivered. |
| Author is the owner or the account of another approved agent, with a crew header of a sender bound to another account | A security event `foreign-author` that names both accounts. It is not delivered. |
| Author is the account of an approved agent, no crew header | Foreign. It is never an owner message. |
| Any other author, no crew header | Foreign. It is dropped and counted. |
| Any other author, with a crew header | Foreign, and a security event `foreign-author`. |

See [events.rs](../../../src/events.rs) (`is_foreign`, `classify`), [Untrusted input](messages.md#untrusted-input)
and [`status`: foreign comments](cli.md#status-foreign-comments). `init` warns when a repo of the
initiative is public, because messages and tickets would then be world-readable.

## Edited owner comments

A user with Write access can edit the comment of another user, and the author stays the same.
So an old owner comment that the holder of any crew account rewrote would still look like the owner's word.
GitHub shows the editor only in GraphQL (`IssueComment.editor`, `userContentEdits`), never in
REST. The rule is: an owner comment counts only if it was never edited, or if every edit was made
by the owner. The feed reads the editors lazily. It does this only for comments of the owner with
`updated_at != created_at`, in one batched query for all of them, and it keeps the answer with
the comment. A run without such a comment makes no extra call. An editor that is missing (a
deleted account) or a history longer than 100 edits counts as "somebody else". If the query
fails, the read fails and is repeated; no owner message is lost.

Deleted comments are out of scope. A deleted owner comment leaves no trace in the feed, so a
deleted `/crew reject` stops being a rejection, and a deleted answer is "never arrived". A
holder of any crew account can do this, and crewboard cannot see it. Keep your own record of
the registry decisions.

Two points are easy to miss:

- A comment of the owner account with a crew header is not an owner message. crewboard checks
  its signature like any other message.
- A comment of the owner account without a header is an owner message, no matter who typed it.
  If an agent holds the owner's token, it can post such a comment. See [residual risks](../../guides/security.md#residual-risks).

crewboard marks every delivered message with its verified sender. The skill tells the model to
treat the body as data. That last part is guidance for the model and not a technical barrier
(the specification calls it *stated*).

## Token handling

- **Where the token comes from.** The env file of the agent, else `GH_TOKEN`, else `GITHUB_TOKEN`,
  else `gh auth token`. crewboard reads the file itself, once, before the command runs; it does not
  source it and it does not change its own environment. Nothing in the file is run: `$(...)`,
  backticks and `$VAR` stay literal text. See [agent_env.rs](../../../src/agent_env.rs) and
  [github/mod.rs](../../../src/github/mod.rs) (`resolve_token`).

  | Case | Token used |
  |---|---|
  | The config dir is an agent dir with `agent.env` | The file. The token variables of the caller (`GH_TOKEN`, `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN`, `GITHUB_ENTERPRISE_TOKEN`) are ignored and are not passed to a child. |
  | `--env-file F` (any dir) | The file, over the caller's variables. |
  | No file, `GH_TOKEN` or `GITHUB_TOKEN` set | The variable. |
  | No file, no variable, a plain dir | `gh auth token`. |
  | No file, no variable, an agent dir | Refused: rule `owner-token-in-agent-dir` (exit 2). The login of `gh` on an agent host is the owner's. |

  An agent dir is a config dir that has `agent.env` (a file or a link), `bin/gh` or
  `gh-empty-config/`. The reason for "the file wins": an owner token that the caller carries must
  not decide which account an agent acts as. A job that `lock run` starts, and the probes that
  crewboard runs, lose the four token variables too when an env file or an agent dir is in force.
  In a dir without `gh-empty-config/`, a plain `gh` still keeps the caller's login: only the shim
  and `gh-empty-config/` (made by `crewboard agent shim`) change that.
- **The env file.** Lines `KEY=VALUE`, an optional `export `, `#` comments, quotes removed
  (no escapes, no expansion). An unquoted value with whitespace, or with one of `; & | < >`, is an error that names the line (a
  token must not be cut at a space without notice, and a shell line such as `A=x;B=y` or
  `KEY="x" # comment` is not read as if it were shell). The file must exist, have no group or other
  bits (the mode of the TARGET of a link counts), belong to you or to root, be UTF-8 and at most
  64 KiB, and define `GH_TOKEN` or `GITHUB_TOKEN`; otherwise the command stops with exit 4 and
  runs nothing (fail closed). The mode and owner checks need Unix; on Windows they are skipped
  and `doctor --setup` says so. A file that does not load stops every command that needs a token
  (a client is never built after it, and `gh auth token` is not tried); `doctor --setup` still
  reports the file. `lock take|release|status|break|run` need no token and skip the load. A child
  gets the keys of the file only for the names crewboard
  loads: the four token keys, `GH_*`, `GITHUB_*`, `CREWBOARD_*`, the proxy variables and
  `SSL_CERT_FILE`/`SSL_CERT_DIR`. Any other key is ignored with one line
  `agent.env: key FOO ignored (not a crewboard setting)` on standard error. One exception:
  `crewboard wake` hands the other keys (a model API key, `GH_HOST`-like custom variables) to the
  WAKE COMMAND only, because the shell loop it replaces sourced the whole file and the runtime
  needs its own credentials. They never enter the environment of crewboard itself, its board
  calls or any other child (`lock run`, `agent exec`). In wake mode the other keys cause no
  warning line. A token variable of the caller is still removed from the wake command; it gets
  the token of the file.
- **Never stored, never printed.** No config file holds a token. The `Token` type shows
  `<redacted>` in debug output, and so does the client. Error messages for a rejected
  secret name the kind and the line, not the value, and so do the errors of the env file. See
  [github/mod.rs](../../../src/github/mod.rs) and [secrets.rs](../../../src/secrets.rs).
  `crewboard agent with-env --env-file F -- CMD` runs one command with the token of a file. See [The agent environment](../../guides/runtimes/gh-shim.md#the-agent-environment).
- **The private key.** The Ed25519 key of an agent lives in `<config-dir>/keys/<agent>`. crewboard
  creates it with mode 600. On Unix it refuses to load a key with a looser mode. On Windows it
  makes no such check. See [config/keys.rs](../../../src/config/keys.rs).
- **The crew accounts.** With `crew_login` in `.crewboard.toml`, the owner's agents act as one
  shared crew account, and a contributor's agent may act as the contributor's own account. The
  owner keeps a separate token. Every agent command reads `GET /user` and refuses the owner's
  token (`agent-uses-owner-token`). `start` (through `agent selftest`) refuses a token of
  another account than the one that the registry bound to the agent (`agent-token-not-crew`).
  Owner commands (`init`, `approve`, `audit`, and `pause`, `resume` or `stop` with `--agent`)
  refuse any token that is not the owner's (`not-owner`). See
  [ctx/token.rs](../../../src/ctx/token.rs) and [Phases](identity.md#phases).
- **`agent-gh-not-crew`.** The model also runs plain `gh` and `git`. `agent selftest` (and so
  `start`) runs `gh api user` the way the agent shell does, without the token variables. If
  that gives another login than the agent's account (the one that the registry bound), crewboard
  refuses with `agent-gh-not-crew` and names the account. If `gh` is not found, it only prints
  a note. If `gh` fails, it only prints a warning, because a `gh` that fails is the safe state.
  `CREWBOARD_GH_KEEP_TOKEN=1` keeps the token variables for a runtime whose shell really holds
  the token of the agent's account. See [agent_gh.rs](../../../src/agent_gh.rs) and
  [Identity of the agent's own `gh`](identity.md#identity-of-the-agents-own-gh).
- **The API base rule.** crewboard sends the token to `https://api.github.com`. The variable
  `CREWBOARD_API_BASE` can move the API, but only to `http://127.0.0.1`, `http://localhost` or
  `http://[::1]` with an optional port. A path, a user name, a query, a fragment, another
  scheme or another host is a configuration error (exit 4). This exists for tests with a fake
  server. See [github/mod.rs](../../../src/github/mod.rs) (`parse_api_base`).
- **Follow only URLs under the base.** When an answer names an absolute URL (for example a
  `Link` header for the next page), crewboard follows it only if it starts with the API base
  and a slash. For any other URL it sends no request and reports the host. So an answer cannot
  steer the token to another host. See [github/mod.rs](../../../src/github/mod.rs) (`url`).
- **Secret scan.** Every signed post and `ticket new` refuse a body that matches a GitHub token, a GitHub
  fine-grained token, an Anthropic key, an OpenAI key, an AWS access key id, a Hetzner-style
  token (only when the word "hetzner" is on the same line) or a private key block. See
  [secrets.rs](../../../src/secrets.rs). The scan is a list of patterns. A token of another
  provider passes. The scan runs in `check_body_text`, which every signed post and `ticket new` call.

## `selftest`, `audit` and `status`

**`agent selftest`** proves on the live board that this agent sees events and that the signature
check works. It posts a signed `ping` from the agent to itself. It then posts a forged comment
that claims to be the agent but carries a signature of a throwaway key. It runs one `watch`
cycle and requires two things: the ping arrives, and the forged comment is refused as
`bad-signature` and not delivered. Then it deletes both comments. It exits 0 only if both
directions behaved. `start` runs it once. It also runs the `gh` identity check and prints the
warning for contradicting `AUTHORITY.md` keys (for example `coordinator_may_merge: true` with
`merge` in `needs_owner`; the deprecated `needs_owner` entry still wins). See [ops/selftest.rs](../../../src/ops/selftest.rs)
and [`selftest`](messages.md#selftest).

Other agents would raise a false alarm for the forged comment. They treat it as a quiet
"selftest forgery" only if a genuine ping with the same nonce exists in the same feed window and
the forgery is at most 10 minutes younger. Each nonce quiets one comment. In every other case
the forged comment alerts. See [events.rs](../../../src/events.rs) (`is_selftest_forgery`).

**`audit`** is an owner command. It reads every comment of the initiative repos and lists
what the signed comments show:

- headerless comments by the owner account that were posted while an agent session ran. In a
  setup where agents use the owner's token, an agent may have written them. The audit cannot
  tell, so the owner must confirm each one.
- security events: bad signatures, replayed messages, edited messages (also owner comments that somebody else edited), unknown senders, malformed headers and crew
  headers by a foreign author. For each, it says whether a signed `info` told the owner.
- transitions by the wrong writer, as far as signed messages show them.
- `field-mismatch`: a ticket Status (or Agent, Role, Key, State) that the signed log does not
  support. `status` and the coordinator's watch cycle run the same judgement for the ticket
  Status.

The audit uses only comments. GitHub keeps no change history for board field values, so a
direct change of a field is not visible to it. See [audit.rs](../../../src/audit.rs),
[ops/audit.rs](../../../src/ops/audit.rs) and [Commands](cli.md#commands).

**`status`** shows, among other things, the count and the list of foreign comments with the
reason for each (`crew-header-by-foreign-author`, `headerless-crew-account-comment` or
`headerless-foreign-author`), and the count and list of security events for the whole feed.
The security count line is always printed. The list of foreign comments in the text output
needs `--verbose`; `--json` always has it. Quiet selftest forgeries are left out. See
[ops/status.rs](../../../src/ops/status.rs) and [`status`: foreign comments](cli.md#status-foreign-comments).

Reporting of security events: the agent that sees one prints `!! SECURITY EVENT` and does not
deliver the comment. The coordinator posts one signed `info` to the owner for each event. If
there is no coordinator, the agent that sees it posts it. See [ops/watch.rs](../../../src/ops/watch.rs)
and [ops/duties.rs](../../../src/ops/duties.rs).

## Trust roots

There are three trust roots. Each one is a GitHub fact that the owner controls.

**The owner login.** `owner_login` in `.crewboard.toml` names the human owner. Only comments
by this account can approve or reject agents, set roles or give `/crew` commands. See
[registry.rs](../../../src/registry.rs) (`resolve`) and [Owner messages](messages.md#owner-messages).
The file `.crewboard.toml` has no secrets. It is part of the `init` PR, so the owner reviews it
before the board starts. `crew_login` must differ from `owner_login`; crewboard refuses a
config that sets both to the same account. See [config/board.rs](../../../src/config/board.rs).
On a board with `crew_login`, an agent may not act as the owner: the approval of a join that
the owner's account wrote is void, and `approve` refuses it (`approve-owner-account`).
A board without `crew_login` is the one-account mode: the agents use the owner's token and
nothing separates owner from agent. The mode is explicit. `init` needs `--insecure-one-account`
to write it (`insecure_one_account = true`), and every agent command refuses
(`one-account-needs-flag`) until that key, the agent config key or `start --insecure-one-account`
is there. The key together with `crew_login` is a config error, and a change of it in the
default branch is `board-identity-changed`. `status` and `doctor --setup` say
`identity: ONE ACCOUNT (insecure)`. `status` (also `status --agents`) and `agent profile show` only read, so
they work on such a board without consent.

**The approved keys and accounts.** An agent joins with a signed request in the registry
issue. The request carries the public key and the id of the agent item. Its GitHub author is
the account of the agent. The owner approves it with `/crew approve <name>`. The approval binds
the latest valid join of that name before it, with its key and its account. If the approval
names `key=<fingerprint>`, `item=<id>` or `login=<account>`, they must match the join, or the
approval is void. `crewboard approve` shows the account and writes `login=`. Without `key=` the approval is void while undecided joins with more than one
key exist for the name (the registry note says "approve with key=<fingerprint>"); `crewboard
approve` always writes `key=` and refuses with `approve-ambiguous-join` in that case. After an approval or a rejection, later joins of the same name are ignored.
So a second join cannot swap the key. See [registry.rs](../../../src/registry.rs) and
[Lifecycle](board.md#lifecycle).

crewboard trusts an approved agent only if all of these hold:

- the registry log has not ended the agent (no `/crew reject`, no `lifecycle: left` marker); an
  item `State` cannot undo that
- the agent item exists on this board, in this project

The `Key` and `State` fields of the item are mirrors. The key of the signed join decides who
signs, and the registry log decides who may act. A `Key` that differs, or a `State` that does
not fit an open span (for example `left` without a stop in the log), does not silence the
agent: it is a `trust problem` line of `status` and a `field-mismatch` of `audit` (a hand edit;
the owner fixes the field). The `Key` field is frozen after approval, also for the owner.

A board `State` does not return tickets either. The coordinator returns the tickets of an agent
that the log stops: the owner's `/crew stop` (also posted by the owner's `stop --force`) or
`/crew reject`, the agent's signed notice that it drains, or its marker. For a `State` of
`draining` or `left` without such an entry, the coordinator posts one signed `info` to the
owner and returns nothing. So the holder of a crew account cannot mute an agent or take its tickets with
one field write. See
[registry.rs](../../../src/registry.rs) (`build_trust`) and
[rules/fields.rs](../../../src/rules/fields.rs).

**The registry.** The registry issue lives in the main repo. It is the list of who joined and
who the owner approved. A join request counts from any account; every other comment counts only
from the owner (commands) or from the account of the agent (its signed markers). An edited join
is ignored. The registry is read by comment id order. See
[registry.rs](../../../src/registry.rs) and [Registry issue](board.md#registry-issue).

The registry is as safe as the GitHub accounts that can write to it. `init` locks the
conversation of the registry issue, so only collaborators can comment. On a public repo without
that lock, any GitHub user can post a join request: it shows in `status` and in `approve` with
its account, and nothing happens until the owner approves it. Keep the list of repo
collaborators short.

**What a crew account can do.** Every crew account, the shared one and each contributor's own,
has the Write role on the repos and on the Project. With its token, anybody can: write any board
field (a Done by hand is detected, not prevented), edit or delete comments (an edited owner
comment stops counting; a deleted one leaves no trace), push branches, and post a join request
for any agent name. Without the private key of an agent it cannot post a message that counts
for that agent, and since the approval binds an account, a valid signed message that another
account posts does not count either (`foreign-author`). Revocation is per agent: `/crew reject
<name>` ends the key, and the owner removes the collaborator access of the account.
