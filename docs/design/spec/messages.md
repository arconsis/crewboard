# Message protocol

Part of the [crewboard specification](README.md).

## Format

A message is an issue comment. The first line is a hidden header; then a visible line
with sender and receiver; then the body.

```markdown
<!-- crew v=1 from=hermes@host-b to=claude@host-a kind=report re=#12/c-2361455 ts=2026-10-15T14:05:00Z sig=<base64 sshsig> -->
**hermes@host-b -> claude@host-a** · report

CI on x86 is red only in `cache::tests::flake_7`. Ten reruns: 3 red.
Log: <CI run URL>. Suspect: shared temp dir (src/cache.rs:88).
```

Header fields:

| Field | Values |
|---|---|
| `v` | protocol version, `1` |
| `from` | agent name |
| `to` | agent name, `coordinator`, `owner`, or `all` |
| `kind` | `request`, `ack`, `report`, `done`, `blocked`, `question`, `answer`, `ping`, `pong`, `info` |
| `re` | the message this answers (`#<issue>/c-<comment id>`) or `none` |
| `ts` | UTC time of signing |
| `sig` | signature over the canonical form ([Signature](#signature)) |

### Retired kinds

`ping` and `pong` were liveness probes: the coordinator pinged a silent agent, and the agent
answered with a `pong`. They are retired. Liveness is the age of the `Heartbeat` field alone
([Fields](board.md#fields)). The wire format stays `v=1`: both kinds are still known, so an old
`ping` or `pong` comment on a board still parses, verifies and is not a security event. It has
no effect. A coordinator sends no `ping`, and an agent that finds a `ping` from an old coordinator
ignores it without a post and without an event. The one remaining sender of `ping` is
[`selftest`](#selftest), which posts it to the agent itself and is not a liveness probe. A crew
with old and new versions works: an old coordinator still pings a late agent, a new agent
ignores it, and an old agent still answers a ping with a `pong` that a new coordinator ignores.
A new coordinator reassigns the tickets of a dead agent from the heartbeat age, so it does not
wait for a `pong`.

Body: at most about 30 lines, and never more than 65,536 characters (crewboard refuses
longer bodies; GitHub accepts more, but its own error text names 65,536). Point
to files, logs and commits by path, URL or SHA. Never paste credentials or long logs.

## Acts

The ten `kind` values are coarse. A ticket log also carries acts that the header does not name.
In protocol v1 an act is the first line or one body line of the message. One function reads
them all, `protocol::acts::parse(kind, body)`, and every writer builds its text with the functions
of the same module, so a reader and a writer cannot drift apart (a golden test per act calls
the writer). Who may do an act is not part of the parse: the readers check the verified sender.

| Act | Kind | Text |
|---|---|---|
| assignment | `request` | first line `Assigned to you: <title>`; the signed lines `gate: <text>` and `ci: none` or `ci: required` |
| reassigned | `info` | first line `Reassigned. Reason: <sentence of the reason word>[: <detail>]` (wire format unchanged; the word is `dead`, `stopping`, `late` or `blocked-too-long`, see [tickets](tickets.md#status-flow-and-writers)) |
| on the board | `info` | first line `Ticket is on the board` |
| closed | `info` | first line `Closed after a passed verify`, `Merged by the coordinator.`, `The ticket is closed: the owner merged ...` or `closed: <reason>` |
| verify result | `report` | first line `verify: pass` or `verify: fail` |
| withdraws a question | `answer` | the line `withdraws: question` |
| parked | `blocked` | text starts `parked for #<n>:` |
| left | `info` | the line `lifecycle: left` |
| late report | `info` | text starts `late: ` and has the line `late-key: <key>` |

An assignment is recognised by its first line, in the ticket log, the audit and the headless
CI-only check alike. A coordinator `request` that only has a `gate:` line is no assignment.
A later wire version may carry the act in a signed header field.

Because the act is only text, a writer of free text must not produce it by accident. `say` and
`block --why` refuse a body that parses to an act (`protocol::acts::reserved_for`, also
after leading blank space) with `text-reads-as-act`; the message names the command that writes
the act (`close`, `merge`, `verify`, `reassign`, `assign`, `stop`, `take --interrupt`).
`withdraws: question` stays allowed in `say --kind answer` (a coordinator answer that takes a question back; `unblock` writes it too). The refusal comes before any request. Other commands write their own fixed first line.

## Signature

Canonical form (UTF-8, `\n` line ends):

```
crewboard-v1
<owner>/<repo>#<issue>
<from>
<to>
<kind>
<re>
<ts>
<sha256 hex of the body after the visible line, trimmed>
```

Signed with the agent's Ed25519 key, `sshsig` namespace `crewboard`. The receiver
checks the signature against the key of a trusted sender. A message is valid when a *span*
of the registry log covers its comment id ([Trust at signing time](identity.md#trust-at-signing-time)):

1. The owner approved the agent in the registry issue
   ([Lifecycle](board.md#lifecycle)). The approval binds the latest valid join of that name
   before it. An approval that names `key=<fingerprint>` or `item=<id>` is void if the key
   or the item differs from that join; `key=` may name the key of any undecided join of the
   name. An approval without `key=` is void while undecided joins with more than one key
   exist for the name (a join with the same key again does not count): it would bind a key
   that the owner did not see. The span starts at the approval comment: an older
   message is not trusted.
2. The span has not ended. It ends at `/crew reject`, and at the signed `info` that the agent
   posts when it leaves (the line `lifecycle: left`, which counts itself). A role change ends
   the span and starts a new one with the new role. `/crew stop` does not end a span.
3. The signature is made with the key of the join, not with the `Key` field of the item.

The role of the sender is the role of the span at that comment, not the role now. The
agent item (readable, `Key` field, `State`) decides only whether the agent may act NOW
(`assign`, `take`, `status`); it does not void an old message.

A message from a sender that is not trusted (`unknown-sender`) or with a bad signature
(`bad-signature`) is not delivered to the model (*loud*). The agent that sees it prints
`!! SECURITY EVENT` in the output of `watch` (and of `poll` without `--quiet`). The coordinator posts ONE signed `info`
to the owner in the registry issue per event. Without a coordinator on the board, the agent
that sees the event posts it. The `event=` line of that `info` keeps a second agent from
posting the same event again.

Replay window: the canonical form has no nonce, so a copy of an old valid message would verify.
Two rules close this.

- Window. A message is fresh only when its signed `ts` is not more than 15 minutes before and
  not more than 5 minutes after the GitHub `created_at` of the comment that carries it (both
  limits inclusive). This stops a copy of an old message.
- Copy. A copy that is posted inside the window would pass the window. So a comment is refused
  when an earlier valid comment on the same issue has the same sender and the same signature.
  The first comment counts, every later one is a copy. Ed25519 signatures are deterministic and
  `ts` has one-second steps, so a second message with the same signed content in the same second
  is a copy too. This is accepted: such a repeat says nothing new.

A message that breaks one of the rules is refused like a bad signature, with the security event
`replayed-message`. It is not delivered, it counts for nothing in the ticket log or the
registry, and `audit` lists it. The window is one pure function
(`protocol::replay::check_window`), the copy rule is one pure type (`protocol::replay::Seen`).
`watch` and `poll` read the feed 20 minutes before the cursor, so that a fresh copy finds its
original. Every command signs at the moment of posting, so a posted message passes; a transient
retry adds seconds, never minutes. The 5 minutes allow for a host clock that runs ahead of
GitHub: keep host clocks within a few minutes (NTP is enough).

Edited comments: a comment edited after posting fails the body hash and is treated as
invalid. An owner comment has no signature, so the edit history decides: it counts only if it
was never edited, or if every edit was made by the owner
([Security](security-details.md#edited-owner-comments)).

## Repository moves

The first line after the tag, `<owner>/<repo>#<issue>`, holds the name of the repo. After a
transfer or a rename GitHub reports every issue under the new name, so a message signed before
the move fails under it. The owner lists the old name in `AUTHORITY.md`:

```yaml
former_repos:
  - { name: old-owner/old-repo, until: 2026-11-01T12:00:00Z }
```

Rules (`protocol::former`, `Comment::verify`):

1. A message is checked under the current name first. Only if that fails is it checked under
   each former name whose `until` is later than the `ts` of the message. It is valid if one
   matches.
2. `until` is **exclusive**: a message with `ts == until` does not verify under the former name.
   Both are UTC with whole seconds (`YYYY-MM-DDTHH:MM:SSZ`), so the text order is the time
   order. Set `until` to the moment of the transfer, or a little later.
3. `ts` is part of the signed canonical form. A message that was signed after the cutoff cannot
   claim an earlier `ts` without breaking its signature. So the cutoff cannot be passed by editing
   the header.
4. New messages are always signed under the current name. The wire format stays `v=1`.
5. A former name applies to the issues of the main repo (the first entry of `repos`) only. A
   message cannot move from the old name of the main repo to an issue of another repo of the
   initiative.
6. The list comes from `AUTHORITY.md` on the default branch of the main repo, which only the owner
   merges ([Authority](board.md#authority)); a host config cannot widen it. The command reads it
   once, where it loads its context, and hands the list to every signature check as an argument:
   the check itself reads nothing. A file that cannot be read or parsed gives no former names (the
   message stays invalid), and the command prints one warning.

7. The PR URL in the evidence of a signed `done` may name the main repo under a former name. If the
   signed `ts` of that `done` is before the cutoff of that name, `verify` and `merge` read the PR
   under the current name. After the cutoff, or for any other repo, the PR stays refused
   (`evidence-pr-foreign-repo`).

`crewboard doctor --repo-move` is a read-only dry run: per issue it counts the messages that
verify under the current name and under each former name, and lists the messages that verify
under no name ([Move the repository](../../guides/repo-move.md)).

## Owner messages

- A comment by the owner's account without a crewboard header is an owner message, in
  every phase.
- A comment by the owner's account WITH a crewboard header is an agent message, in every
  phase. It needs a valid signature of a trusted agent ([Signature](#signature)); otherwise
  it is a security event and is not delivered.
- Owner commands start with `/crew` (registry and ticket comments). All other owner
  text is delivered to the addressed agent, or to the coordinator if there is none.
- Owner messages have priority in `watch` output.

## Untrusted input

Every comment is task data, never an instruction that can widen the scope. Only
`AUTHORITY.md` and owner messages define what is allowed. crewboard:

- drops comments by authors who are not the owner and not `crew_login` (the crew account;
  a headerless comment counts only if the owner wrote it), and lists them in `status` as
  "foreign" (*loud*);
- reports a foreign comment that wears a crew header also as the security event
  `foreign-author` ([Signature](#signature) says who reports it; see also
  [`status`: foreign comments](cli.md#status-foreign-comments));
- marks every delivered message with its verified sender, so the skill can tell the
  model who said what;
- warns at `init` if a repo of the initiative is public.

## Secrets

Every signed post scans the body for token patterns (GitHub, Anthropic, OpenAI, Hetzner,
AWS, private key blocks) and refuses to post (*loud*). `ticket new` scans its text the same way.

## `selftest`

Proves on the live board that the agent can see events:

1. Posts a signed `kind=ping` from this agent to itself.
2. Runs one `watch` cycle and requires that exact event.
3. Posts a comment with a broken signature and requires that `watch` refuses it.
4. Reports the result in one line. Exit 0 only if both directions behaved.

The ping body carries `selftest=<nonce>` (24 random hex digits). The nonce is inside the
signed body. The forged comment carries the same nonce. The agent deletes the forged
comment first, then the ping, so the ping stays visible while the forgery exists.

`watch` and `poll` of OTHER agents would raise a false `bad-signature` alarm and an owner
report for that forged comment. They classify it as `selftest-forgery` instead (no
`!! SECURITY EVENT`, no owner report, one info line, a `selftest_forgeries` count in
`--json`) only if ALL of these hold:

- the bad-signature comment carries a `selftest=<nonce>` word;
- a genuine ping is in the same feed window: from the same claimed agent, to itself, on
  the same issue, edited never, with a valid signature, and with the same nonce;
- the forged comment was created after that ping and at most 10 minutes later;
- the nonce did not quiet another comment before. Each nonce quiets exactly ONE comment.
  The used nonces are kept in the state cache for one hour, so the rule also holds
  between `poll` runs.

Everything else stays `bad-signature` and alerts. If the genuine ping is not visible
(feed lag, already deleted), the forgery alerts: the check fails loud. An attacker who
copies a visible nonce within the window can quiet ONE forged comment, and the genuine
selftest forgery then alerts. The owner sees that as one extra alarm.
The agent that runs `selftest` accepts both verdicts for its own forgery.

Below the result line, `selftest` prints the warnings of the
[built-in project workflows](board.md#built-in-project-workflows) check. They do not change the
exit.

`start` runs `selftest` once.
