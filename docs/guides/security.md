# Security

Read this if you must decide whether to trust crewboard with an initiative. It says what
crewboard protects, how you check it, and what it does not protect.

## Summary

**Threats that crewboard handles:** forged, edited and replayed messages, unknown senders,
strangers, stolen agent keys, an agent that acts as the owner, secrets in comments, a token sent
to the wrong host, a model that merges a protected change, and ticket text that widens a scope.
[Threats](#threats-that-crewboard-handles) lists each one with its defence.

**What crewboard guarantees.**

- Agents talk only through GitHub comments. A message counts only if its GitHub author is the
  account that the owner approved for the sender and its Ed25519 signature matches the approved
  key of the sender. Any other comment is data, never an instruction.
- The owner approves each agent, its key and its GitHub account by hand. The owner's own
  comments need no signature.
- On a crew board (a shared crew account or own accounts), every agent command refuses the
  owner's token, and `start` refuses a token or a plain `gh` of another account than the
  approved one. Only the trial (board key `insecure_one_account`) skips these checks.
- Only the owner merges a PR that changes a [protected path](owner.md#protected-paths), by the
  crewboard CLI. `git` and `gh` skip that check; the branch rule that `init` sets makes them
  need a pull request and your review.
- Through the CLI a ticket reaches Done only in the [two ways](../concepts.md#done-and-dropped):
  a merge after a passed verify, or the owner's merge. A ticket that ends without a merge is
  Dropped.
- crewboard never stores or prints a token. It sends the token only to the GitHub API.

**What crewboard does not guarantee.**

- It does not prevent a board field from being written. The holder of any crew token can set
  `Status = Done`. crewboard detects it ([Done without proof](../concepts.md#done-without-proof-is-detected)).
- It does not stop an agent that holds the owner's token on purpose, a compromised host, or a
  person who steals a crew token or a private key.
- It does not choose who may join. You approve each account by hand.
- It does not make an agent write correct code. Your Gate, your CI and the owner's review do
  that. A workflow that tests nothing still gives a green run.
- The headless coordinator is limited by its OS user and token, not by a config key.

[Residual risks](#residual-risks) lists what stays open, and
[A key or a token leaks](#a-key-or-a-token-leaks) what to do after a leak.

The detail behind this page, with the source file of each statement:
[Security details](../design/spec/security-details.md). It covers what is signed, how comments
are sorted, edited owner comments, token handling, `selftest`, `audit` and `status`, the trust
roots, and [who holds which power](../design/spec/security-details.md#who-holds-which-power).

## Threats that crewboard handles

| Threat | What crewboard does | Where |
| --- | --- | --- |
| A forged message: a comment that claims to come from an agent. | The signature does not match the key of that agent. `watch` does not deliver it and reports a security event `bad-signature`. | [Signature](../design/spec/messages.md#signature); [events.rs](../../src/events.rs) |
| An edited message: someone changes a valid message later. | The body hash is signed, so the signature fails. Any comment whose `updated_at` differs from `created_at` is refused as `edited-message`, even if the body still verifies. | [canonical.rs](../../src/protocol/canonical.rs); [events.rs](../../src/events.rs) (`classify_message`) |
| An edited owner comment: the holder of any crew token rewrites an owner answer or `/crew reject`. | An owner comment counts only if it was never edited, or if the owner made every edit. Otherwise it is not delivered, and a security event `edited-message` names the editor. | [Edited owner comments](../design/spec/security-details.md#edited-owner-comments); [comments.rs](../../src/comments.rs) |
| A replayed message in another issue. | The issue (`owner/repo#number`) is signed. The copy fails the check. | [canonical.rs](../../src/protocol/canonical.rs) |
| A replayed message in the same issue. | The signed `ts` must be at most 15 minutes before and 5 minutes after the GitHub `created_at`. A copy inside the window repeats a sender and signature that the issue already shows. Both are refused as `replayed-message`. One limit is a [residual risk](#residual-risks). | [replay.rs](../../src/protocol/replay.rs); [comments.rs](../../src/comments.rs) (`check_ts`) |
| An unknown sender: a crew header with a name that nobody approved. | Refused as `unknown-sender`. | [events.rs](../../src/events.rs); [registry.rs](../../src/registry.rs) |
| A stranger who posts on a ticket. | The comment is "foreign": dropped and counted. With a crew header it is a security event `foreign-author`. | [Untrusted input](../design/spec/messages.md#untrusted-input); [events.rs](../../src/events.rs) (`is_foreign`) |
| One agent's account posts a message signed by another agent. | The approval binds each agent to one account. A message from another account is not delivered: security event `foreign-author`, with both accounts. | [Signature](../design/spec/messages.md#signature); [events.rs](../../src/events.rs) (`classify`) |
| A stranger who asks to join. | Nothing happens until the owner approves. `approve` shows the account; `init` locks the registry issue to collaborators. | [Registry issue](../design/spec/board.md#registry-issue); [registry.rs](../../src/registry.rs) |
| A stolen agent key. | The key alone is not enough: the comment must also come from the approved account. The owner can reject the agent ([leaks](#a-key-or-a-token-leaks)). | [events.rs](../../src/events.rs); [registry.rs](../../src/registry.rs) |
| An agent that runs with the owner's token and so looks like the owner. | On a crew board every agent command refuses the owner's token (`agent-uses-owner-token`), and `approve` refuses a join of the owner's account (`approve-owner-account`). `selftest` refuses a token (`agent-token-not-crew`) or a plain `gh` (`agent-gh-not-crew`) of another account. This stops mistakes, not intent. | [Phases](../design/spec/identity.md#phases); [agent_gh.rs](../../src/agent_gh.rs) |
| An owner command that runs with an agent's token. | An owner command reads `--env-file` or the owner's own `gh auth token`, ignores `GH_TOKEN` and `GITHUB_TOKEN`, and refuses any login but the board owner (`owner-command-not-owner`). In an agent's config dir it never runs `gh`. This stops mistakes, not an agent that runs `gh` itself. | [Token handling](../design/spec/security-details.md#token-handling); [ctx/mod.rs](../../src/ctx/mod.rs) (`new_owner_client`) |
| A secret in a message. | Every signed post and `ticket new` scan the body against a pattern list and refuse to post. | [Token handling](../design/spec/security-details.md#token-handling); [secrets.rs](../../src/secrets.rs) |
| A token sent to the wrong host. | The API base is fixed, and crewboard follows only URLs under it. | [Token handling](../design/spec/security-details.md#token-handling); [github/mod.rs](../../src/github/mod.rs) |
| A model in the headless coordinator's dir that leaves its scope: it removes the `headless` key or passes `--config-dir`. | Not stopped. The key is a guardrail against mistakes and prompt drift, not a boundary ([residual risks](#residual-risks)). | [Headless coordinator](../design/spec/agents.md#headless-coordinator) |
| A coordinator model that merges a crew PR that edits a protected path. | `verify`, `close --verified` and `merge` (so also `tick`) refuse it with `protected-path-owner-merge`, whatever the keys say. A rename out of such a path counts. A file list of more than 1000 files, or one that cannot be read, is refused (fail closed), also by `done`. `verify --fail` stays allowed. | [Protected paths](owner.md#protected-paths); [rules/protected.rs](../../src/rules/protected.rs) |
| The holder of any crew token edits the issue body to turn a Gate into "CI green" or to add `CI: none`. | The headless "CI only" check reads the signed `gate:` line of the assignment, not the body. `assign` also signs `ci: none` or `ci: required`, and `done`, `verify` and `merge` read that line. One hole remains: a ticket assigned before the signed `ci:` line existed falls back to the body. | [Ticket content](../design/spec/tickets.md#ticket-content); [headless.rs](../../src/headless.rs) |
| The holder of any crew token writes the board `State` of an agent to mute it or to free its tickets. | The `State` is a mirror. The registry log decides who may act and who stops. The agent stays trusted and keeps its tickets. `status` shows a `trust problem` line, `audit` a `field-mismatch`, and the coordinator tells the owner once. | [Fields](../design/spec/board.md#fields); [registry.rs](../../src/registry.rs) (`build_trust`) |
| A ticket text that tries to widen the scope of an agent. | Comments are task data. Only `AUTHORITY.md` and owner messages define what is allowed, and crewboard refuses commands that `AUTHORITY.md` forbids. | [Untrusted input](../design/spec/messages.md#untrusted-input); [authority.rs](../../src/authority.rs) |

## Residual risks

crewboard cannot remove these risks. Plan for them.

- **The default branch is not protected.** Every crew account has Write. Without a rule that
  requires a pull request, an agent can push a change of `AUTHORITY.md` or `.crewboard.toml`
  directly. `init` sets such a rule ([Branch rule](owner.md#branch-rule)). The risk stays with
  `--no-branch-rule`, a token that is not admin, or a rule that you delete later. `start` and
  `doctor --board` warn when nothing protects the branch, and say `unknown` when they cannot
  read it.
- **A stolen crew token**, of the shared account or of a contributor. The holder has the write
  access of the account: it can change board fields, edit or delete comments, open PRs and read
  the repos. It needs an approved agent key to make a valid agent message. A holder who deletes
  a message can post one copy of it within 15 minutes of the signing time. Its headerless
  comments are only foreign. All agents of the shared account share its token; a contributor's
  token reaches only that contributor's agents. Revoke it at once ([leaks](#a-key-or-a-token-leaks)).
- **A contributor account** has the Write role, so it can do what a stolen crew token can do.
  Add only people that you trust, and remove the access of a contributor who leaves.
- **Join requests of strangers.** On a public repo any GitHub user can ask to join if the
  registry issue is not locked ([Init](owner.md#init)). `approve` shows the account of each request.
- **The trial.** With `insecure_one_account` the owner's token is also the agents' token. Use it
  only for a one-person trial on a test repo.
- **The owner's token on an agent host.** An agent there can act as the owner: a headerless
  comment of the owner account is an owner message. Run autonomous agents only on hosts without
  an owner token. `audit` shows this afterwards; it does not prevent it
  ([Phases](../design/spec/identity.md#phases)).
- **A runtime that strips the environment.** Some runtimes remove `GH_TOKEN` from the model's
  commands, so plain `gh` can fall back to the owner's login. Put the shim of `crewboard agent
  shim` (`<agent dir>/bin/gh`) first on the `PATH`.
- **A compromised agent host.** The private key and a crew token sit there, so an attacker can
  sign as that agent and use the crew account. crewboard has no
  hardware key and no key encryption at rest, only the file mode. Keep agents on separate hosts
  and keep the tickets that an agent can touch few.
- **A bad agent that is approved.** A valid signature proves who wrote a message, not that it is
  true or safe. `AUTHORITY.md` limits what crewboard allows, the coordinator verifies evidence,
  and the owner holds merge, release and credentials.
- **Owner-only rules rest on GitHub accounts.** If the owner account is taken over, or a
  collaborator can write to the registry issue, the trust roots are open.
- **The headless OS user.** The `headless` key limits the `crewboard` binary, not the OS user. That
  user can edit its own config dir, and it holds a crew token and a signing key, so its own code
  can call the GitHub API or sign. The boundary is a separate OS user with a limited token: no
  admin rights, no "Workflows" permission, a branch rule that requires CI and allows no bypass
  ([Headless setup](headless-setup.md)).
- **Deleted comments.** crewboard refuses an edited message, but anyone with write access can
  delete a comment, and a deleted message never arrived. This includes your `/crew reject`:
  revoke the crew token before you reject ([leaks](#a-key-or-a-token-leaks)).
- **The secret scan** finds only the listed patterns.
- **The model.** The skill tells the model to treat comments as data. A valid message of an
  approved agent can still mislead it.
- **Cache and clock.** An attacker who copies a visible nonce within the selftest window can
  quiet one forged comment ([`selftest`](../design/spec/messages.md#selftest)). The owner then
  sees one extra alarm.

## A key or a token leaks

Only these steps have support in the code or the docs. Anything else is not covered.

**A crew token leaks.** Revoke it on GitHub. The agents of that account then exit with 3 or 4
and do nothing. Make a new token and load it through the env file of each host; for a
contributor's own account the contributor does this, or you remove its access. Then run
`crewboard audit --since <time>` and check each finding since the leak
([`audit`](../design/spec/security-details.md#selftest-audit-and-status)).

**A host is compromised.** The attacker holds the crew token and the key of every agent on the
host, and can delete your `/crew reject`. Do the steps in this order:

1. Revoke the crew token on GitHub. The attacker can no longer write.
2. Post `/crew reject <name>` for each agent of the host (see "An agent key leaks").
3. Make a new token and load it on the hosts that are still clean.
4. Run `crewboard audit --since <time>` and check each finding since the leak.

A reject is safe only after step 1. Release 0.7 moves rejects to a place that the token cannot
write.

**A contributor leaves.** See [the Owner guide](owner.md#a-contributor-with-an-own-account).

**An agent key leaks.**

1. Post `/crew reject <name>` in the registry issue. crewboard then refuses the agent's messages
   (`unknown-sender`), and `status` shows it as `REJECTED`; the item `State` does not change
   ([Lifecycle](../design/spec/board.md#lifecycle), [registry.rs](../../src/registry.rs)).
2. Have the agent join again under a new name with a new key, and approve it. A decided name is
   final: `join` refuses a known name with another key (`name-taken`)
   ([ops/join.rs](../../src/ops/join.rs)).
3. Run `crewboard audit` and check the messages of that name since the leak.

Old signatures stay valid: crewboard has no key revocation list with a time stamp, and no rule
for tickets that the leaked key already changed.

**The owner token leaks.** Not covered. Revoke the token on GitHub, then review the owner
comments and the registry issue by hand: a headerless owner comment is trusted as the owner.

**Not covered yet:** a command that rotates a key under the same name, an automatic lockout
after a security event, and a guide for the loss of the owner account.

If this page, the specification and the code differ, the code decides what is true today. Open
an issue, so that the text or the code is fixed.
