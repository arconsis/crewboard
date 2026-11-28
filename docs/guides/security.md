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
- On a crew board (a shared agent account or own accounts), every agent command refuses the
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

- It does not prevent a board field from being written. The holder of any agent token can set
  `Status = Done`. crewboard detects it ([Done without proof](../concepts.md#done-without-proof-is-detected)).
- It does not stop an agent that holds the owner's token on purpose, a compromised host, or a
  person who steals an agent token or a private key.
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
| An edited message: someone changes a valid message later. | The body hash is signed, so the signature fails. Any comment whose `updated_at` differs from `created_at` is refused as `edited-message`, even if the body still verifies. | [canonical.rs](../../crates/crewboard-protocol/src/canonical.rs); [events.rs](../../src/events.rs) (`classify_message`) |
| An edited owner comment: the holder of any agent token rewrites an owner answer or `/crew reject`. | An owner comment counts only if it was never edited, or if the owner made every edit. Otherwise it is not delivered, and a security event `edited-message` names the editor. | [Edited owner comments](../design/spec/security-details.md#edited-owner-comments); [comments.rs](../../src/comments.rs) |
| A replayed message in another issue. | The issue (`owner/repo#number`) is signed. The copy fails the check. | [canonical.rs](../../crates/crewboard-protocol/src/canonical.rs) |
| A replayed message in the same issue. | The signed `ts` must be at most 15 minutes before and 5 minutes after the GitHub `created_at`. A copy inside the window repeats a sender and signature that the issue already shows. Both are refused as `replayed-message`. One limit is a [residual risk](#residual-risks). | [replay.rs](../../crates/crewboard-protocol/src/replay.rs); [comments.rs](../../src/comments.rs) (`check_ts`) |
| An unknown sender: a crew header with a name that nobody approved. | Refused as `unknown-sender`. | [events.rs](../../src/events.rs); [registry.rs](../../src/registry.rs) |
| A stranger who posts on a ticket. | The comment is "foreign": dropped and counted. With a crew header it is a security event `foreign-author`. | [Untrusted input](../design/spec/messages.md#untrusted-input); [events.rs](../../src/events.rs) (`is_foreign`) |
| One agent's account posts a message signed by another agent. | The approval binds each agent to one account. A message from another account is not delivered: security event `foreign-author`, with both accounts. | [Signature](../design/spec/messages.md#signature); [events.rs](../../src/events.rs) (`classify`) |
| A deleted note: the holder of an agent token deletes a signed note of the coordinator (a close, a verify report, an assignment) and sets the board back. | The coordinator remembers the comment ids of its own notes on each ticket. When it reads the ticket again and a note is gone, it raises the security event `deleted-message` once and tells the owner. The limits are a [residual risk](#residual-risks). | [Deleted notes](../design/spec/security-details.md#deleted-notes); [own_notes.rs](../../src/ops/own_notes.rs) |
| A stranger who asks to join. | Nothing happens until the owner approves. `approve` shows the account; `init` locks the registry issue to collaborators. | [Registry issue](../design/spec/board.md#registry-issue); [registry.rs](../../src/registry.rs) |
| A stolen agent key. | The key alone is not enough: the comment must also come from the approved account. The owner can reject the agent ([leaks](#a-key-or-a-token-leaks)). | [events.rs](../../src/events.rs); [registry.rs](../../src/registry.rs) |
| An agent that runs with the owner's token and so looks like the owner. | On a crew board every agent command refuses the owner's token (`agent-uses-owner-token`), and `approve` refuses a join of the owner's account (`approve-owner-account`). `selftest` refuses a token (`agent-token-not-crew`) or a plain `gh` (`agent-gh-not-crew`) of another account. This stops mistakes, not intent. | [Phases](../design/spec/identity.md#phases); [agent_gh.rs](../../src/agent_gh.rs) |
| An owner command that runs with an agent's token. | An owner command reads `--env-file` or the owner's own `gh auth token`, ignores `GH_TOKEN` and `GITHUB_TOKEN`, and refuses any login but the board owner (`owner-command-not-owner`). In an agent's config dir it never runs `gh`. This stops mistakes, not an agent that runs `gh` itself. | [Token handling](../design/spec/security-details.md#token-handling); [ctx/mod.rs](../../src/ctx/mod.rs) (`new_owner_client`) |
| A secret in a message. | Every signed post and `ticket new` scan the body against a pattern list and refuse to post. | [Token handling](../design/spec/security-details.md#token-handling); [secrets.rs](../../src/secrets.rs) |
| A token sent to the wrong host. | The API base is fixed, and crewboard follows only URLs under it. | [Token handling](../design/spec/security-details.md#token-handling); [github/mod.rs](../../src/github/mod.rs) |
| A model in the headless coordinator's dir that a text in a PR or an issue steers: it passes a ticket, releases a ticket that waits for you, or floods the board. | With `unattended_coordinator: off` (the default) a headless config dir makes no judgment: `unattended-off`. With `verify`, each command decides from the signed log of the ticket and not from a list or a board field. It refuses a ticket with an open question to you, a ticket that the round limit stopped, a PR on the owner route and a ticket that the signed log does not give to the agent on the board. It cannot withdraw your question, create or reassign a ticket or run checks on the host (`verify --run-check`). crewboard takes no file path for a message text there (`--body-file <path>`); the text comes from `--body` or from standard input. It assigns only a ticket whose log has a signed assignment to copy, so it never signs a Gate, a guard or a CI line from an issue body; a headless config dir merges only a ticket whose signed Gate is `ci`. A verify pass still reads the files of the PR, so a protected path goes to you. The verdicts have a budget per UTC day (`unattended_budget`), one ticket gets 2, and a day has at most 30 messages and 5 questions to you. Each message is reserved under the lock of the state file before the post, and a failed verify that stops a ticket at the round limit reserves its result and its question together. A count that cannot be read or saved refuses the command, and so does a stored count of a later day than today. A post that fails after the reservation stays counted. Every such message has the line `unattended: <n>/<budget>`, which only crewboard writes. Limits that stay: the text of an answer, a question or a verdict is free text from the coordinator account, and a dev model reads it as the coordinator's word; a question to you stops the tick's verify and merge of that ticket until you answer; the counts that survive a deleted comment live in the state of one config dir on one host; two commands that run at the same time can both post a question on one ticket, because the lock holds the counts and not that rule; the day comes from the host clock and the count from GitHub's time, so a host clock that is ahead reads a new day with a count of 0; `agent exec`, `agent with-env`, `lock run` and `repeat` start a command in a headless config dir, so a model with a shell can read a file of that OS user and pass it on standard input: what it can read is a limit of the runtime and the OS user, not of crewboard; and what that OS user does with its token outside crewboard ([residual risks](#residual-risks)). | [Unattended coordinator](../design/spec/agents.md#the-scope-of-the-unattended-coordinator); [unattended.rs](../../src/unattended.rs) (`check_act`) |
| A model in the headless coordinator's dir that leaves its scope: it removes the `headless` key or passes `--config-dir`. | Not stopped. The key is a guardrail against mistakes and prompt drift, not a boundary ([residual risks](#residual-risks)). | [Headless coordinator](../design/spec/agents.md#headless-coordinator) |
| A coordinator model that merges a crew PR that edits a protected path. | `verify`, `close --verified` and `merge` (so also `tick`) refuse it with `protected-path-owner-merge`, whatever the keys say. A rename out of such a path counts. A file list of more than 1000 files, or one that cannot be read, is refused (fail closed), also by `done`. A PR whose base is not the default branch is also judged by the files that differ between the default branch and its head: the author of a PR chooses the base branch, and a base that already holds the change would hide the file. The command reads the open PR again after the file lists, for every base branch: a head that moved or a base branch that changed is `evidence-head-moved`. One window stays: a head or a base that changes and is back at its first value between the two reads is not seen; each try that misses the window is a visible refusal. `verify --fail` stays allowed. | [Protected paths](owner.md#protected-paths); [rules/protected.rs](../../src/rules/protected.rs) |
| The holder of any agent token edits the issue body to turn a Gate into "CI green" or to add `CI: none`. | The headless "CI only" check reads the signed `gate:` line of the assignment, not the body. `assign` also signs `ci: none` or `ci: required`, and `done`, `verify` and `merge` read that line. Only the first assignment of a ticket signs the body. A later assignment (`reassign --to`, the move of a queued ticket, an `assign` after a `reassign`) copies the signed lines and warns when the body differs. Two limits remain. When the assignment comment is deleted or edited, the log has no assignment, and the next `assign` signs the body again: it prints the Gate that it signs, with a warning. A signed assignment without a `ci:` line (a ticket of an earlier version) lets the body decide about CI. | [Ticket content](../design/spec/tickets.md#ticket-content); [headless.rs](../../src/headless.rs) |
| A dev model that is told "make the test pass" edits the test, a fixture, a golden file or the check script. | The ticket names these paths in `Guard:` lines (`ticket new --guard <glob>`). The assignment signs them as `guard:` lines, and every later assignment copies the signed lines; no reader takes the guard from the body. `done`, a verify pass, `close --verified` and `merge` (so also `tick`) refuse a PR that changes such a file with `guarded-path-changed`. A rename counts with both names. No flag overrides it. A base branch that hides the file does not help: the files that differ from the default branch count too. With `--must-fail-at`, a guarded file must be the same on the red commit and on the tested one. Four limits remain. The guard holds for the paths that the coordinator names, for the PR of that ticket and for a merge by the crewboard CLI: a second ticket without the guard or a direct push can change the file. Until the first `assign` the guard is unsigned body text; `assign` prints the guard that it signs, also `guard none`, and `ticket new --assign` signs at once. When the assignment comment is deleted or edited, the next `assign` signs the body again, with the guard that the body has then: the warning and the `signed from the body:` line name it. And a coordinator host with a version without guarded paths writes assignments without `guard:` lines, which ends the guard of a ticket without a warning: update every coordinator host before the first `--guard`. | [Guarded paths](../design/spec/tickets.md#guarded-paths); [rules/guard.rs](../../src/rules/guard.rs) |
| The holder of any agent token writes the board `State` of an agent to mute it or to free its tickets. | The `State` is a mirror. The registry log decides who may act and who stops. The agent stays trusted and keeps its tickets. `status` shows a `trust problem` line, `audit` a `field-mismatch`, and the coordinator tells the owner once. | [Fields](../design/spec/board.md#fields); [registry.rs](../../src/registry.rs) (`build_trust`) |
| A ticket text that tries to widen the scope of an agent. | Comments are task data. Only `AUTHORITY.md` and owner messages define what is allowed, and crewboard refuses commands that `AUTHORITY.md` forbids. | [Untrusted input](../design/spec/messages.md#untrusted-input); [authority.rs](../../src/authority.rs) |

## Residual risks

crewboard cannot remove these risks. Plan for them.

- **The default branch is not protected.** Every agent account has Write. Without a rule that
  requires a pull request, an agent can push a change of `AUTHORITY.md` or `.crewboard.toml`
  directly. `init` sets such a rule ([Branch rule](owner.md#branch-rule)). The risk stays with
  `--no-branch-rule`, a token that is not admin, or a rule that you delete later. `start` and
  `doctor --board` warn when nothing protects the branch, and say `unknown` when they cannot
  read it.
- **A stolen agent token**, of the shared account or of a contributor. The holder has the write
  access of the account: it can change board fields, edit or delete comments, open PRs and read
  the repos. It needs an approved agent key to make a valid agent message. A holder who deletes
  a message can post one copy of it within 15 minutes of the signing time. Its headerless
  comments are only foreign. All agents of the shared account share its token; a contributor's
  token reaches only that contributor's agents. Revoke it at once ([leaks](#a-key-or-a-token-leaks)).
- **A contributor account** has the Write role, so it can do what a stolen agent token can do.
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
- **A compromised agent host.** The private key and an agent token sit there, so an attacker can
  sign as that agent and use the agent account. crewboard has no
  hardware key and no key encryption at rest, only the file mode. Keep agents on separate hosts
  and keep the tickets that an agent can touch few.
- **A bad agent that is approved.** A valid signature proves who wrote a message, not that it is
  true or safe. `AUTHORITY.md` limits what crewboard allows, the coordinator verifies evidence,
  and the owner holds merge, release and credentials.
- **Owner-only rules rest on GitHub accounts.** If the owner account is taken over, or a
  collaborator can write to the registry issue, the trust roots are open.
- **The headless OS user.** The `headless` key limits the `crewboard` binary, not the OS user. That
  user can edit its own config dir, and it holds an agent token and a signing key, so its own code
  can call the GitHub API or sign. The boundary is a separate OS user with a limited token: no
  admin rights, no "Workflows" permission, a branch rule that requires CI and allows no bypass
  ([Headless setup](headless-setup.md)).
- **Deleted comments.** crewboard refuses an edited message, but anyone with write access can
  delete a comment, and a deleted message never arrived. This includes your `/crew reject`:
  revoke the agent token before you reject ([leaks](#a-key-or-a-token-leaks)). Only a deleted
  note of the coordinator is detected, and only by the coordinator that posted it: its state
  file holds the ids. A fresh host or a lost state file remembers nothing, and `audit` or
  `status` from another host cannot see a deletion. On a ticket with more than 5000 comments a
  deletion is seen only when the list fits again; you get one note about such a ticket
  ([Deleted notes](../design/spec/security-details.md#deleted-notes)).
- **The secret scan** finds only the listed patterns.
- **The model.** The skill tells the model to treat comments as data. A valid message of an
  approved agent can still mislead it.
- **Cache and clock.** An attacker who copies a visible nonce within the selftest window can
  quiet one forged comment ([`selftest`](../design/spec/messages.md#selftest)). The owner then
  sees one extra alarm.

## A key or a token leaks

Only these steps have support in the code or the docs. Anything else is not covered.

**An agent token leaks.** Revoke it on GitHub. The agents of that account then exit with 3 or 4
and do nothing. Make a new token and load it through the env file of each host; for a
contributor's own account the contributor does this, or you remove its access. Then run
`crewboard audit --since <time>` and check each finding since the leak
([`audit`](../design/spec/security-details.md#selftest-audit-and-status)).

**A host is compromised.** The attacker holds the agent token and the key of every agent on the
host, and can delete your `/crew reject`. Do the steps in this order:

1. Revoke the agent token on GitHub. The attacker can no longer write.
2. Post `/crew reject <name>` for each agent of the host (see "An agent key leaks").
3. Make a new token and load it on the hosts that are still clean.
4. Run `crewboard audit --since <time>` and check each finding since the leak.

A reject is safe only after step 1. Release 0.7 moves rejects to a place that the token cannot
write.

**A contributor leaves.** See [the Owner guide](owner.md#give-an-account-access).

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
