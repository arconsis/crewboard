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
- A `done` counts only from the assignee at that time, and only with evidence.
- A ticket is an issue with a signed assignment. Any other issue counts nowhere.

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
| Volume | The first and the last message time, the tickets with an assignment, the delivered tickets (at least one `done`). |
| Verify | The tickets with a verify result. The tickets whose first result is a pass, with their share; when accepted fails exist (below), a second figure counts a ticket whose first result is an accepted fail as passed. The tickets with a fail and the number of fails, each with and without the accepted fails. |
| Verify fails by class | For each class: the count and the fails (ticket and URL). The text lists the latest 5 fails of a class and then the line `... and N more (--json lists all)`. The same limit holds for every list of the text. `--json` lists all. |
| Rounds | The 5 tickets with the most fails that are rework. |
| Andon | The questions to the owner, to the coordinator and to another agent. The `blocked` and the parked messages. The question to the owner that is still open, for each ticket that has not ended. |
| Refusals mentioned | For each [refusal id](../../reference/refusals.md) that a dev agent names as a whole word in a `question`, `blocked` or `report` message: the count and the messages. A refusal of the CLI is not on the board, so this is its only trace. |
| Flow | The reassigned tickets, the dropped tickets, the closes by kind, the late notes. The median and the 90th percentile of the hours from the first assignment to the first `done`, and from the first `done` to the close. |

The median of an even number of values is the mean of the two middle values. The 90th
percentile is the value at position `ceil(0.9 n)` of the sorted values. A close is counted for
a ticket that the log still shows as ended; a ticket that the owner moved back is open.

### Classes of a verify fail

`verify` writes each `failed:` line of a result from a fixed text, and `retro` reads the class
from the same text ([verify_fail.rs](../../../crates/crewboard-protocol/src/acts/verify_fail.rs)).
A new failure of `verify` does not compile without a class.

| Class | The failure |
|---|---|
| `review` | The Gate check of the coordinator failed (`verify --fail`): free text. A fail without a `failed:` line is `review` too. |
| `head-moved` | The PR head is not the commit of the evidence. |
| `accepted-after-head-moved` | A `head-moved` fail that was accepted (below). |
| `ci-red` | CI is not green. |
| `ci-missing` | No CI run counts for the commit. |
| `ci-evidence` | A finding about a CI run that the evidence names. |
| `ci-unreadable` | CI cannot be read. |
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
A bad `--since` is a usage error (64), before any request.

## Cost

`retro` sends at most 5 requests for the login of the token, the board, `AUTHORITY.md`, the
registry issue (one more for each 100 comments after the first 100) and the agent items. Then:

- Without `--since`: one request for each 100 comments of each repo.
- With `--since`: one request for each 100 comments of each repo that changed since that time,
  and one request for each 100 comments of each issue that has a crew message among them.

An edited comment of the owner account costs one more request for its editors, as in every
command that reads comments.
