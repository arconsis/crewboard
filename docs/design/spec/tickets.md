# Tickets

Part of the [crewboard specification](README.md).

## Ticket content

A ticket is a normal issue with label `crew:work`, added to the board. The body has:

- **Goal:** one or two sentences.
- **Gate:** one line that can fail, for example "`make test` passes on CI and
  `crewboard agent selftest` reports a planted event". Also stored in the `Gate` field. The Gate of a
  ticket that needs a green CI run and nothing else is `Gate: ci` (`ticket new --gate-ci` writes
  it): the typed CI-only Gate, the only one that the no-model coordinator merges
  ([CI only, exactly](agents.md#headless-coordinator)). Any other text is a free-text Gate.
- **Scope / not in scope.**
- **Pointers:** files, issues, earlier PRs.
- **Needs:** (optional) the capabilities that an agent needs for this ticket, one line, for
  example `Needs: x86_64, x86-builds`. The same can be given as labels `needs:<tag>`.
  [Requirements and matching](profiles.md#requirements-and-matching) defines the matching; `assign` enforces it.

`crewboard ticket new --template` prints this body. It reads no config, calls no board and writes
nothing, so a no-model coordinator may run it. Only `Gate:` is checked by `ticket new` and
`assign`. The optional lines are HTML comments: a template that is used as it is does not set
`Needs`, `Route` or `CI`.

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

`assign` refuses a ticket without a Gate (*loud*).

The assignment message is signed and carries the Gate as the line `gate: <text>` (all `Gate:`
lines of the body, joined with ` | `; one line gives that line). The issue body is not signed:
any holder of a crew token can edit it. So a judgement that must hold reads the signed line.
The headless "CI only" check is one ([No-model coordinator](agents.md#headless-coordinator)).
`reassign` copies the latest signed `gate:` line into its new assignment message. The latest
`gate:` line of a coordinator `request` is the signed Gate; a `gate:` line of a dev is ignored.

The same message carries `ci: none` when the body declares `CI: none`, else `ci: required`.
`done`, `verify` and `merge` (and the headless "CI only" check) read this signed line, so a body
edit after the assignment cannot add or remove `CI: none`. `reassign` copies it forward too. A
ticket that was assigned before this line existed has no `ci:` line: the live body decides
for it.

The assignee is the agent of the latest signed assignment of a verified coordinator; a later
signed `Reassigned.` notice ends it, and a later assignment sets a new one. The board `Agent`
field is a view of it. `take`, `done`, `block`, `ask`, `unblock`, `verify` and `merge` refuse
with `ticket-agent-not-signed` when the board `Agent` is set and the signed log does not name
the same agent (an `Agent` written by hand, or left after a reassign): board fields are not
signed, and any holder of the crew token can write one. An empty `Agent` is no contradiction.
The refusal names the repair: `assign` counts an `Agent` field as an assignment only when a
signed assignment names it, so `assign <t> --to <agent>` on a Ready ticket, or `reassign`,
posts the signed message and sets the field.
`status` flags a `Status` that no signed message supports.

## Status flow and writers

| From -> to | Writer | Condition |
|---|---|---|
| (new) -> Backlog | coordinator or owner | |
| Backlog -> Ready | coordinator | Gate is set, Agent is set to an `active` dev agent |
| Ready -> In Progress | assigned dev agent | posts `kind=ack` |
| In Progress -> Review | assigned dev agent | posts `kind=done` with evidence ([Evidence](#evidence)) |
| In Progress -> Blocked | assigned dev agent | posts `kind=blocked` with the reason, or `ask` (it posts the question and also moves the ticket here), or the park of `take --interrupt` (below); the agent State is not written. `Doing` goes `idle` when no other ticket of the agent is In Progress, so it can take its next Ready ticket |
| Blocked -> In Progress | assigned dev agent | an owner message or a coordinator `kind=answer` was posted in the ticket after the latest `blocked` or `question` message. A `question` is answered by an owner message or by a coordinator `answer` with the line `withdraws: question`, not by another coordinator `answer`. Or the latest `blocked` message is a "parked for #<n>" message written by that same agent (park, below). While another ticket of the agent is In Progress, only `take <n> --interrupt` (park, below) resumes it, and only when it is ahead |
| (Blocked, no status change) | coordinator | `crewboard unblock <issue>` posts the signed `kind=answer` to the assignee, which is the message that lets the assignee `take` the ticket again. When the latest blocking message is a question to the owner, the answer also carries the line `withdraws: question` (a plain answer does not unblock such a ticket), and the output says `unblocked: the open question to the owner is withdrawn`; `unblock` refuses with `unblock-not-effective` when its message would not satisfy `take`. `say --kind answer` from the coordinator to the assignee of such a ticket warns on stderr and still posts, and the `take` refusal `status-answer-missing` names the missing owner comment or `unblock`. It refuses with `unblock-needs-blocked` for a ticket that is not Blocked and with `unblock-no-assignee` for one without an assignee; nothing is written. A coordinator `kind=info` does NOT unblock: `say --kind info` to the assignee of a Blocked ticket prints a warning on stderr (and still posts), and the `take` refusal `status-answer-missing` then names this case |
| Blocked -> Review | assigned dev agent | the ticket was split ([`ticket split`](#ticket-split)): its body has a `Split off:` line AND a signed split note of a coordinator names the same ticket; posts `kind=done` with evidence ([Evidence](#evidence)). No `take` and no answer are needed. It is not an In Progress ticket: the agent State and the other In Progress ticket stay as they are. Without the proof the move is refused (`status-unknown-transition`): `take` it first |
| Review -> Review (re-delivery) | assigned dev agent | after a branch update (for example after a `pr-not-mergeable` refusal of [`merge`](#merge)) the assignee runs `crewboard done <n> --pr <url> --sha <new head>`. The new head SHA is not the SHA of its latest `done`. The checks are those of `In Progress -> Review` ([Evidence](#evidence): PR, head, CI of the new head). crewboard posts the signed `kind=done` and writes the `Evidence` field. The Status stays Review: no Status write, and the agent State and Doing stay. The coordinator gets the `done` as an event, and `verify` judges this latest `done`. The same head SHA, another agent or the coordinator: `status-no-change` |
| Review -> Done | coordinator | `verify` passed ([Verification by the coordinator](#verification-by-the-coordinator)) |
| Review -> In Progress | coordinator | `verify` failed (also `verify --fail` after a failed Gate check, [Failed Gate check](#failed-gate-check)) and the agent has no OTHER ticket In Progress; posts what failed. With another ticket In Progress the move is refused (`status-verify-failed-agent-busy`): see the next row |
| Review -> Ready | coordinator | `verify` failed (also `verify --fail`) and the agent has ANOTHER ticket In Progress: same `Agent`, `Order` at the front of the agent's Ready queue ([Queue order per agent](#queue-order-per-agent): lowest `Order` of its Ready tickets minus 1), so the agent keeps one In Progress ticket ([rule of `take`](#queue-order-and-take)) and the next `take` returns to this ticket. The signed verify report has a line `case: verify failed; <agent> works on <other>; queued as next (Ready, Order <n>)`. Idle agent: `status-verify-requeue-agent-idle` |
| any but Done -> Done (owner merge) | coordinator; also the headless `tick` for Review, In Progress, Blocked and Ready | the owner merged the PR by hand and the issue is closed: by GitHub through the closing keyword, or by `close` itself on the owner route ([Close after an owner merge](#close-after-an-owner-merge)): `close <n>`. No `verify` is needed: the owner's merge ended the ticket. The label `needs-owner` goes |
| any but Done -> Done (by judgment) | coordinator | the coordinator judges that the ticket needs no merge, for example it has no PR to verify ([Close without a PR](#close-without-a-pr)): `close <n> --why "<text>"`. It ends any ticket that is not Done, with a signed reason. No `verify` is needed. The label `needs-owner` goes. Not for a no-model coordinator: it is a judgment |
| Done -> any | owner | Done is final. Every other writer is refused. `reassign`, `ask` and `assign` give `status-done-final`: `ticket is Done; only the owner moves it (open a new ticket, or ask the owner)`. `verify` on a Done ticket gives `verify-needs-review`, and `split` is refused with `ticket-closed` or `split-not-in-progress`. A repeat `close` is `status-no-change`. The safety net of the owner route reopens the issue of a Done ticket and asks the owner, but writes no Status ([Workflow files: the owner route](#workflow-files-the-owner-route)) |
| any but Done -> Ready (reassign `--to <agent>`) | coordinator | one reason of the list below: `dead`, `stopping`, `late` or `blocked-too-long`; a new agent is named (a ticket in Ready always has an agent) |
| any but Done -> Backlog (reassign without `--to`) | coordinator | the same reasons; the Agent field is cleared, and `assign` puts the ticket into Ready again |
| any -> any | owner | |

**A question to the owner.** "Waiting for the owner" is a question, not a status. `ask` posts a
signed `kind=question` to the owner and sets the label `needs-owner`. The assignee (or the
coordinator) asks about a ticket in any status but Done; another dev agent is refused with
`ask-not-assignee`. An In Progress ticket of the assignee also moves to Blocked (the agent is
free for its next ticket); a ticket in any other status keeps its status, for example a ticket
in Review after `done`. A question is open when it is the latest `question` to the owner on the
ticket and no later owner message (a headerless comment of the owner account) exists, and no
later coordinator `answer` carries the line `withdraws: question`. A ticket in the legacy status `Needs Human` reads as Blocked with an open question and ends through the owner
answer on the ticket, `take`, `done` and `verify`.

**Reassign.** `reassign <n> (--reason <word> [--why "<text>"] | --derived) [--to <agent>]` takes a
ticket from its agent (coordinator only). The reason is typed: one word of `dead` (no heartbeat),
`stopping` (the agent drains or has left), `late` (it does not answer) or `blocked-too-long`,
and an optional free-text `--why`. Any other `--reason` is a usage error that names the four
words, before any request. The notice to the old agent and the override record carry the
sentence of the word and the detail (`Reassigned. Reason: agent is dead: host crashed`);
`--derived` reads the heartbeat and the registry log and writes `dead` or `stopping` itself (a
board `State` alone does not count), and is refused with
`status-reassign-not-justified` when the agent is neither. **A ticket in Ready has an agent:**
with `--to` the ticket goes to Ready for that agent (the agent is assignable and matches the
capabilities, one new assignment message that copies the signed Gate); without `--to` the
Agent field is cleared and the ticket goes to Backlog, and the output says
`crewboard assign <ticket> --to <agent>`. The coordinator's own return of the tickets of a dead
agent, or of an agent that the registry log stops or ends, goes to Backlog the same way. A Backlog ticket is `status-no-change`
for `reassign` without `--to`. The wire format is `v=1`: the notice is the `Reassigned.` message, and the signed log
allows Backlog or Ready after it, until an assignment message sets Ready. A Ready ticket without
an Agent (a board from an older release) is a warning, not an error: `status` shows
`! warning: <ticket> is Ready without an Agent (an older reassign left it so)` and
`status --json` lists it in `ready_warnings`; `assign` repairs it.

**A paused agent with Ready tickets.** A paused agent takes nothing new, so its Ready tickets
wait. `status` shows `! warning: <agent> is paused and holds <n> Ready ticket(s): <tickets>`;
the coordinator cycle gives one `agent-paused-ready` event with the same text, again only when
the set of tickets changes (the last finding per agent is kept in the state file, like the
liveness findings). Fixes: `resume`, or move the tickets to another agent (`assign <n> --to <agent>`).

**Take a question back.** Every `reassign <n> ... --reason <word>` of a ticket with an open
question (to the same agent or to another one) posts ONE signed `answer` to the agent with the
reason and the line `withdraws: question`, and ONE `info` to the owner ("question on #n taken
back by the coordinator: reason", with an `event=` line that names the coordinator; this signed `info` IS
the record of the take-back: `audit` lists security findings only and does not list it). Then
the generic path of `reassign` moves the ticket to Ready (with `--to`) or Backlog and removes the label `needs-owner`.
The `reassign` checks run first: the agent must be assignable and match the capabilities
([Requirements and matching](profiles.md#requirements-and-matching)). The agent takes the ticket
with a normal `take`; a new agent asks again if the question still stands. Only a question of
the assignee or of a coordinator counts as open: a signed question of another dev agent opens
nothing and blocks nothing.

**Park (`take --interrupt`).** One In Progress ticket per agent stays the rule. A dev agent
whose new Ready ticket is strictly ahead of its In Progress ticket in its queue runs
`crewboard take <n> --interrupt --why "<why>"`. "Ahead" has one rule: the new ticket needs a
strictly lower `Order` than the ticket in progress (`interrupt-not-ahead`, *loud*; a missing
`Order` is never ahead). The coordinator makes a ticket urgent by putting it first
(`assign <n> --order <number>`). crewboard checks both
tickets, the rule and both transitions first and writes nothing if one check fails. Then it
moves the current ticket
`In Progress -> Blocked` with one signed `kind=blocked` message whose body starts with
`parked for #<n>: <reason>` (agent, label and Order stay), and takes the new ticket as `take`
does. A plain `take` while a ticket is In Progress is refused
and the message names `--interrupt`. After `done` or `block` of the interrupting ticket,
`take <parked>` brings the parked ticket back without an answer, because the latest `blocked`
message is a park by the same agent; a normal `blocked` message still needs the answer, and a
park message of another agent does not count. `mine` lists a parked ticket as
`parked for #<n>`, above the Ready queue.

`--interrupt` also takes an answered Blocked ticket of the agent: the agent blocked ticket A,
took ticket B, and the coordinator or owner answered A. Three checks, all before the first
write. The ticket is assigned to the agent. The answer counts as for a plain `take` (the
Blocked -> In Progress row above: answer after the block, the owner-question rule, the park
rule); without it the refusal of the plain `take` stays (`status-answer-missing`). And A is
ahead of the ticket in progress by the rule above (`interrupt-not-ahead`). Then B is parked as above and A goes Blocked -> In Progress
with the same `Resumed after an answer.` ack as a plain `take`. When A is not ahead, the agent
runs `block` on B first, then a plain `take`. Any other target that is not Ready is refused
with `interrupt-needs-ready-ticket` (the message says: needs a Ready ticket or an answered
Blocked ticket of yours).

**One In Progress ticket per agent: the board decides, not the State.** Every `take`
reads the own tickets of the agent. A take of a Ready ticket and the take-back of a
Blocked ticket are refused with `take-agent-busy` while ANOTHER ticket of the agent is In
Progress, whatever the agent State is. The message names that ticket. The agent State is not
written by `take`, `block`, `ask` or `done`: busy means that a ticket In Progress is assigned to
the agent. A new ticket needs an `active` agent (`agent-paused`, `agent-not-active`); the legacy
State `working` of an old board reads as `active`.
`--interrupt` is judged by the board too:

- one ticket In Progress: it is parked, as above;
- more than one (an old board, or a state made by an older version): `--interrupt` without `--park` is
  refused with `interrupt-park-ambiguous`; the message lists the candidates and names `--park`.
  `take <n> --interrupt --park <m> --why "<why>"` parks ticket `<m>` (`<n>` or
  `<owner>/<repo>#<n>`) and takes `<n>`. The other tickets stay In Progress; the agent finishes
  or blocks them;
- `--park` names a ticket that is not In Progress for the agent: `interrupt-park-not-in-progress`
  (the message lists the In Progress tickets);
- none In Progress: `interrupt-nothing-to-park`; use a plain `take`.

`status` prints a line `! <agent> has <n> tickets In Progress: <keys>` for each agent with more
than one, and `status --json` lists them in `multiple_in_progress`. The coordinator uses it to
repair old states.

crewboard refuses a transition by the wrong writer (*loud*). The owner can always move
anything; the next `watch` reports the change to the agents.

## Evidence

`done` refuses a PR that is already merged (`pr-already-merged`): [A PR merged while the dev works](#a-pr-merged-while-the-dev-works).

A `kind=done` message must contain all of:

- `pr`: PR URL in a repo of the initiative,
- `sha`: the PR head SHA,
- `ci`: the CI proof for that SHA. It is one of four kinds ([CI proof](ci.md#ci-proof)), and
  the line has the form of that kind: the URL of the CI run (`runs`), `ci: none` (only allowed
  if the ticket says so), `ci: none (all files paths-ignored: <patterns>)` or `ci: local-gate`
  with its lines (while `AUTHORITY.md` has `ci_mode: local`; the CI of the head is not judged).

crewboard checks that the PR exists, that its head is that SHA, and that the run
belongs to that SHA. It refuses `done` otherwise (*loud*). A claim in a message ("pass") is
never evidence: the run and its conclusion are.

## Verification by the coordinator

`verify <ticket>` runs on a ticket in Review without an open question to the owner
([Verify after the owner's go](#verify-after-the-owners-go)). It checks:

1. The evidence is still valid (the PR head did not move, the CI run is green).
2. Every required check of the PR is `SUCCESS` (by conclusion, never by grepping check
   names; a job may be called "fail").
3. The coordinator then checks the Gate itself: it reads the run log or reruns the
   command. This step is the model's work; crewboard records the result.

Only then: `Review -> Done` with
`close <n> --verified`. `merge` is the only command that merges, and it follows `AUTHORITY.md`.
`close --verified` is the exit when the coordinator may not merge.

`close --verified` pins the close to the head that the passing verify saw. It reads the PR before
any write. It is refused with `close-verified-head-moved` when the head of the PR is not the
`head_now` of the verify (somebody pushed after the verify; run `verify` again), and when the PR
is closed without a merge. A PR that is merged at the verified head passes. So Done after
`close --verified` means "accepted at head H". The merge may come later, by the owner. Done after
`close <n>` without `--verified` means the owner merged. The command posts the signed note first
and writes the Status last, as `close` does: a failed write leaves a Review ticket, and a rerun
finishes.

A verify PASS reads the current file list of the PR before it reads the CI. A PR that changes a
protected path (the built-in seven: `AUTHORITY.md`, `.crewboard.toml`, `.github/workflows/**`,
`.github/actions/**`, `.github/CODEOWNERS`, `CODEOWNERS`, `docs/CODEOWNERS`; and the globs of `protected_paths`;
[Authority](board.md#authority)) is refused with `protected-path-owner-merge`.
A rename out of such a path counts, and so does a list of more than 1000 files (fail closed).
The refusal is not a failed `verify`: nothing is posted, because a failed `verify` would send
the dev back to a PR that no dev can finish. `verify --fail` stays allowed: the coordinator may
still judge the work wrong and send the ticket back as for any failed verify. Only a verify PASS,
`close --verified` and `merge` are refused. The owner reviews and merges the PR by hand; then
`crewboard close <n>` ([Close after an owner merge](#close-after-an-owner-merge)).
The `done` of such a PR carries the line `protected-paths: <paths>` and is accepted as usual.

With `ci_mode: local` and a local-run `done`, `verify` needs `--local-run "<what you checked>"`
and does not read the CI; the report carries both local-run lines. `ci_mode: ci` does not accept a
local-run `done` ([Local run](ci.md#local-run-ci_mode-local)).

A failed `verify` moves the ticket in one of two ways. The agent of the ticket often works
on another ticket by then. The one-In-Progress rule of `take` ([Status flow and writers](#status-flow-and-writers))
stays true:

- The agent has no other ticket In Progress: `Review -> In Progress`.
- The agent has another ticket In Progress: `Review -> Ready` for the same agent, first in its
  queue. The other ticket is not touched. The report says why in a `case:` line. The agent takes
  the ticket with a normal `take` after `done`, `block` or `ask` on the other ticket.

Read cost: only a failed `verify` reads the board rows of the agent's tickets (one `nodes(ids:)`
read of the known tickets). A passed `verify` reads nothing more. The headless `tick` runs the
same `verify`, so it follows the same rule; both writes (Status, Order) are coordinator writes,
and `verify` is allowed for a no-model coordinator ([No-model coordinator](agents.md#headless-coordinator)).

### Failed Gate check

The evidence can be formally complete and CI green while the coordinator's own Gate
check (step 3) fails, for example when Gate parts are missing. A plain `verify` passes then,
so the coordinator records the failed Gate check:

`crewboard verify <n> --fail --body "<what the Gate check found>"`

- `verify` runs the checks of the evidence as always. The note is one more failure:
  `failed: Gate check by the coordinator: <note>`. The report has no extra `note:` line. The
  signed verify report says `verify: fail`.
- The writes are those of any failed `verify`: In Progress, or Ready first in the queue
  when the agent works on another ticket. The ticket leaves Review, so the stale-work check
  `review-not-verified` stops for it. The dev fixes the work and runs `done` again.
- The same refusals as `verify` apply: only the coordinator (`verify-coordinator-only`), only
  a ticket in Review without an open question to the owner
  ([Verify after the owner's go](#verify-after-the-owners-go)).
- `--fail` without `--body` (or with an empty note) is refused with `verify-fail-needs-note`
  before any read.
- A no-model coordinator is refused with `headless-scope`: the Gate check is a judgment
  ([No-model coordinator](agents.md#headless-coordinator)).
- A plain `verify` (without `--fail`) does not change.

### Verify after the owner's go

A dev agent finished a ticket (`done`) and then asked the owner for a go, for
example a release, deploy or legal approval (`ask`). The ticket stays in Review and carries the
label `needs-owner`. `verify`, `merge` and `close --verified` need a ticket in Review without an
open question to the owner. With one they refuse with `verify-owner-question-open`:
"<ticket> has a question to the owner since <time>: verify after the owner answers on the ticket,
or take the question back with crewboard reassign <n> --to <agent> --reason blocked-too-long; an answer in
a chat is not signed". The dev does not `take` the ticket and does not run `done` again on the
same head.

The owner answers with a comment on the ticket. Owners often answer in a chat with the
coordinator. A chat answer is not signed and crewboard cannot prove that it is the owner's, so it
does not count. After the owner's comment, `verify` judges the evidence as for any ticket in
Review (head not moved, CI, named runs) and writes the same messages:

- Pass: no status write. `close --verified` or `merge` moves the ticket to Done and removes the
  label `needs-owner`.
- Fail: In Progress, or Ready first in the queue when the agent works on another ticket.
  The label goes.
- To have the dev deliver again instead, the coordinator takes the question back
  ([Take a question back](#status-flow-and-writers)).

`verify --fail` records a failed Gate check and is no exit to Done, so an open question does not
stop it. A refusal writes nothing. A no-model coordinator never acts on a ticket with an open
question: its `verify` and `merge` refuse with `headless-scope` before any other check, and
`tick` leaves such a ticket alone.

### Close after an owner merge

The owner may merge a crew PR by hand, for example while CI is down or when an agent
may not merge. GitHub closes the ticket issue through the closing keyword of the PR, but the
board item stays where it was. `verify` and `close --verified` would judge evidence and CI that
the owner's merge made moot, and a ticket without a signed `done` cannot be verified at all
(`verify-no-evidence`).

`crewboard close <n>` (coordinator, without `--verified` or `--why`) moves the ticket to Done
when ALL of these hold:

1. The ticket issue is closed.
2. A PR in a repo of the initiative is merged, and its body closes this issue (the closing-keyword
   check that `done` uses, [Evidence](#evidence)).
3. The merge actor (`merged_by` of the PR) is the `owner_login` of the board config.

The owner route is the exception to conditions 1 and 2. When the latest signed `done` of the
assignee has the line `owner-route:`, ONLY the proof of the owner route counts, also when the PR
text has a closing keyword (a keyword added after the `done` proves nothing). The PR of that
`done` counts as the owner merge without a closing keyword and while the issue is open, when ALL
of these hold:

- the proof belongs to this ticket: the patch named in the `done` is `docs/patches/<n>-workflow.patch`
  for this issue `<n>`, and the file list of the PR contains that file. A `done` that names an
  old PR of another ticket proves nothing;
- the PR is in a repo of the initiative and is merged by the owner;
- the workflow change is applied, as a content check that fails closed: the PR changed a file
  under `.github/workflows/`, OR the patch is read at the head of the PR and, for each workflow
  file it names, the file on the base branch has every hunk of the patch (at most 5 files; a
  patch with more is not applied). A hunk is there when its new side (context and added lines,
  in order) is one block of the file and, if the hunk removes lines, its old side (context and
  removed lines) is no block of the file. So a real patch on the unchanged file is not applied. The check ignores commit messages and
  times. A patch that cannot be read, is over 1 MB or is malformed, that names no workflow file,
  that has no hunk for a file, or whose path is not a plain path under `.github/workflows/`
  (only `A-Za-z0-9._/-`, no `..`) is not applied. The check never widens to the whole directory.

`close` then also closes the issue. If a condition fails, `close` falls back to `--why` and names
the reason; the headless `tick` leaves the ticket alone. A patch-only PR that was merged before
the patch was applied is refused, whatever its text says. A `close` of an owner-route ticket reads
the PR once and its files, then the patch and one file per named workflow file (at most 5): at
most 8 requests plus extra pages of the file list.

The PR is the one in the evidence of the latest signed `done` of the assignee. If that PR does not
meet the conditions, or the ticket has no `done`, the cross-references on the issue timeline name
more candidate PRs (one more read; at most 8 PRs are read). A ticket on the owner route has no
such search: only the PR of its `done` counts.

- **Status.** Any status but Done: the owner's merge ends the ticket. The PR checks below are
  the proof, so a ticket that went back to Ready after its work was delivered (for example a
  `reassign` after a question) ends the same way. A ticket that is Done is `status-no-change`,
  so a repeat is safe. `tick` closes only Review tickets by itself.
- **Writes**, in this order: ONE signed `info` to the assignee, else to the coordinator, with the
  lines `pr: <url>`, `merge-commit: <sha>`, `merged by the owner` and `closed: owner merged <url>`
  (a `--why` is added as the line `reason: <text>`); Status Done; the label `needs-owner`
  removed. The agent State is not changed.
- **Refusals.** `close-needs-reason` names the failed condition (issue open, PR not merged, body
  does not close the issue, wrong merge actor, no PR found) and says how to go on: give
  `--why`, or use `--verified` after a passed verify. A caller that is not the coordinator
  gets `status-wrong-writer`. A refusal writes nothing.
- `--why` and `--verified` do not combine. With `--why` and no owner merge the close is the
  coordinator's judgment ([Close without a PR](#close-without-a-pr)).
- **`tick`.** The no-model coordinator does this close too, because it is no judgment
  ([No-model coordinator](agents.md#headless-coordinator)). It looks at the tickets in Review
  whose issue is closed. A Review ticket that does not meet the conditions is judged as usual. The
  issue of an owner-merged ticket is closed, so the tick also lists the 30 newest closed tickets
  of each repo when it cannot find the issue of a board item (tickets that the cache knows cost
  no read).
  The tick also closes a ticket in In Progress, Blocked or Ready whose issue is closed.
  The issue state comes with the board items that the tick reads anyway, so a ticket with an
  open issue costs no read. A closed one costs the ticket read and the PR lookup. A refusal
  gives no line and no note.

### A PR merged without a passed verify

Anyone with write access can merge a crew PR outside crewboard (the owner in the
browser, or a person with access). Crewboard does not refuse that and does not undo it: the
merge stands, and `close` ([above](#close-after-an-owner-merge)) still ends the ticket.
It only makes the gap visible.

The gap: the PR of the ticket (the PR of the latest signed `done` of the assignee, or the PR
that `close` finds) is MERGED, and no `verify: pass` of a coordinator names the head of
that PR in its `head_now:` line.

- **The note.** The coordinator posts ONE signed `info` on the ticket, to the coordinator. The
  first line starts with `merged without verify`. The lines: `pr: <url>`, `merge-commit: <sha>`,
  `merged-by: <login>`, `head: <sha>` and `last-gate: ...`. `last-gate` is the latest verify
  result or local-run line on the ticket for any head, with that head (`verify fail, head
  <sha>`, `local-gate <text>, head <sha>`), or `none`. A note exists once per (ticket, merge
  commit): an earlier note with the same `merge-commit:` line stops a repeat.
- **Who posts.** `tick` for Review tickets (also when the merge actor is not the
  owner, so the owner-merge close does not apply), and `close` after an owner merge (the PR is read already, so
  it costs no read). `watch` and `poll` do not: a PR read for each Review ticket in each poll
  is not worth it.
- **`status`.** A ticket with a note, and a Review ticket whose merged PR shows
  the gap, gets the marker `merged without verify` ([`status`](cli.md#status-the-pr-of-a-ticket)).
- **No judgment.** The note changes no Status and refuses nothing.

### Close without a PR

A test-only ticket may find that the defect is in the app code, so a new ticket
supersedes it. Or the coordinator decides that a ticket will not be done. `close --verified` needs
a verify, which needs a PR, and a bare `close` needs a merged PR. For the rest:

`crewboard close <n> --why "<text>"`: the ticket ends by the coordinator's judgment. When a
ticket replaces it, the text names that ticket; the link is text only.

Coordinator only (`status-wrong-writer`). A no-model coordinator refuses it with `headless-scope`:
it is a judgment ([No-model coordinator](agents.md#headless-coordinator)). `--why` does not
combine with `--verified`. When crewboard finds an owner merge, the close is the owner-merge close
and the reason is added as a line. A close without a reason and without an owner merge is refused
with `close-needs-reason`. The checks run first and a refusal writes nothing:

- The ticket is not Done (`status-no-change`; a repeat is safe). Any other status is allowed,
  including Backlog, Ready and Blocked.
- The reason is not empty. Line breaks in the reason become spaces.
- The text of the note passes the body check of every post (size, secrets: `body-too-long`,
  `secret-in-body`).

**Writes**, in this order: ONE signed `info` on the ticket, to the assignee, else to the
coordinator, with the line `closed: <reason>`; Status Done; the label `needs-owner` removed. The
status table allows `-> Done` from any status but Done only with this fact. The agent State is
not changed.

### A PR merged while the dev works

A push to the branch of a merged PR goes nowhere: the change is not on the default
branch. The PR still shows its old head, so a `done` with that head would look fine.

- **`done` refuses a merged PR.** `done <n> --pr <url> --sha <sha>` on a PR that is MERGED is
  refused with `pr-already-merged` before any write: "PR <url> was merged at <merge sha> by
  <login>; push nothing more to its branch. For a remaining change open a new PR from the default
  branch and run done with it". The dev opens a new PR for the rest and runs `done` with it. A
  ticket whose PR is merged and that the coordinator should end uses `close`
  ([Close after an owner merge](#close-after-an-owner-merge)).
- **`tick` tells the dev.** For a ticket In Progress with an assignee, the PR of its latest
  `done` is the PR in the `Evidence` field of its board item. When that PR is merged (by anyone),
  `tick` posts ONE signed `info` to the assignee: `PR <url> was merged by <login> at <merge sha>;
  do not push to its branch; a remaining change needs a new PR from the default branch.` It posts
  once per ticket and merge commit: an earlier `info` of a coordinator with the same text stops a
  repeat, and the state cache keeps the told PR, so it is not read again. It changes no Status.
- **Reads.** A ticket without a PR in `Evidence` costs no read. Else one PR read per tick, until
  the dev is told. A merged PR that is not told yet adds the read of the ticket (issue,
  comments, item) and the one post. `watch` and `poll` do not do this.

### Work that an earlier agent left

A dead, draining or replaced agent may have pushed a crew
branch and opened a PR with `Closes #<n>`. Every reassign writes it down: the command (with or
without `--to`), `reassign --derived`, the coordinator duty for a dead or departed agent and
`stop --force`. Before it posts the notice to the old agent, crewboard reads GitHub once: the open
PRs of the repo for a head branch `<dev_branch_prefix><old agent>/<n>-<slug>`, and, when there is
none, the refs of that prefix. A read that fails prints a warning to stderr and never blocks the
reassign. The notice (still `Reassigned. Reason: ...`, wire format `v=1`) then has one more line:
`open-pr: <url> branch: <branch>`, or `earlier-branch: <branch>` for a branch without an open PR.
The reports that list the returned tickets (`agent-dead`, `agent-stopped`, the notice of
`stop --force`) show `<ticket> (earlier PR <url>)`.

- **Who is told.** `reassign --to` and `assign` print `an earlier PR exists: <url>; continue on it
  with `crewboard take <n> --continue-pr <url>` or close it` (the PR is read again, so a closed PR is
  not named; a branch of the new agent itself is not named). `take` prints the same fact when it
  was not called with the flag. The line comes from the LATEST `Reassigned.` notice of a
  coordinator in the signed log, so a line in another message does not count.
- **`take <n> --continue-pr <url>`** (dev, first take of a Ready ticket; not with `--interrupt`)
  checks before it writes: the PR is in the repo of the ticket and is the one that the latest
  reassign notice names (`adopt-pr-not-recorded`; `adopt-pr-other-repo`); it is open
  (`adopt-pr-not-open`); its head branch is a crew branch of THIS ticket,
  `<dev_branch_prefix><agent>/<n>-<slug>` (`adopt-pr-bad-branch`); it names the ticket
  (`adopt-pr-not-linked`). The ack carries `adopts-pr: <url> branch: <branch>` as a record, and the
  output lists branch, PR and `done`. A PR that the notice does not name cannot be adopted: start a
  branch of your own.
- **`done` and `merge`** accept the head branch of that PR for the new assignee instead of
  `evidence-branch-name`, only when the latest reassign notice of a coordinator names this PR and
  this branch and the branch is under `dev_branch_prefix` (the same rule as for `adopts-pr:` of a
  creation info). The PR body must still close the ticket (`evidence-pr-not-linked`).
- **`ticket new --continue-pr`** (the follow-up case below) also takes a PR whose parent is back in
  Backlog or Ready and whose latest reassign notice names that PR, not only a Done parent.
- An older notice has no line: `take --continue-pr` is refused with `adopt-pr-not-recorded`.

### A follow-up ticket continues an open PR

When the owner sets a ticket Done while its PR is still open, the coordinator can make a
follow-up ticket for that PR. Without the rule below, `done <follow-up> --pr <url>` refuses with
`evidence-branch-name`: the head branch is `crew/<agent>/<old ticket>-...`, GitHub cannot change
the head branch of a PR, and a renamed branch closes the PR.

`crewboard ticket new ... --continue-pr <url>` (coordinator) records the PR for the new ticket. These
checks run before anything is created, in this order:

1. The URL is a PR URL (`evidence-pr-url`) of a repo of the initiative (`repo-not-in-initiative`),
   and the PR exists (`evidence-pr-missing`).
2. The PR is open and not merged (`adopt-pr-not-open`).
3. The head branch is `<dev_branch_prefix><agent>/<n>-<slug>` (`adopt-pr-bad-branch`).
4. Ticket `<n>`, in the repo of the PR, is on the board and Done, or it is in Backlog or Ready and
   its latest reassign notice names this PR ([Work that an earlier agent left](#work-that-an-earlier-agent-left))
   (`adopt-pr-parent-not-done`).

Then crewboard creates the ticket as `ticket new` does and writes the line
`adopts-pr: <url> branch: <branch>` into its signed creation info (the `info` comment that holds
the item id). The new ticket has its own number, Gate and Agent as usual (`--assign` works).

- `done <new> --pr <url>` accepts the head branch of that PR for the new ticket instead of
  `evidence-branch-name`, only when a signed `info` of a coordinator on the new ticket has the
  line for this PR and this branch, and the branch is still under `dev_branch_prefix`. A line
  that a dev agent wrote, a comment of the owner's account without a header, and a line in the
  ticket body do not count. `merge` uses the same line.
- The PR body must still close the NEW ticket. After the creation, `ticket new` prints a note if
  it does not. `done` refuses with `evidence-pr-not-linked`, and its text says that the dev agent
  edits the PR body so that it closes the new ticket.
- Every other check of `done` is unchanged (head SHA, CI of that head, workflow files).

## Branches

Branch per ticket: `crew/<agent-name>/<issue-number>-<slug>`. The slug has only the letters
`A-Za-z0-9._-`, at most 100, no leading `-` or `.`, no `..`; `done` refuses any other branch with
`evidence-branch-name`. A branch name goes into command lines and URL paths, so crewboard
quotes it for the shell and encodes it in every URL path. The PR body contains
"Closes #<n>" (or the cross-repo form). No agent pushes to a protected branch. One exception:
a ticket that continued an open PR of a Done ticket keeps the branch of that PR
([A follow-up ticket continues an open PR](#a-follow-up-ticket-continues-an-open-pr)).

## Queue, labels, ticket creation and merge

The headless dev agents are in [Headless dev agents keep working](agents.md#headless-dev-agents-keep-working).

### Queue order and take


1. `assign` accepts a dev agent that is busy (a ticket In Progress). The ticket goes to Ready with the Agent
   set. The agent takes it after its current ticket. An agent that is not a dev, is paused,
   draining or left, or is not alive, is still refused (`status-agent-not-active-dev`,
   `status-agent-not-alive`).
2. The queue position of a ticket is its `Order` field ([Queue order per agent](#queue-order-per-agent)).
   Only the coordinator (or the owner) writes it.
3. `watch` names the assignment in an event (`assigned to you; status Ready`). The signed
   assignment message has a line `order: <n>`. `status` shows an Order column (JSON key
   `order`). "Take #N next" is therefore board data.

### Agent labels


1. The board views show only the fields that someone made visible. The `Agent` field is not
   visible in the views that GitHub creates, and the API has no call to change the visible
   fields of a view. So `assign`, `reassign` and `reassign --derived` also set the label
   `agent: <name>` on the issue and remove the label of the previous agent. The label is
   created on first use, in the repo of the issue, with one stable color per agent (a hash of
   the name picks one of 12 colors). `close` keeps the label as history.
2. A ticket that has an open question to the owner ([Status flow and
   writers](#status-flow-and-writers)) carries the label `needs-owner`. `ask` and `done` on the
   owner route set it. `take`, `verify`, `merge`, `close` and `reassign` remove it when no
   question is open any more, and `close` removes it in any case. The coordinator's real
   `poll` or `watch` cycle also removes a stale label (the issue is closed, or no question is
   open): one list of the labelled issues per repo, at most every 5 minutes.
3. The labels help people only. The board field `Agent` and the signed messages stay the
   source of truth, and crewboard never reads a label back as a fact. A failed label write
   is a warning, not a refusal.
4. **Owner setup:** none. `init` creates the ticket views with the columns Title, Status,
   Agent and Order, sorted by `Order`. A view that already exists keeps its columns and its
   sort: delete the view and run `init` again.

### `ticket new` and `ticket adopt`


The coordinator creates a ticket and puts it on the
board by a command, not by raw GraphQL.

1. `crewboard ticket new --title T --body-file F [--repo R] [--backlog | --assign <agent>]
   [--continue-pr <url>]` (coordinator; `--continue-pr`: [A follow-up ticket
   continues an open PR](#a-follow-up-ticket-continues-an-open-pr)). It reads the text, checks the `Gate:` line ([Ticket content](#ticket-content)), the
   secret scan and the size limit, and checks the writer and AUTHORITY.md
   (`coordinator_may_create_tickets`). With `--assign` it also runs the checks of `assign`
   (agent is a live dev in State `active`, capabilities) before anything is
   created, so a refusal leaves no half-made ticket. Then it creates the issue with the label
   `crew:work` as the crew account, adds the board item, writes the signed info comment with the
   item id ([Item ids are the source of truth](board.md#item-ids-are-the-source-of-truth)), sets `Kind` Work, `Status` Backlog and `Gate`. Without `--assign` the ticket
   stays in Backlog. With `--assign`, `assign` moves it to Ready.
2. `crewboard ticket adopt <issue> [--assign <agent>]` does the same steps for an issue that
   exists (label `crew:work`, open, `Gate:` line). It is idempotent: a ticket that is on the
   board keeps its Status. This is also the repair for a ticket that was added by hand and is
   missing from `status`.
3. A missing `Gate:` line, a closed issue and a caller that is not the coordinator are refused
   (`status-gate-missing`, `ticket-closed`, `status-wrong-writer`).

### `merge`


A warning for contradicting AUTHORITY.md keys: [Contradicting keys in AUTHORITY.md](board.md#contradicting-keys-in-authoritymd).

1. `crewboard merge <issue> [--method squash|merge|rebase]` (coordinator). It runs only when
   AUTHORITY.md lets the coordinator merge: `coordinator_may_merge: true`. It is
   refused with `authority-merge-not-allowed` if the flag is false. A `merge` entry in
   `needs_owner` is deprecated advice that still refuses (`authority-needs-owner`).
   The checks run in this order: the authority (`check_merge`), then the protected paths (item 1a),
   then the rest.
1a. The protected paths: `merge` reads the current file list of the PR (after the PR, before any
   CI read). If the PR changes `AUTHORITY.md`, `.crewboard.toml`, `.github/workflows/**`,
   `.github/actions/**`, `.github/CODEOWNERS`, `CODEOWNERS`, `docs/CODEOWNERS` or a glob of `protected_paths` (a rename out of
   one counts), or has more than
   1000 files, the merge is refused with `protected-path-owner-merge`, whatever the keys of
   AUTHORITY.md say. The refusal writes nothing, and its text ends with the command that follows
   the owner's merge: `crewboard close <n>`. `tick` leaves such a ticket.
2. The evidence checks are those of `done` and `verify`. The ticket is in Review without an open question
   ([Verify after the owner's go](#verify-after-the-owners-go)) and
   the latest `done` has a passed `verify`. The PR is the one that the signed `done` names, in a repo of the
   initiative, still open, and its body closes the ticket. Its head SHA equals the SHA of the
   evidence (`evidence-head-moved` if the head moved). The CI of that SHA is green, or the ticket
   declares `CI: none`. A local-run `done` passes the CI part only while `ci_mode` is still
   `local` and its passed `verify` has the coordinator's `coordinator-local-gate:` line on the same head
   ([Local run](ci.md#local-run-ci_mode-local)).
3. The merge call carries the verified SHA (`PUT /repos/{repo}/pulls/{n}/merge` with `sha`), so
   GitHub refuses the merge too if the head moves between the check and the call
   (`merge-head-moved`). A repo setting that forbids the method is `merge-not-allowed`.
4. After the merge, crewboard deletes the PR branch if it is in the same repo and under
   `dev_branch_prefix` and not protected, moves the ticket to Done, and posts a signed `info`
   message to the dev agent (PR, SHA, method, merge commit). If a step after the merge fails,
   the error is `merge-done-ticket-open` and names the repair: `crewboard close <n> --verified` (the PR was
   merged with the coordinator's token, so a bare `close`, which needs the owner as merge actor,
   would find no owner merge; the ticket is in Review with a passed verify, so `--verified` holds).
5. The merge is the first write, because it cannot be undone. Every refusal leaves the PR and
   the board as they were. The one exception is the request in item 6.
6. Mergeability. Before the merge call, `merge` reads the PR once more and takes
   `mergeable_state` (REST; the same values as GraphQL `mergeStateStatus`). Mapping: `clean`,
   `has_hooks` and `unstable` go on (the CI of the verified SHA is green by item 2, so
   `unstable` is a non-required check); `blocked` goes on too, because GitHub's refusal
   in item 7 is the guard for it; `draft` is refused with `pr-not-mergeable` and "the PR is a
   draft; mark it ready for review" (no request; GitHub's draft text does not match item 7);
   `dirty` and `behind` are refused with `pr-not-mergeable`
   (exit 2), naming the state and the next command of the dev agent:
   `crewboard done <n> --pr <url> --sha <new head>` after the branch update (the
   re-delivery from Review, [Status flow and writers](#status-flow-and-writers)). Then the
   coordinator runs `verify` and `merge` again; `unknown` (GitHub still computes it) is read again up to 4 times,
   2 s apart, then refused with `pr-not-mergeable` and "state unknown, try again". On `dirty`
   `merge` first posts ONE signed `request` to the assignee: rebase onto the base branch, with
   the files of the PR as the list (the conflicting files need a local merge-tree; the message
   says which list it gives). The request names the same `crewboard done` command. On `behind` and `unknown` nothing is posted. No message says
   "merged" in any of these cases, and the merge call is never made.
7. GitHub's own refusal of the merge call stays the final guard, because the state can change
   between the read and the call: HTTP 405 or 409 whose message says "not mergeable", "merge
   conflict" or "base branch was modified" is `pr-not-mergeable` too (no request is posted in
   this case). A moved head is still `merge-head-moved`.

### `ticket split`


One ticket is one PR, and `merge` needs a verified ticket. A ticket with several independent
causes would block its finished part. `ticket split` moves the open part to a new ticket.

1. `crewboard ticket split <issue> --title T --body-file F` (coordinator). The old ticket must
   be In Progress or Blocked (the typical case is Blocked: the dev blocked with
   "the finished part can merge, the cause is still open") and have an Agent.
2. Rules, in this order. A caller that is not the coordinator is refused (`status-wrong-writer`).
   `coordinator_may_split_tickets: false` in AUTHORITY.md refuses it
   (`authority-split-not-allowed`). A ticket in another status (Backlog, Ready,
   Review, Done) is refused (`split-not-in-progress`), and so is a ticket without an Agent (`split-no-agent`). The new
   ticket must pass the checks of `ticket new` (title, `Gate:` line, size limit) and the
   checks of `assign` for the same agent. A refusal writes nothing.
3. The new ticket gets the text of the file plus a line `Split from: <repo>#<n>`. If the file
   has no `Gate:` line, the `Gate:` of the old body is copied.
4. The old body gets ONE appended line `Split off: <repo>#<new> - <title>`. Nothing else in it
   changes.
5. The new ticket goes on the board and into the queue of the same agent, like `assign` to a
   busy agent (Ready, Agent, signed assignment message).
6. One signed info note on the old ticket tells the agent that the scope shrank and where the
   open part went. It also says: run `done <old> --pr <PR>` for the finished part, no `take`
   is needed. The old ticket keeps its status (In Progress or Blocked). The
   agent runs `done` on it with the finished PR.
   - In Progress: `done` as always.
   - Blocked: `done` moves the ticket to Review without a `take`, with
     every check of `done` (PR, CI judgement, `unread-events`; see [Status flow and
     writers](#status-flow-and-writers)). It does not touch the In Progress ticket of the agent
     and does not count as a second In Progress ticket. The proof of the split is the
     `Split off: <key> - <title>` line of the old body AND the signed split note of a
     coordinator for the same `<key>` on the ticket. The line alone is not enough, because
     anyone who can edit the issue can write it. A Blocked ticket without the proof keeps the
     old rule: `done` is refused, take it first.
7. Idempotency and repair: every step is idempotent, so a split that failed halfway is
   repaired by running the same command again. Before it creates a ticket, the command looks
   for the ticket of an earlier run: the `Split off:` line with the same title in the old
   body, else an open `crew:work` issue with the same title and the line `Split from: <old>`.
   It reuses that ticket and never creates a second one. Then it runs the missing steps in
   order: the link line in the old body; the board item and the queue entry (Ready, same
   Agent); the signed note. The output names the steps done now and those done before, for
   example `split: <old> -> <new>; now: queued, note posted; already done: exists, linked`.
   A rerun of a complete split writes nothing and exits 0. Another title is another split.
8. The no-model coordinator ([No-model coordinator](agents.md#headless-coordinator)) is refused with `headless-scope`.

## Workflow files: the owner route


The crew token has no `workflow` scope. This is a security rule: a token that can write
workflows can add a workflow that reads the secrets of the repository. GitHub therefore rejects
a push by a dev agent that changes `.github/workflows/**`. This section defines the route for
such tickets, so that no agent has to `block` for it.

1. **The mark.** A ticket is on the owner route if its body has the line `Route: owner-workflow`,
   Only this line marks the route. A `Scope:` line that names `.github/workflows` does not,
   because text cannot tell whether the scope includes or excludes the workflow files. The
   coordinator writes the line for a ticket that changes a workflow file. It is a body line, not a
   `Needs:` line: `Needs:` is a capability of the agent ([profiles](profiles.md)), and `assign` refuses an agent
   that lacks one. Every dev agent can write a patch file, so the route is no capability.
   `assign` adds the lines `route: owner-workflow` and `patch: docs/patches/<issue>-workflow.patch`
   to the signed assignment message and prints the route.
2. **The delivery.** The dev agent pushes everything except the workflow files on its crew
   branch. It adds the workflow change to the PR as the file `docs/patches/<issue>-workflow.patch`.
   The title and the body of this PR say `Refs #<issue>`, never `Closes #<issue>` (item 7).
3. **The safety net.** `done` reads the files of the PR (`pulls/<n>/files`). It decides:
   workflow files in the PR: normal route (they are on the branch). The patch file in the PR:
   owner route, marked or not. Marked, but no patch and no workflow file: refused with rule
   `workflow-needs-owner`; the message names the patch path and says not to block. Otherwise:
   normal route.
4. **The record.** On the owner route `done` judges the evidence as usual (PR, SHA, CI), then
   writes the `done` message (with the line `owner-route: <patch>`), the `Evidence` field and
   Status Review, sets the label `needs-owner`, and posts ONE `question` to the owner. The question holds the exact command that the owner runs: `git fetch`, `git
   switch <branch>`, `git apply <patch>`, `git add .github/workflows`, `git commit`, `git push`.
   Its first line is `owner-route: <patch> sha=<head>`. Its next line says: apply the patch to
   the branch and push, do NOT merge the PR as it is (item 7). A second `done` for the same patch and
   head finds this line in the signed messages and sends nothing (dedup). The coordinator does
   not send a second message.
5. **The end.** The owner applies the patch, pushes, answers on the ticket, and merges the PR
   by hand: a PR that changes a workflow file is merged by the owner only
   (`protected-path-owner-merge`). Then the coordinator runs `crewboard close <n>`
   ([Close after an owner merge](#close-after-an-owner-merge)). The PR has no closing keyword by
   design, so `close` accepts it although the issue is open, and it closes the issue. `verify` of a ticket whose latest
   `done` is on the owner route is refused with `verify-owner-route`: "the latest done of <ticket>
   is on the owner route: the owner applies the patch and merges the PR by hand, then crewboard
   close <n>".
6. A `block` for "workflow scope" is not needed and not the route. The crew token never gets
   the `workflow` scope.
7. **No closing keyword.** The owner may merge the patch-only PR before the
   patch is applied. A closing keyword would then close the ticket, and the board would show
   Done while `.github/workflows` is unchanged. So on the owner route `done` reads the title
   and the body of the PR and refuses with the rule `owner-route-closing-keyword` when they
   close THIS ticket. It writes nothing. A closing keyword is one of `close`, `closes`,
   `closed`, `fix`, `fixes`, `fixed`, `resolve`, `resolves`, `resolved` (any letter case, an
   optional colon), followed by `#<n>`, `<owner>/<repo>#<n>` or the URL of the issue. A keyword
   for another issue does not count. The PR must still link the ticket (`Refs #<n>`); if not,
   `done` refuses with `evidence-pr-not-linked`. After the owner applied the patch, the
   workflow files are in the PR, the normal route applies, and `Closes #<n>` is right again.
8. **The safety net after the fact.** A ticket is a finding when: its Status is
   Done; the latest `done` of its assignee has the line `owner-route: <patch>` with the patch of
   THIS ticket (`docs/patches/<n>-workflow.patch`); and the PR of that `done` contains that file,
   is merged, and changed no file under `.github/workflows`, and the content check of
   [Close after an owner merge](#close-after-an-owner-merge) fails: the base branch lacks a hunk
   of the patch (the owner may apply the patch on the base branch with a commit of their
   own and merge the patch-only PR). A patch that cannot be read counts as not applied. A `done`
   that names another patch, or a PR without the patch file, is not judged: it is no finding and
   is read again next time (nothing is kept). The `poll` and `watch`
   of the interactive coordinator find it and, in this order: post ONE signed `question` to the
   owner (first line `owner-route-merged: <PR URL>`, so a repeat finds it again: once per PR),
   reopen the issue, and set the label `needs-owner`. The Status stays Done (Done is final):
   the owner moves the ticket. A failed run repeats the missing steps. The question says: apply
   the patch (it is on the base branch now) and push it on a new branch, do NOT treat the
   merged PR as the delivery. A second poll finds nothing: the ticket counts as judged until its
   board item changes. The ticket stays Done with the label until the owner moves it. A lost
   state cache judges it again, which is safe: the question is found again from the signed
   messages, and the reopen and the label are idempotent. A write that is in place already
   (the issue is open, the label is set) is skipped, so the net never changes the board item
   by itself twice. An answer ends the finding: when ANY owner message or a coordinator answer
   with `withdraws: question` follows the `owner-route-merged` question, the net does nothing
   more for that PR, even when the item changes later (the owner decides what comes next, so
   the net asks once and does not check the owner's word).
   The no-model coordinator writes nothing ([No-model coordinator](agents.md#headless-coordinator)):
   its `tick` logs one line `left for the interactive coordinator: <repo>#<n>, ...` per
   finding, once per change of the board item (the tick keeps the finding in its state cache with
   the prefix `tick:`; the interactive coordinator still judges it). The reads are bounded.
   Only Done tickets are looked at. A Done ticket costs one read (the comments of the issue)
   once; a ticket whose last `done` names the owner-route patch of this ticket costs two more
   (the PR and its files, 1 per 100). Only a merged PR without a workflow file costs the patch
   (1) and one file of the base branch per workflow file of the patch (at most 5) as well: at
   most 9 requests plus extra pages. The result is kept in the state cache
   (`owner_route_judged`, per board item and its `updated` time), so a judged ticket costs no
   read until the item changes. At most 10 Done tickets are judged per run, so a long board
   is judged over several polls. A ticket whose owner-route PR is not merged yet, or whose
   `done` is not tied to this ticket, is not kept and is read again (1 to 3 requests). If a
   judgement fails, the findings before it stay and the failure is named with its ticket. The
   tick sees the Done tickets that its own state cache knows.

## Queue order per agent


Each agent has an explicit queue order on the board, so that an order is board data and an
imbalance between agents is visible.

1. **The issue number is not an order.** The issue number is the order of creation, not of the
   plan, and a coordinator who wants #40 before #32 cannot say it. So the board has an explicit
   per-agent order. `Order` is the one concept: the position in the queue of an agent, lowest
   first.
2. **Field `Order`.** A number field on a work item (whole number 0 to 1000000 from `assign`,
   lowest first). The requeue after a failed verify ([Verification by the coordinator](#verification-by-the-coordinator)) can write -1 or lower.
   `init` creates it on a board that has none. A board that `init` did not make gets it on first
   use by `assign`. Until it exists, `status --queue` prints a note and the order is by issue
   number. Only the coordinator and the owner write
   it (the writer table of [Fields](board.md#fields)). A number field has no options, so the complete-list rule for single-select
   options ([Fields](board.md#fields), [Item ids are the source of truth](board.md#item-ids-are-the-source-of-truth)) does not apply; no option of another field is touched.
3. **Next ticket.** The next ticket of an agent is its lowest `Order` among Ready. Ties: lower
   issue number. A Ready ticket without `Order` comes after all that have one, ordered by issue
   number. `mine` lists In Progress first, then Ready in this order; the first Ready line is the
   next ticket. The dev brief says: take the first Ready ticket of `mine`. A ticket that a
   failed `verify` re-queues gets the front `Order` ([Verification by the
   coordinator](#verification-by-the-coordinator)). `take --interrupt` uses `Order` too
   ([Park](#status-flow-and-writers)). The queue of an agent is its open work: its tickets in
   Ready, In Progress or Blocked (a ticket in Review waits for the coordinator and is not in
   the queue).
4. **`assign <issue> --to <agent> [--order N]`.** As `assign` ([Queue order and take](#queue-order-and-take)) and also sets `Order`; the
   signed assignment message has an `order:` line. Without `--order`, the ticket gets the end
   of the queue: the highest `Order` of the open tickets of that agent plus one (1 for an empty
   queue), and the output says `Order set to <n> (the end of the queue)`.
5. **`assign <issue> --order N` on a queued ticket (no `--to`, or `--to` its own agent).** Sets
   `Order` of a Ready ticket that has an agent. Refused with `reorder-not-queued` for any other
   ticket and with `order-invalid` for a bad number. A dev agent cannot do it
   (`field-wrong-writer`). It posts no message. `--to` its own agent without `--order` is
   `ticket-already-assigned`. `assign` without `--to` and without `--order` is refused with
   `assign-needs-to`.
6. **`assign <issue> --to <other agent> [--order N]` on a queued ticket.** Moves a Ready ticket
   to the queue of another agent: field `Agent`, the agent label ([Agent labels](#agent-labels)),
   `Order` and ONE signed assignment message to the new agent. It is the normal assignment (with the
   signed `gate:` and `ci:` lines, `order:` and `moved-from: <old agent>`), because the signed log
   takes the assignee only from an assignment. Default `Order`: the highest `Order` in the
   Ready queue of the target plus one (1 for an empty queue). The target may be busy ([Queue
   order and take](#queue-order-and-take)). Refusals: `rebalance-not-ready` (the ticket is not
   Ready; use `reassign`), `rebalance-no-agent`, `rebalance-same-agent`,
   `status-agent-not-active-dev` (the target is not an approved dev agent in State `active`), the
   liveness rule of `assign` (late or dead) and `capability-mismatch` (`--override-capabilities`
   with `--reason` overrides it, as for a new assignment). A refusal changes nothing.
7. **`status --queue`.** For each approved dev agent that has not left: its State and liveness,
   the number of In Progress and Ready tickets, the In Progress ticket, then the Ready tickets
   in order with `Order`; the Ready tickets without an agent. With `--json` the
   full status gets a key `queue` (agents with `ready_count`, `ready_without_agent`,
   `imbalance`, `board_has_order_field`).
8. **Imbalance note.** One line when an agent A has at least 3 more Ready tickets than another
   agent B, B takes work now (State `active`, liveness `alive`) and B can take the
   work of A: the same `arch:` fact and every declared tag of A. The pair with the largest
   gap is named. The line suggests `assign <n> --to <agent>`. It is advice: the coordinator decides.
