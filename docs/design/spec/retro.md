# Retrospective report

Part of the [crewboard specification](README.md).

`crewboard retro [--since <time|duration>] [--json] [--initiative <name>]` reads the signed log
of the tickets and prints a retrospective report. It is an [extension](../../guides/extensions.md):
no ticket needs it to reach Done, and the skill tells no agent to run it. The code is
[retro.rs](../../../src/retro.rs) (what counts; pure) and [ops/retro.rs](../../../src/ops/retro.rs)
(the reads and the output).

## Read-only

`retro` writes nothing: no comment, no board field, no cursor and no state file. A headless
coordinator may run it.

## Who may run it

Every reader of the board: the owner, the coordinator, a dev agent. `retro` opens the board as
`status` does. In the config dir of an agent it uses the config and the token of that agent. In
any other config dir it uses the board config of that dir and the token of the caller. It asks
for no role, because the report holds only what the comments of the repos show to every reader.

## What counts

Only a message that passes the trust filter of `status` and `mine` counts
([Signature](messages.md#signature)): a valid signature of a sender that the owner approved,
posted by the account of that sender, not edited, and no copy. A comment without a signature,
a comment of a foreign account and a signed text on another issue count nowhere.

One exception: a message that fails only the replay window counts too. Such a message has a
signed time that is too far from the creation time of its comment: the clock of the host was
wrong, or GitHub stamped the comment late. It is still the sender's own message, and a retro
gives it no authority. A copy of it does not count: the first comment with a signature counts,
every later one is dropped. The report states how many such messages it counted
(`outside_window`).

The sender decides who did an act:

- An assignment, a reassign, a verify result and a close count only from a coordinator.
- A `done` counts only from the assignee at that time, only with evidence, and only while the
  ticket has not ended. This is the rule of the ticket log
  ([Status flow and writers](tickets.md#status-flow-and-writers)): an owner move or a new
  assignment opens an ended ticket again.
- A park (`parked for #<n>:`) counts only from that assignee too. The same text of another
  agent is a plain `blocked` message.
- A late note counts only as a message to the owner, from the coordinator or a dev agent.
- A ticket is an issue with a signed assignment. Any other issue counts nowhere, with one
  exception: [merged work without an assignment](#dropped-with-a-merged-pr).

## The period

Without `--since`, the report covers every ticket. `--since` takes a time
(`2026-10-16T08:00:00Z`) or a duration back from now (`36h`, `7d`). It selects tickets, not messages:
a ticket is in the report when an agent message on it was created at or after that time. The
whole log of each selected ticket counts, so the numbers of a ticket stay true.

## The report

The text and `--json` hold the same data. In JSON, each listed event has the ticket, the URL
of its comment, the time, the sender (`from`) and the addressee (`to`). The text names no
agent: a retrospective looks for classes of causes, and a ranking of agents hides them.

| Section | Content |
|---|---|
| Volume | The first and the last message time, the tickets with an assignment, the delivered tickets (at least one `done`), the [merged work without an assignment](#dropped-with-a-merged-pr). |
| Verify | The tickets with a verify result. The tickets whose first result is a pass, with their share; when accepted fails exist (below), a second figure counts a ticket whose first result is an accepted fail as passed. The tickets with a fail and the number of fails, each with and without the accepted fails. The tickets that went back two times or more (two or more fails that are rework). The passes whose local-run line the tool wrote (`verify --run-check`), against all passes: how much of the verify work a machine decides. |
| Gates | The tickets by the kind of their signed Gate: the `gate:` line of the latest assignment, never the issue body. `ci` is the typed CI-only Gate that the [headless coordinator](agents.md#headless-coordinator) can verify and merge; every other text is "another Gate". For each kind: the tickets and the share whose first verify passed. The share of `ci` tickets says how much of the work CI alone can decide. |
| Verify fails by class | For each class: the count and the fails (ticket and URL). The text lists the latest 5 fails of a class and then the line `... and N more (--json lists all)`. The same limit holds for every list of the text. `--json` lists all. |
| Repeated faults | The review fails that are rework, by the tag of their note. A tag is the word in brackets at the start of the note of `verify --fail` (`[formatter] ...`: lower-case letters, digits and `-`, at most 24); a failed check of the tool has the tag `check-<name>`. The text lists each tag with 3 fails or more with its fails, then the tags below 3 with their counts, then the number of review fails without a tag. `--json` has `tags.rows` (every tag) and `tags.untagged`. The section is left out when no review fail exists. [From a repeated fault to a rule](../../guides/lessons.md) says what the owner does with it. |
| Rounds | The 5 tickets with the most fails that are rework. |
| Andon | The questions to the owner, to the coordinator and to another agent. The `blocked` and the parked messages. The question to the owner that is still open, for each ticket that has not ended. |
| Refusals mentioned | For each [refusal id](../../reference/refusals.md) that a dev agent names as a whole word in a `question`, `blocked` or `report` message: the count and the messages. A refusal of the CLI is not on the board, so this is its only trace. |
| Flow | The reassigned tickets, the dropped tickets, the tickets [dropped with a merged PR](#dropped-with-a-merged-pr), the closes by kind, the late notes. The median and the 90th percentile of the hours from the first assignment to the first `done`, and from the first `done` to the close. |

The median of an even number of values is the mean of the two middle values. The 90th
percentile is the value at position `ceil(0.9 n)` of the sorted values. A close is counted for
a ticket that the log still shows as ended; a ticket that the owner moved back is open.

### Dropped with a merged PR

A PR that another account than the owner merged, without a passed verify, gives no Done. The
coordinator can only drop such a ticket, and the drop note records the PR
([a drop that records a merged PR](tickets.md#a-drop-that-records-a-merged-pr)). The work is
delivered all the same, so the report does not hide it among the drops:

- **Dropped with a merged PR** (text: `dropped with a merged PR`, JSON: `flow.dropped_merged`):
  the dropped tickets whose drop note has the lines `pr:` and `merge-commit:`. Each of them is
  also a dropped ticket (`flow.dropped`) and a close of the kind `dropped`. It is no merge close:
  it accepts no `head-moved` fail and gives no time from `done` to close.
- **Merged work without an assignment** (text: `merged work without an assignment`, JSON:
  `merged_unassigned`): the same drop note of a coordinator on an issue without a signed
  assignment. Such an issue is no ticket. It is in this number and in no other.

Both numbers count only an issue that the log still shows as Dropped, and only the note that
dropped it. Both read only the signed note; `retro` asks GitHub for no PR. A drop note without
these lines, as every build before this one wrote it, is a plain drop. Both numbers rank no
agent.

When the period has no ticket, the text prints the line of the merged work without an
assignment only when that number is not 0. The exit code stays 1.

### Classes of a verify fail

`verify` writes each `failed:` line, in the signed result and in its own answer, from a fixed
text, and `retro` reads the class from the same text ([verify_fail.rs](../../../crates/crewboard-protocol/src/acts/verify_fail.rs)).
A new failure of `verify` does not compile without a class.

| Class | The failure |
|---|---|
| `review` | The Gate check of the coordinator failed (`verify --fail`): free text. A fail without a `failed:` line is `review` too. |
| `head-moved` | The PR head is not the commit of the evidence. |
| `accepted-after-head-moved` | A `head-moved` fail that was accepted (below). |
| `ci-red` | CI is not green. |
| `ci-missing` | No CI run counts for the commit. |
| `ci-evidence` | A finding about a CI run that the evidence names. |
| `ci-unreadable` | CI cannot be read. The text of older builds (`CI cannot be read: ...`) has this class too. |
| `ci-none-undeclared` | The evidence says `ci: none`, the ticket does not. |
| `local-gate` | A local gate without its line, or on a board that needs CI. |
| `pr-closed` | The PR is closed without a merge. |
| `pr-repo` | The PR is not in a repo of the initiative. |
| `other` | A text that this build does not know. |

A fail with failures of two classes is in both classes and counts as one fail.

A fail is `accepted-after-head-moved` when both of these hold:

1. `head-moved` is its only class. A fail that also has a review text or another machine
   class stays rework.
2. The next verify result of the ticket is a pass, or the ticket closes as merged
   (`merge`, `close`, `close --verified`), and no other verify fail comes in between. A `done`
   in between does not matter: the dev agent runs it again for the new head.

Then nobody had to change the work: the head moved because somebody updated the branch, and
the change was accepted as it was. This is an artifact of the flow, not rework. A fail that
another fail follows, and a fail of a ticket that is dropped or still open, stays
`head-moved`. The report shows the fails with and without the accepted ones, and the rounds
leave them out.

## Exit codes

0 with a report. 1 when no ticket is in the period ([exit codes](../../reference/exit-codes.md)).
A bad `--since` is a usage error (64), before any request. A comment list over the page limit
is a usage error too ([Cost](#cost)).

## Cost

`retro` sends at most 5 requests for the login of the token, the board, `AUTHORITY.md`, the
registry issue (one more for each 100 comments after the first 100) and the agent items. Then:

- Without `--since`: one request for each 100 comments of each repo.
- With `--since`: one request for each 100 comments of each repo that changed since that time,
  and one request for each 100 comments of each issue that has a crew message among them.

An edited comment of the owner account costs one more request for its editors, as in every
command that reads comments.

One list read follows at most 50 pages, that is 5000 comments. When a repo has more comments,
`retro` without `--since` prints no report: a part of the log would give wrong numbers. It ends
with a usage error (exit 64) that names `--since`, because the fix is another command line and
not a retry. The same error tells you to pass a later `--since` when more than 5000 comments
changed since that time, or when one ticket has more than 5000 comments.
