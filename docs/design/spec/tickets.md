# Tickets

Part of the [crewboard specification](README.md).

Each move of a ticket by an agent leaves a signed message on the ticket. The signed log is the
proof, and the board field `Status` is a view of it.

## Ticket content

A ticket is an issue with the label `crew:work` that is on the board. `crewboard ticket new
--template` prints the body. It reads no config and writes nothing, so a headless coordinator may
run it. The optional lines are HTML comments, so an unchanged template sets none of them.

```text
Goal: <one or two sentences>
Gate: <required: one line that can fail>
<!-- CI-only ticket: write the Gate line as: Gate: ci -->
Scope: <what is in scope, and what is not>
Pointers: <files, issues, earlier PRs>
<!-- optional: Needs: x86_64, x86-builds -->
<!-- optional: Route: owner-workflow -->
<!-- optional: CI: none -->
```

- `Gate:` is required. The `Gate` field holds it too. `Gate: ci` (`ticket new --gate-ci`) is the typed CI-only Gate, the only Gate that the
  [headless coordinator](agents.md#headless-coordinator) merges.
- `Needs:` names capabilities, like labels `needs:<tag>`
  ([Requirements and matching](profiles.md#requirements-and-matching)).
- `Route: owner-workflow` marks the [owner route](#workflow-files-the-owner-route).
- `CI: none` declares a ticket without CI ([CI proof](ci.md#ci-proof)).

The issue body is not signed. So the signed assignment carries `gate: <text>` (all `Gate:`
lines, joined with ` | `) and `ci: none` or `ci: required`. `done`, `verify`, `merge` and the
headless "CI only" check read these lines, not the body; without a `ci:` line the body decides.
`reassign --to` copies both lines forward.

## Status flow and writers

The statuses are Backlog, Ready, In Progress, Review, Blocked, Done and Dropped
([Boards of earlier builds](board.md#boards-of-earlier-builds) for older options). Done means a
PR merged at a verified head, or merged by the owner. Dropped means that the coordinator ended the ticket by judgment,
without a merged PR. The writers
are the owner, the coordinator and the assignee ([Assignment](#assignment)).
`check_status_transition` in `src/rules/status.rs` judges every move. The allowed moves:

| From -> to | Writer | Command | Signed proof |
|---|---|---|---|
| new -> Backlog | coordinator | `ticket new`, `ticket adopt` | creation `info` |
| Backlog -> Ready | coordinator | `assign` | assignment `request` |
| Ready -> In Progress | assignee | `take` | `ack` |
| In Progress -> Review | assignee | `done` | `done` with evidence |
| Review -> Review | assignee | `done` with a new head SHA | `done` with evidence |
| Blocked -> Review | assignee | `done` on a [split](#ticket-split) ticket | `done` with evidence |
| In Progress -> Blocked | assignee | `block`, `ask`, `take --interrupt` | `blocked`, or `question` to the owner |
| Blocked -> In Progress | assignee | `take` after an answer or its own park | `ack` |
| Review -> In Progress | coordinator | failed `verify`, agent idle | `verify: fail` report |
| Review -> Ready | coordinator | failed `verify`, agent busy | `verify: fail` report |
| Review -> Done | coordinator | `merge` after a passed `verify`, or `close --verified` for a PR merged at the verified head | close `info` |
| any but Done and Dropped -> Done | coordinator | `close` after an owner merge | close `info` |
| any but Done and Dropped -> Dropped | coordinator | `close --why` | `dropped: <reason>` |
| any but Done and Dropped -> Ready | coordinator | `reassign --to <agent>` | `Reassigned.`, then an assignment |
| any but Done and Dropped -> Backlog | coordinator | `reassign` | `Reassigned.` |
| any -> any but Done and Dropped | owner | `crewboard move` | owner comment `/crew move` |

Done and Dropped are final: only the owner moves a ticket out of them. "Agent busy" means that
the agent has another ticket In Progress.

`ticket::log::fold_ticket` reads the signed messages in order and gives the status and the
assignee that the log supports. A `question` of the assignee to the owner allows the status
before or Blocked. A coordinator close gives Done, a `dropped: <reason>` note gives Dropped. The
note `closed: <reason>` of an older build gives Done, so the history of a board stays valid.
After a coordinator close or drop, a message of the assignee changes nothing, until an owner move
or a later coordinator creation info, assignment, reassign or failed verify. `status` and `audit`
report a board that the log does not support.

### The status table

`src/rules/status/table.txt` is the full matrix and the source of truth. A test in
`src/rules/status/table.rs` renders every move under named fact profiles and compares. A line is `<profile>|<from>|<verdicts>`, where `from` is `new` or a status. The 21
verdicts are the targets Backlog, Ready, In Progress, Review, Blocked, Done and Dropped, each for
the actors owner, coordinator and dev. A verdict is `ok` or a refusal code.

| Profile | Facts |
|---|---|
| `none` | no fact holds |
| `all_ok` | every fact holds: Gate set, active dev agent, actor is the assignee, evidence complete, verify passed, reassign reason, answer after the block |
| `verify_unset`, `verify_failed` | as `all_ok`, but no verify, or a failed one |
| `verify_failed_busy` | failed verify, and the agent is busy |
| `failed_busy_not_justified` | as `verify_failed_busy`, without a reassign reason |
| `no_answer`, `parked_no_answer` | no answer after the block; the second is a park of the actor |
| `not_assignee`, `no_gate`, `no_active_dev`, `no_evidence`, `not_justified` | as `all_ok`, without that fact |
| `split_proven*`, `new_head*` | the split proof holds, or the `done` names a new head; with `_no_evidence` or `_not_assignee` one fact more fails |
| `reason_*` | a move to Done by `verified_close`, `verified_merge` or `owner_merged`; `verified_close_unverified` lacks the verify |

A move to Dropped needs no fact besides the writer, so every profile gives the same Dropped
column.

The owner passes every move except a move to the current status. `crewboard move` adds its own rules
([Owner move](#owner-move)). The refusal codes:

| Code | Meaning |
|---|---|
| `status-wrong-writer` | The actor is not the writer of this move. |
| `status-unknown-transition` | The move is not in the table. |
| `status-no-change` | The ticket has the target status already. A repeat `close`, `reassign` or `move` is safe. |
| `status-done-final` | The ticket is Done. Only the owner moves it. |
| `status-dropped-final` | The ticket is Dropped. Only the owner moves it. |
| `status-gate-missing` | No Gate: Backlog -> Ready, `ticket new`, `assign`. |
| `status-agent-not-active-dev` | `Agent` is no approved dev agent in State `active`. |
| `status-answer-missing` | Blocked -> In Progress without an answer, and no park of the actor. |
| `status-evidence-missing` | A move to Review without complete evidence. |
| `status-verify-required` | Review -> Done without a passed verify. |
| `status-verify-failed-required` | Review -> In Progress without a failed verify. |
| `status-verify-failed-agent-busy` | Review -> In Progress for a busy agent. |
| `status-verify-requeue-agent-idle` | Review -> Ready for an idle agent. |
| `status-reassign-reason` | A reassign without a reason. |

### A question to the owner

"Waiting for the owner" is a question, not a status. `ask` posts a signed `kind=question` to the
owner and sets the label `needs-owner`. The assignee or the coordinator asks about a ticket that
has not ended (`status-done-final`, `status-dropped-final`). Another dev agent gets
`ask-not-assignee`. An In Progress ticket of the assignee
moves to Blocked, so the agent is free. Any other status stays, for example Review.

A question is open while it is the latest `question` of the assignee or a coordinator to the
owner, and no owner message and no coordinator `answer` with `withdraws: question` follows it. An
owner message is a comment of the owner account without a header.

### Blocked and the answer

The assignee resumes a Blocked ticket with `take` when an owner message or a coordinator
`kind=answer` follows the latest `blocked` or `question` message. A question to the owner needs
an owner message or an answer with `withdraws: question`. A coordinator `kind=info` is no answer.
A park of the agent itself needs no answer.

`crewboard unblock <issue>` (coordinator) posts the answer that `take` needs, with
`withdraws: question` when a question to the owner blocks the ticket. It writes no status. Its
refusals: `unblock-coordinator-only`, `unblock-needs-blocked`, `unblock-no-assignee`, and
`unblock-not-effective` when the answer would not satisfy `take`.

### One ticket In Progress per agent

`take` of a Ready ticket, and the resume of a Blocked
one, is refused with `take-agent-busy` while another ticket of the agent is In Progress. `take`,
`block`, `ask` and `done` write no agent State. A new ticket needs an `active` agent (`agent-paused`,
`agent-not-active`). A resume passes in every State.

**Park.** `take <n> --interrupt --why "<why>"` takes a ticket that is ahead of the ticket In
Progress: a strictly lower `Order` (`interrupt-not-ahead`; a missing `Order` is never ahead). An
answered Blocked ticket of the agent is a target too; any other is
`interrupt-needs-ready-ticket`. All checks run before the first write. Then the current ticket
gets a signed `blocked` that starts with `parked for #<n>: <reason>` and moves to Blocked, and
the new ticket is taken. `mine` lists a parked ticket above the Ready queue.

With more than one ticket In Progress, `--park <m>` names the ticket to park
(`interrupt-park-ambiguous`, `interrupt-park-not-in-progress`). With none, the refusal is
`interrupt-nothing-to-park`. `status` lists such agents in `multiple_in_progress`.

### Reassign

`reassign <n> (--reason <word> [--why "<text>"] | --derived) [--to <agent>]` (coordinator) takes
a ticket from its agent. The word is `dead`, `stopping`, `late` or `blocked-too-long`.
`--derived` reads the heartbeat and the registry log, not the board `State`, and is refused with
`status-reassign-not-justified` unless the agent is dead or stopping. With `--to`, the ticket
goes to Ready for that agent, after the checks of `assign`. Without it, `Agent` is cleared and the
ticket goes to Backlog: a ticket in Ready always has an agent. The coordinator duty returns the
tickets of a dead or stopped agent the same way.

The notice to the old agent starts with `Reassigned. Reason: <sentence>[: <detail>]`. A reassign
takes an open question back: one `answer` with `withdraws: question` to the old agent, and one
`info` to the owner as the record. The label `needs-owner` goes.

### Owner move

`crewboard move <issue> <status> [--why <text>]` runs with the owner's token
(`owner-command-not-owner`). It posts the owner comment `/crew move <status>` with the reason on
the next lines. Then it reopens a closed issue, sets `Agent` to the signed assignee if it differs,
and writes `Status`. The comment puts the move into the signed log. A drag on the board has none, so
`status` and `audit` report a drag.

- Ready, In Progress, Review and Blocked need a signed assignment (`move-needs-assignee`).
  Backlog keeps `Agent` as it is.
- Done is refused (`move-done-needs-close`): the coordinator posts the signed close.
- Dropped is refused (`move-dropped-needs-close`): the coordinator drops the ticket with
  `close --why` and a signed reason.
- A Done or Dropped ticket moves out only this way, for example back to Ready.
- A name that is no status is `move-bad-status`. Case does not matter; `-` or `_` is a space.
- A target that the board and the log both show is no change. A target that only the board
  shows gets the comment and no `Status` write.

Only a comment of the board's `owner_login` counts, and only if no other account edited it
([Edited owner comments](security-details.md#edited-owner-comments)). It also answers an open
question to the owner. The move keeps the assignee.

## Assignment

The assignee is the agent of the latest signed assignment of a coordinator. A later
`Reassigned.` notice ends it. The board `Agent` field is a view. `take`, `done`, `block`, `ask`,
`unblock`, `verify` and `merge` refuse with `ticket-agent-not-signed` when `Agent` names another
agent than the log. An empty `Agent` passes. `assign` on a Ready ticket, or `reassign`, repairs
it.

`assign <issue> --to <agent> [--order N]` (coordinator) moves a ticket to Ready. It posts the
signed assignment (`gate:`, `ci:`, `order:`) and sets `Agent`, `Order` and the agent label. The agent
must be an active dev agent. A busy agent gets the ticket in its queue. A late or dead agent is `status-agent-not-alive`. Missing
capabilities are `capability-mismatch`; `--override-capabilities` with a reason passes.

`status` warns about a Ready ticket without an Agent (`ready_warnings`) and about a paused agent
with Ready tickets. The coordinator cycle reports the second as `agent-paused-ready`.

### Queue order

`Order` is a number field: the position in the queue of the agent, lowest first. Only the
coordinator and the owner write it ([Fields](board.md#fields)). `init` or the first `assign`
creates it.

- The next ticket is the lowest `Order` among the agent's Ready tickets, then the lower issue
  number. A ticket without `Order` comes last. `mine` lists In Progress first, then Ready.
- `assign` without `--order` appends: the highest `Order` of the agent's open tickets plus one.
  `--order` takes 0 to 1000000 (`order-invalid`).
- A failed `verify` of a busy agent writes the lowest Ready `Order` minus one, which can be
  below 0.
- `assign <n> --order N` on a Ready ticket with an agent sets its position and posts nothing
  (`reorder-not-queued`, `field-wrong-writer`). `--to` its own agent without `--order` is
  `ticket-already-assigned`. Neither flag is `assign-needs-to`.
- `assign <n> --to <other>` on a Ready ticket moves it to another queue, with one assignment
  message that has `moved-from: <old agent>` (`rebalance-not-ready`, `rebalance-no-agent`,
  `rebalance-same-agent`).
- `status --queue` shows the queues, and a note when one agent has at least 3 more Ready
  tickets than a live agent that can take the same work.

### Agent labels

GitHub's own views do not show `Agent`. So `assign` and `reassign` set the label
`agent: <name>` and remove the old one; `close` keeps it. The label `needs-owner` marks an open
question to the owner. `ask` and an owner-route `done` set it. `take`, `verify`, `merge`,
`reassign` and `close` remove it when no question is open, and the coordinator's `poll` or
`watch` removes a stale one. crewboard never reads a label as a fact. A failed label write is a
warning.

## Ticket creation

`crewboard ticket new --title T --body-file F [--repo R] [--backlog | --assign <agent>]
[--continue-pr <url>]` (coordinator) checks the `Gate:` line, the size, the secret scan and
`coordinator_may_create_tickets` (`authority-no-ticket-creation`). With `--assign` it runs the
checks of `assign` first, so a refusal creates nothing. Then it creates the issue with the label
`crew:work`, adds the board item, posts the signed creation `info` with the item id
([Item ids are the source of truth](board.md#item-ids-are-the-source-of-truth)), and sets `Kind` Work,
`Status` Backlog and `Gate`.

`ticket adopt <issue> [--assign <agent>]` does the same for an open `crew:work` issue. It keeps
the Status of a ticket on the board, so it repairs a ticket added by hand. A closed issue is
`ticket-closed`.

## Evidence

A `kind=done` message has the lines `pr` (a PR in a repo of the initiative), `sha` (its head) and
`ci` (a [CI proof](ci.md#ci-proof) for that SHA). crewboard checks the PR, its head and the CI
proof against GitHub, and refuses `done` otherwise. A claim such as "pass" is never evidence.

A re-delivery from Review needs a new head SHA. The checks are the same, and the Status stays
Review.

## Verification by the coordinator

`verify <ticket>` (coordinator, `verify-coordinator-only`) needs a ticket in Review
(`verify-needs-review`) with a signed `done` (`verify-no-evidence`) and no open question to the
owner. It checks that the evidence is still valid (head not moved, CI green) and that every
required check is `SUCCESS` by conclusion. Then the coordinator checks the Gate itself, from the
run log or by a rerun. crewboard records the result.

A `verify` without `--fail` reads the file list of the PR before the CI. A change to a protected path
([Authority](board.md#authority)), a rename out of one, or more than 1000 files, is
`protected-path-owner-merge`. `close --verified` and `merge` apply the same rule. The refusal
posts nothing. The owner merges by hand, then `close <n>`
([Close after an owner merge](#close-after-an-owner-merge)).

With `ci_mode: local`, a local-run `done` needs `verify --local-run "<what you checked>"`
([Local run](ci.md#local-run-ci_mode-local)).

A passed `verify` writes no status. `merge` follows ([Merge](#merge)). `close <n> --verified`
is for a PR that is merged already, for example by auto-merge or by a maintainer. It reads the PR
first and needs it merged at the `head_now` of the passing verify. An open PR is refused with
`close-verified-not-merged`, and the text names `merge`. A PR closed without a merge, or merged at
another head, is refused with `close-verified-head-moved`. It posts the signed note before it
writes Done, so a rerun finishes a failed write.

A failed `verify` moves an idle agent's ticket to In Progress. A busy agent's ticket goes to
Ready at the front of its queue, and the report has a `case:` line.

Of the ticket commands, a headless coordinator runs only `verify`, `merge` of a CI-only ticket,
and `close` after an owner merge. It refuses the others with `headless-scope`, and it never acts
on a ticket with an open question ([Headless coordinator](agents.md#headless-coordinator)).

### Failed Gate check

The evidence can be complete while the coordinator's own Gate check fails. The coordinator runs
`crewboard verify <n> --fail --body "<what the Gate check found>"`. The note is one more failure,
and the report says `verify: fail`. The writes are those of any failed `verify`. An empty note is
`verify-fail-needs-note`. An open question does not stop `--fail`.

### Verify after the owner's go

A dev agent may run `done` and then `ask` the owner for a go. The ticket stays in Review with the
label `needs-owner`. `verify`, `merge` and `close --verified` refuse it with
`verify-owner-question-open`. The owner answers with a comment on the ticket; a chat answer is not
signed. Then `verify` judges as usual. The coordinator can also take the question back with
[`reassign`](#reassign).

### Close after an owner merge

The owner may merge a crew PR by hand. `crewboard close <n>` (coordinator) then moves the ticket
to Done from any status but Done and Dropped when all of these hold:

1. The issue is closed.
2. A PR in a repo of the initiative is merged, and its body closes this issue.
3. The merge actor is the board's `owner_login`.

The PR comes from the latest `done` of the assignee, else from the cross-references of the issue
(at most 8 PRs).

On the owner route, only this proof counts: the PR of the latest `done` (the one with
`owner-route:`) contains `docs/patches/<n>-workflow.patch` of this issue, is in a repo of the
initiative, is merged by the owner, and the workflow change is applied. Applied means that the PR
changed a file under `.github/workflows/`, or that the base branch has every hunk of the patch
(at most 5 files). A patch that cannot be read counts as not applied. The issue may be open, and
`close` then closes it.

Writes, in order: the [merged-without-verify note](#a-pr-merged-without-a-passed-verify) if
needed; one signed `info` with `pr:`, `merge-commit:` and `closed: owner merged <url>`; Status
Done; the label `needs-owner` removed. Without an owner merge and without `--why`, `close` is
refused with `close-needs-reason`, which names the failed condition. With `--why` it becomes a
[close without a PR](#close-without-a-pr): Dropped, not Done. A Dropped ticket is not closed by
an owner merge (`status-dropped-final`).

The headless `tick` does this close too, for tickets in Review, In Progress, Blocked and Ready
whose issue is closed.

### A PR merged without a passed verify

crewboard does not refuse a merge outside crewboard, but it makes the gap visible. The gap: the PR of the ticket is merged, and no `verify: pass` names its
head in `head_now:`. Then the coordinator posts one signed `info` that starts with
`merged without verify`, with the lines `pr:`, `merge-commit:`, `merged-by:`, `head:` and
`last-gate:`. It posts once per merge commit. `tick` posts it for Review tickets, and `close`
after an owner merge. `status` marks the ticket ([`status`](cli.md#status-the-pr-of-a-ticket)).
The note changes no Status.

### Close without a PR

`crewboard close <n> --why "<text>"` drops a ticket that is not Done or Dropped by the
coordinator's judgment, for example when a new ticket supersedes it. Status Dropped, never Done:
Done always means a merged PR. It applies only when `close` finds no owner merge; with an owner
merge the reason is a line of the close note, and the ticket is Done. The reason is one line and
passes the body check of every post (`body-too-long`, `secret-in-body`). `--why` and `--verified`
do not combine. A repeat on a Dropped ticket is `status-no-change`.

The board needs the Status option Dropped. A board that `init` made before the option existed
lacks it: `close --why` is refused with `dropped-option-missing` before any write, and the text
says to run `crewboard init` again. The rerun adds the option and keeps every other option id.

Writes, in order: one signed `info` that starts with `dropped: <reason>`, Status Dropped, the
label `needs-owner` removed. The issue stays open. The note `closed: <reason>` of an older build
still reads as Done.

### A PR merged while the dev works

A push to the branch of a merged PR goes nowhere. So `done` refuses a merged PR with
`pr-already-merged` before any write, and the dev opens a new PR from the default branch. For a
ticket In Progress, `tick` reads the PR in the `Evidence` field. When it is merged, `tick` posts
one signed `info` to the assignee, once per merge commit. It changes no Status.

### Work that an earlier agent left

Before the notice, every reassign reads
the open PRs for the branch `<dev_branch_prefix><old agent>/<n>-<slug>`, else the refs of that
prefix. The notice then has the line `open-pr: <url> branch: <branch>` or
`earlier-branch: <branch>`. A failed read never blocks the reassign. `assign`, `reassign --to`
and `take` print the earlier PR from the latest `Reassigned.` notice of a coordinator.

`take <n> --continue-pr <url>` (first take of a Ready ticket, `adopt-pr-needs-ready-ticket`)
adopts that PR. It checks before it writes: same repo (`adopt-pr-other-repo`), named by the latest
notice (`adopt-pr-not-recorded`), open (`adopt-pr-not-open`), a crew branch of this ticket
(`adopt-pr-bad-branch`), and a title or body that mentions the ticket (`adopt-pr-not-linked`).
The ack carries `adopts-pr: <url> branch: <branch>`. `done` and `merge` then accept that branch.
The PR body must still close the ticket (`evidence-pr-not-linked`).

### A follow-up ticket continues an open PR

A follow-up ticket can continue the open PR of a Done or Dropped ticket on its branch.
`ticket new ... --continue-pr <url>` checks, in order: a
PR URL (`evidence-pr-url`) of a repo of the initiative (`repo-not-in-initiative`) that exists
(`evidence-pr-missing`); open; a crew branch `<dev_branch_prefix><agent>/<n>-<slug>`; and ticket
`<n>` is Done or Dropped, or in Backlog or Ready with a reassign notice that names this PR
(`adopt-pr-parent-not-done`). The status of ticket `<n>` is the one of its signed log, as
`status` and `audit` judge it. A board Status alone does not count: the crew token can write it.
To end the old ticket first, run `close <n> --why "<text>"`. A board that shows Done or Dropped
without a signed end (set by hand) makes `close --why` refuse too (`status-no-change`): the
owner first moves the ticket back in the signed log (`crewboard move <n> Backlog`), then
`close <n> --why` drops it. Both refusals name this way. The signed creation `info` of the new ticket gets
`adopts-pr: <url> branch: <branch>`. Only that coordinator line lets `done` and `merge` accept
the branch. The PR body must close the new ticket.

## Branches

The branch of a ticket is `crew/<agent-name>/<issue-number>-<slug>`, where `crew/` is the default
`dev_branch_prefix`. The slug has only `A-Za-z0-9._-`, at most 100 characters, no leading `-` or
`.`, and no `..`. `done` refuses any other branch with `evidence-branch-name`, except an adopted
branch. crewboard quotes a branch name for the shell and encodes it in URL paths. The PR body
says "Closes #<n>" or the cross-repo form. No agent pushes to a protected branch.

## Merge

`crewboard merge <issue> [--method squash|merge|rebase]` (coordinator). The checks, in order:

1. The ticket, as for `verify`: Review, no open question, no owner-route `done`.
2. `AUTHORITY.md`: `merge` in `needs_owner` is `authority-needs-owner`, and
   `coordinator_may_merge: false` is `authority-merge-not-allowed`
   ([Contradicting keys](board.md#contradicting-keys-in-authoritymd)).
3. The protected paths, after the PR read and before any CI read. The refusal names
   `crewboard close <n>`.
4. The evidence: a passed `verify` of the latest `done` (`merge-verify-required`); the PR of
   that `done`, open (`merge-pr-not-open`), in a repo of the initiative, and closing the ticket;
   its head at the evidence SHA (`evidence-head-moved`); green CI, or `CI: none`. A local-run
   `done` passes only with `ci_mode: local` and a coordinator `coordinator-local-gate:` line on
   the same head.
5. The mergeable state. `clean`, `has_hooks`, `unstable` and `blocked` go on. `unknown` is read
   up to 4 times, 2 s apart. Any other state is `pr-not-mergeable`. For `draft` the text asks to
   mark the PR ready. Else it names the dev's next command,
   `crewboard done <n> --pr <url> --sha <new head>`. On `dirty`, `merge` first posts one signed
   `request` to the assignee to rebase.

The merge is the first write, because it cannot be undone. The call carries the verified SHA, so
GitHub refuses a moved head (`merge-head-moved`). A forbidden method is `merge-not-allowed`.
GitHub's "not mergeable" answer maps to the same refusal as step 5. Then crewboard deletes the
PR branch (same repo, under `dev_branch_prefix`, not protected), writes Done, and posts a signed
`info` to the dev. A failure after the merge is `merge-done-ticket-open`; the repair is
`close <n> --verified`. Every other refusal leaves the PR and the board as they were.

## Ticket split

One ticket is one PR. A ticket with independent causes would block its finished part, so
`crewboard ticket split <issue> --title T --body-file F` (coordinator) moves the open part to a
new ticket. The old ticket must be open, In Progress or Blocked (`split-not-in-progress`), with
an Agent (`split-no-agent`). `coordinator_may_split_tickets: false` refuses it
(`authority-split-not-allowed`). The new ticket must pass the checks of `ticket new` and `assign`.
A refusal writes nothing. Then:

1. The new ticket gets the file text and `Split from: <repo>#<n>`, and the old Gate if the file
   has none.
2. The old body gets one line `Split off: <repo>#<new> - <title>`.
3. The new ticket goes into the queue of the same agent.
4. A signed note on the old ticket says where the open part went and to run `done` on the old
   ticket for the finished part.

The old ticket keeps its status. A Blocked old ticket goes to Review with `done`, without a
`take`. The proof is the `Split off:` line and the signed split note of a coordinator together;
anyone who can edit the issue can write the line alone. Every step is idempotent: a rerun finds
the new ticket by the `Split off:` line or by its title and `Split from:` line, and runs only the
missing steps. Another title is another split.

## Workflow files: the owner route

A crew token has no `workflow` scope: a token that can write workflows can read the secrets of
the repository. So a dev agent delivers a change to `.github/workflows/**` as a patch.

1. **Mark.** The body line `Route: owner-workflow` marks the route; a `Scope:` line does not.
   `assign` adds `route: owner-workflow` and `patch: docs/patches/<issue>-workflow.patch` to the
   signed assignment.
2. **Delivery.** The dev agent pushes everything but the workflow files, and puts the workflow
   change into the PR as `docs/patches/<issue>-workflow.patch`.
3. **Route of `done`.** Workflow files in the PR: normal route. The patch file in the PR: owner
   route, marked or not. Marked but neither: `workflow-needs-owner`, and the text says not to
   block.
4. **No closing keyword.** On the owner route, `done` refuses a PR whose title or body closes
   this ticket (`owner-route-closing-keyword`). A merge of the patch-only PR would close the ticket
   while the workflow is unchanged. The PR says `Refs #<n>`. A keyword is `close`, `fix` or
   `resolve` in any form and case, with an optional colon, before `#<n>`, `<owner>/<repo>#<n>` or
   the issue URL.
5. **Record.** `done` posts the `done` with `owner-route: <patch>` and one `question` to the owner
   that starts with `owner-route: <patch> sha=<head>`. It sets `needs-owner` and moves the ticket
   to Review. The question holds the commands to apply the patch on the branch, and says not to
   merge the PR as it is. A second `done` on the same head sends no second question.
6. **End.** The owner applies the patch, pushes, answers, and merges by hand. Then the coordinator
   runs `crewboard close <n>` ([Close after an owner merge](#close-after-an-owner-merge)).
   `verify`, `merge` and `close --verified` refuse an owner-route `done` with
   `verify-owner-route`.

**Safety net.** The owner may merge the patch-only PR before the patch is applied. A Done ticket
is a finding when the latest `done` is on the owner route with the patch of this ticket, its PR is
merged, the content check of [Close after an owner merge](#close-after-an-owner-merge) fails, and
no answer follows an `owner-route-merged` question for that PR. Any owner message settles it.

The interactive coordinator's `poll` and `watch` then post one `question` to the owner that
starts with `owner-route-merged: <PR URL>`, reopen the issue, and set `needs-owner`. A step that is
in place is skipped. The Status stays Done; the owner moves the ticket
([Owner move](#owner-move)). The headless `tick` only logs the finding. At most 10 Done tickets
are judged per run, and the state cache skips a judged ticket until its board item changes.
