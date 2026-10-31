# Events and polling

Part of the [crewboard specification](README.md).

## `watch` semantics

- It reads the cursor from the agent item. It returns **every** event after the cursor
  that concerns this agent: messages addressed to it, to `all` or to its role;
  assignments; status changes of its tickets; owner messages; registry changes.
- It never waits for a specific message, file name or ticket: such a wait fails silently when the
  name changes.
- It advances the cursor only after printing the events, so a crash cannot lose them.
- Output: one block per event, human-readable by default, `--json` for programs.
- It sends heartbeats while it waits.
- Events are hints. The truth is the board plus the signed log: an agent that misses an event
  reads the state again and does not depend on the event.

## Status changes by GitHub automation

A built-in project workflow can write `Status` behind crewboard
([Built-in project workflows](board.md#built-in-project-workflows)). Without a mark, the watch
reports its write as an ordinary `status-changed`, and the coordinator cannot see that no
crewboard command made it.

1. The `creator` of a field value is not the last writer: it still names
   `github-project-automation` after an agent account set the value again. Only the issue timeline
   names the real actor: `GET /repos/{o}/{r}/issues/{n}/timeline`, event
   `project_v2_item_status_changed`, field `actor.login`. The automation shows there as
   `github-project-automation[bot]`.
2. A `status-changed` event is explained when a signed crewboard message of a trusted agent is
   on the same ticket among the fresh comments of the same cycle. The crewboard commands that
   write `Status` post such a message on the ticket (for example `take`, `done`, `block`,
   `assign`). A crewboard write without one costs one timeline read and gets no mark. The
   agent's own writes give no event at all.
3. For an event that is not explained, `poll` and `watch` read the timeline of that one issue:
   one REST call per event, at most 100 timeline events. If the last
   `project_v2_item_status_changed` names the automation, the event text ends with
   `(by GitHub automation)`. Example: `status Blocked -> In Progress (by GitHub automation)`.
4. No mark when the actor is another account, when the page has no status event, or when the
   timeline has more than 100 events (the newest are on a later page). A read error leaves the
   event as it is and prints one line on stderr. The cycle does not fail.
5. Call cost: no call in a cycle without a status change, no call for an explained event, no
   call in a peek (`poll --quiet`) and in `resume`, which compute the same events again at each
   call. One REST call for each other `status-changed` event, once: the next cycle has the new
   snapshot and gives no event.

## Status against the signed log

Any agent token can write the board `Status`, so crewboard detects a Done or Dropped without
proof instead of preventing it. The coordinator's cycle (`poll`, `watch`) checks the tickets that
another writer moved to Done or Dropped since the last snapshot, the tickets that another writer
moved out of Done or Dropped (set back behind a close; both are final; the agent's own writes are
in its snapshot already), and the tickets that an earlier cycle could not settle. `status` and
`audit` check every Status.

1. One read of the comment list of that one ticket (up to 50 pages), then the judgement of
   `audit` and `status` (`ticket::log::status_contradicts`). No call in a cycle without such a
   move. A ticket that the cycle changed itself is skipped. A move out of Done or Dropped waits
   in a queue (state key `setback_queue`): a cycle reads at most 5 of them, the oldest first. A
   list with more than 50 pages cannot be judged: one `notes-not-checked` event (one owner
   report), and the ticket stays under check (a false Done too). It is read again when its board
   row changes and 6 hours after the last read, not in every cycle. A Done that the owner wrote by
   hand has no signed message, so it is reported too; the note to the owner says so.
2. A legitimate command changes the field a moment before it posts its message, so a mismatch
   seen once is kept (state key `status_pending`) and judged again at the next cycle. It is
   reported when it is still there and the item has been quiet for 60 s.
3. The report is ONE signed `info` to `owner` in the registry issue, with the line
   `event=<repo>#<n>/status@c-<id>` (the repo in lower case; `<id>` is the comment id of the
   latest signed message of the ticket), and ONE `status-mismatch` event for the coordinator.
   The key is read back from the registry issue, as for the security events, so a restart or a
   lost cache repeats nothing. A board that flips between statuses gives one report; the owner
   hears again only after a new signed message on the ticket. The cycle writes nothing to the
   board.

## Polling and rate limits

- Default interval 60 s, configurable. Backoff on secondary rate limits.
- GitHub has no server-side filter for "items changed since" (`orderBy` accepts only
  POSITION, and the `updated:` filter has day precision). Each poll reads **all known
  items by id** in one GraphQL query (cost 1 point for up to 100 items)
  and compares `updatedAt` locally.
- An item's `updatedAt` does not change when an issue gets a new comment. Comments come
  from `GET /repos/{r}/issues/comments?since=<cursor>&sort=updated` per repo, with
  conditional requests (ETag). A `304 Not Modified` costs 0 primary rate-limit points
  .
- A comment with `updated_at != created_at` was edited. Its signature then fails the
  body hash, and crewboard treats it as invalid ([Signature](messages.md#signature)). An
  edited comment of the owner is judged by its editors: REST does not name them, so the
  feed reads them with one batched GraphQL query (`IssueComment.editor` and
  `userContentEdits`), only when an edited owner comment is in the batch. Normal runs make
  no extra call. A failed read fails the poll, so the cursor does not move.
- Writes to project items can fail with "temporary conflict". crewboard retries them
  with backoff (at most 3 times) and then reports exit 3.
- Budget: 10 agents at 60 s should stay under 2,500 points per hour of the shared limit.
  `status` shows the remaining limit.

## Event checkpoints and `done`


A dev agent reads its events once, at the start of a wake, and then works for up to an hour. A
coordinator message that arrives during the wake is seen only at the next wake. Two measures
close the gap.

1. **Checkpoints in the skill.** Before each `git push`, before `done` and before the wake
   ends, the agent runs `crewboard poll --quiet`. On exit 0 it runs `crewboard watch --timeout 1m`
   and handles the new events first. A push is not a crewboard command, so only the skill can
   ask for the checkpoint there.
2. **The `done` check.** `crewboard done` looks for events of this agent that are not read yet:
   the feed read of `poll --quiet` with the same filter as `watch`, newer than the event cursor
   (the board `Cursor` field, or the local cursor of a headless config dir, see [Headless coordinator](agents.md#headless-coordinator)). If there
   are some, it refuses with rule `unread-events` (exit 2), names the count, the classes and each
   event (sender, kind, time, URL, and the first lines of the text as in
   [Unread messages on a ticket](#unread-messages-on-a-ticket), item 3), writes nothing, and
   says: handle the events, run `crewboard watch --timeout 1m`, then run `done` again. The check runs before the first write of `done`
   (before the heartbeat refresh too).
3. **`--override-unread`.** The flag passes the check. The signed `done` message then has a last
   line `override: unread-events (comments 123, 456)` with the ids of the unread comments. It
   also covers the unread messages of the ticket itself ([Unread messages on a ticket](#unread-messages-on-a-ticket), same flag), because those are
   a part of the same events.
4. **What counts.** Verified agent messages and owner comments that the `watch` filter delivers
   to this agent, on any ticket: coordinator messages, `answer` and other replies to an `ask`
   or a question, and owner comments that name the agent or that are on a ticket of the agent
   (the ticket of the `done`, and the tickets that the cache shows as the agent's).
   **What does not count:** the agent's own messages; old liveness traffic (pings and pongs of old coordinators);
   board-derived events (assignment and status changes); the events of the duties, among them
   `stale-work` and `ci-finished`; security events and forged or edited comments. The
   `ci-finished` event of the head that `done` names is the reason to run `done`, so it must not
   block it. The check never runs the duties, so no such event can block (test).
5. **`take` marks as read.** As for `take` in [Unread messages on a ticket](#unread-messages-on-a-ticket), the messages of the ticket of the `done` that stand
   before the agent's own ack (the `take`) count as read. The ack is found in the same feed read.
6. **No cursor.** An agent without a readable cursor never watched; the check passes.
7. **Read-only.** The check never moves a cursor, never saves an ETag, and never changes the
   state cache. `poll --quiet` and `watch` stay the only commands that move the cursor.
8. **Cost (measured with the fake GitHub, one repo, one ticket).** One `poll --quiet` makes 6
   requests: REST 3 (`GET /users/<owner>`, the registry comments, the comment feed) and GraphQL 3
   (agent items and ticket rows). An unchanged feed is
   a 304 and costs no primary rate-limit point. The `done` check adds ONE REST read to what
   `done` reads anyway: the comment feed of each linked repo. It makes no ticket read, no PR
   read and no CI read. A refused `done` makes 5 requests: REST 3, GraphQL 2.

## Unread messages on a ticket


An example: the coordinator posted a decision. One minute later the dev agent, still inside a
wake that began before the answer, posted a report that waited for that decision. The agent reads
new events only at its next wake, so every such crossing costs one poll interval.

1. **The check.** `say`, `done`, `block` and `ask` look for unread messages on the ticket they
   post to. A message is unread when it is addressed to the sender and is newer than the sender's
   cursor ([`watch` semantics](#watch-semantics)). "Addressed to the sender" is decided by the same filter as `watch`:
   `to` is the sender, `all` (not from the sender), or `coordinator` for the coordinator. An owner
   comment counts when it names the sender (`-> agent`), and when the ticket is assigned to the
   sender. For `done`, `block` and `ask` the assignment is the `Agent` field of the loaded ticket.
   For `say` it is the cached ticket index (no extra call); a ticket that is not in the cache
   does not count as held.
2. **What does not count.** A message to another agent, a message to the owner, the sender's own
   messages, a message on another ticket, a message older than the cursor, and unsigned,
   foreign, edited or forged comments. The last ones are security events, not messages.
3. **The refusal.** Rule `unread-message-on-ticket`, exit 2. The text names each unread message:
   sender, kind, signing time and URL. It also shows the text, so the agent needs no extra
   `watch` call to read it: for the first 5 messages the first 3 non-blank lines, each cut to
   200 characters (`[...]` marks a cut line; a line `[... n more line(s) at the URL]` marks a
   cut text). A later message gets its sender line only. It says: handle them, run
   `crewboard watch --timeout 1s` (it marks them as read), then run the command again, or pass
   `--override-unread`. The command posts nothing and changes no status.
4. **`--override-unread`.** The flag passes the check and records the override in the signed message:
   a last body line `override: unread-message-on-ticket (comments 123, 456)` with the ids of the
   unread comments. In `done` the line stands after the evidence lines; the evidence parser reads
   only `pr`, `sha`, `ci` and `ci-sources`, so the line does not change the evidence (unit test).
   The line is added only when an unread message existed. The command also prints a `note:` line.
5. **The cursor does not move.** The check only reads. A refusal and an `--override-unread` post leave the
   cursor as it was. Only `watch` and `poll` advance it, so the next `watch` still delivers the
   message.
6. **`take` marks as read.** `take` (also the resume after an answer) prints the messages for the
   agent that came before its own signed ack (sender, kind, time, first lines, URL). The check
   treats every comment up to the agent's latest own ack on that ticket as read. Messages after
   the ack follow the cursor rule. The watch cursor does not move, so `watch` still delivers
   those messages once.
7. **Only tickets.** The check applies only to an issue with a board ticket item: the ticket index
   has it, or the coordinator's signed info comment with `item=PVTI_...` is on it. The registry
   issue and every other issue are never guarded.
8. **Cost.** `done`, `block` and `ask` use the comments that they load anyway. `say` makes one
   extra read of the ticket's comments (the ticket's comment list is not loaded by `say`
   otherwise). An agent without a cursor (never watched) or with an unreadable cursor passes.

## Stale work


Two waits can last hours with no signal: a ticket whose CI is green for hours before it reaches
Done, and a ticket In Progress for hours with no pushed branch. The board must show both.

One table of expected times says how long each wait may last. One function finds the waits that
are over their time (late waits). One rule says who hears of a late wait. `status`, `poll`,
`watch` and the headless `tick` all use them.

1. **Waits.** Each late wait has a `since` time and an age.
   - `push` (event word `no-push`): the ticket is In Progress for longer than `push` and no branch
     `<dev_branch_prefix><agent>/<n>-...` is on GitHub ([Branches](tickets.md#branches)).
   - `done` (`green-not-done`): an open PR on such a branch has green CI (the judgement of [Evidence](tickets.md#evidence) and [Which CI runs count](ci.md#which-ci-runs-count))
     on its head for longer than `done`, and no signed `done` of the assignee names that head.
     The `since` time is when the last green part of the CI of that SHA completed.
     Not with `ci_mode: local` in `AUTHORITY.md`: there the CI of a head is no signal
     ([Local run](ci.md#local-run-ci_mode-local)).
   - `verify` (`review-not-verified`): the ticket is in Review for longer than `verify`. A ticket in Review waits
     for `verify`. A passing `verify` of the latest `done` on the CURRENT head of the open PR ends this wait: the
     verify report names that head (`head_now:`) and the PR still has it. A ticket that passed `verify` and waits
     for its merge (the coordinator holds it on purpose, or `merge` was refused for `mergeable=UNKNOWN`) is not late.
     A verify of an older head does not count: a push after the verify makes the ticket unverified again. The
     scan reads the ticket comments and the open PR list for a Review ticket that is over `verify`; a read that
     fails keeps the wait. A scan that cannot read (`poll` between two reads) raises no `verify` wait: the next
     reading scan judges the ticket, at most 5 minutes later. A wait that was told stays told. A ticket whose
     latest `done` of its assignee (the same `done` that `tick` reads; a `done` of another sender does not count) has a `protected-paths:` or `owner-route:` line has no `verify` wait: `verify` refuses it by
     design, and the `owner-merge` wait covers it. `status` shows the ticket as `verified, waiting for merge since <t>` (`--json`:
     `verified_waiting_since`; `waiting_for_verify` is then false). This is a display, not a late wait: no event,
     no note, no new table key. The headless `tick` reads no PR head: it takes any passing verify of the latest `done` as
     enough, and it tells the owner nothing of a ticket whose latest `done` has a `protected-paths:` or `owner-route:` line.
   - `take`: the ticket is Ready, assigned, and not taken for longer than `take`. Off by default (a sleeping agent is not a stall).
   - `answer`: the ticket is Blocked and its `blocked` message has no answer for longer than `answer`. On by default, after 4 hours.
   - `owner`: the ticket is Blocked and a question to the owner has no answer for longer than `owner`. On by default, after 1 day.
   - `owner-merge`: the ticket is in Review for longer than `owner`, and its latest `done` has a
     `protected-paths:` or an `owner-route:` line, so the owner merges. On by default, after 1 day (the `owner` time).
   - `heartbeat`: a running agent wrote no heartbeat for more than 3 intervals (late,
     [Fields](board.md#fields)). The `since` time is its last sign of life: the `Heartbeat` value,
     capped at the `updatedAt` of its item. If it never wrote one, it is the newer of its approval and
     its start. With no baseline at all nothing is judged. The wait of the coordinator is
     `heartbeat` at any age.
   - `dead`: a running dev agent wrote no heartbeat for more than 5 intervals. No ping and no pong
     is involved: the age of the heartbeat is the only input. The `since` time is the one of
     `heartbeat`. The interval is the heartbeat interval of the board (`heartbeat_interval` of the
     initiative file), not a key of `[late]`.
   The `since` time of a ticket in a status is the last change of its board item, as `age_s` of
   `status` shows it; for `answer` and `owner` it is the time of the question. A value that
   cannot be read gives no late wait; nothing is guessed.
2. **Table.** The optional table `[late]` of `.crewboard.toml`: `push` (default `2h`),
   `done` (`30m`), `verify` (`30m`), `escalate` (`30m`, item 8), `repeat` (`2h`, item 8), and the
   three waits for a person: `take` (off by default), `answer` (`4h`) and `owner` (`1d`, also the time of `owner-merge`). `take`, `answer` and `owner` also accept `off`, which turns the wait off; a key that is not set keeps its default. Each other value is a duration like `90`, `90s`, `30m`, `2h`; a bad
   value or an unknown key refuses the file. The older table `[stale]` (`no_push`, `green`, `review`,
   `escalate`, `repeat`) is still read and means `push`, `done`, `verify`, `escalate`, `repeat`.
   A file with both tables is refused with `late-and-stale-both-set`.
3. **`status`.** The section `late:` has a header with the number of late waits and the number of
   GitHub reads, then one line per late wait: the wait, the subject (ticket or agent), `since`, the age, the
   expected time and who is told now (`dev <agent>`, `coordinator`, `owner`, or `nobody yet`).
   The owner is named where item 8 says so; the `told` column shows the rule, not the delivery
   (the signed `info` of item 8 may be there already).
   With `--json`, the top level has `late` (a list of `wait`, `subject`, `since`,
   `age_s`, `expected_s`, `head_sha`, `pr`, `told`) and `late_github_reads`.
4. **`poll` and `watch`.** (`ci-finished` is in item 7.) One `stale-work` event (class TICKET, `kind: stale-work`) per wait and
   head SHA. The first line of the body is `stale-work: <case>` for the three old waits
   (`no-push`, `green-not-done`, `review-not-verified`) and `late: <wait>` for the others; the
   second line is `ticket: <owner/repo#n>`. The owning dev agent gets `no-push` and `green-not-done` for its own In Progress
   tickets (and `late: take` for its own Ready tickets). The coordinator gets `review-not-verified` for every ticket, and
   `late: answer`. The state cache keeps
   what was given (`<item id>|<case>` -> head SHA or `-`): a second poll says nothing, a new
   head SHA gives a new `green-not-done` event, and the entry goes when the ticket leaves the
   status of the wait. The coordinator also gets an escalated event for a dev
   wait (item 8). The state file keys `stale_seen`, `stale_checked`, `stale_tick_checked` and
   `stale_green` are read as `late_seen`, `late_checked`, `late_tick_checked` and `late_green`.
5. **Cost.** Tickets that cannot hold a wait cost nothing. `take` costs no read. `review-not-verified` costs the reads of the ticket comments (and the PR list) once per Review ticket over `verify`, in a reading scan only.
   For an In Progress ticket: one `matching-refs` read when it is over `push`; one PR list
   per repo; for the PR of the ticket the CI reads of [Evidence](tickets.md#evidence) (4 or 5) unless the green time of that
   SHA is cached; one read of the ticket comments when the CI is green long enough (`status`
   has them already); one read of AUTHORITY.md for the branch prefix. The waits `answer`, `owner`
   and `owner-merge` cost one read of the ticket comments each, and only while their time is not `off`. A wait that was given as
   an event for the current SHA is not read again. `poll` reads GitHub for the waits at most
   once per 5 minutes; between two reads it gives only `late: take`.
6. **What the agents do.** Dev, `no-push`: push, or `done`, or `block`. Dev, `green-not-done`:
   `done`, or push the rest, or `block`. Dev, `late: take`: `take`, or tell the coordinator why not.
   Coordinator, `review-not-verified`: `verify`. Coordinator, `late: answer`: answer the question, or tell the owner.
   Coordinator, escalated wait: see item 8.
7. **`ci-finished`.** A dev agent has no signal that the CI of its PR head has finished.
   Without one it waits in `gh run watch`, and a long runner queue uses up the wake. So `poll`
   and `watch` give one event `ci-finished` (class TICKET, `kind: ci-finished`) per PR head SHA.
   - **Who.** The dev agent that owns the In Progress ticket (the same PR match as for
     `green-not-done`: an open PR on a branch `<dev_branch_prefix><agent>/<n>-...`). The
     coordinator gets none, and `status` shows none. A ticket in Review gets none: its `done`
     is filed, so the next step is `verify`, and a red CI there is a merge matter ([Which CI runs count](ci.md#which-ci-runs-count)),
     not a wake of the dev. With `ci_mode: local` in `AUTHORITY.md` nobody gets it: the dev
     delivers with its local run ([Local run](ci.md#local-run-ci_mode-local)), and a check that
     runs anyway on a crew head (for example code scanning) is no proof of the tests.
   - **Fields.** The first line of the body is `ci-finished: <result>`, then `ticket:`, `pr:`
     (`owner/repo#n`) and `head:` (the full SHA). The result is `green`, `red` or `none`.
   - **Result.** The CI judgement of [Evidence](tickets.md#evidence) and [Which CI runs count](ci.md#which-ci-runs-count), not a new one. The CI is *finished* when every
     run, check and status of the head is completed. Then `green` is the judgement green, and
     `red` is any other judgement; the body names the first five reasons. A red run next to a
     queued or running one is not finished: the event waits for the last check. `none` means
     that no counted CI exists for the head (also: only information runs). GitHub needs some
     seconds after a push to register the runs, and then "no CI" looks the same. So `none` comes
     with the second "no CI" answer for the same head, one throttle period later. A run that
     shows up between the two gives its result instead. A CI source that cannot be read gives
     nothing, as in the waits.
   - **Cache and reads.** The state cache keeps `<item id>|ci-finished` -> head SHA (in
     `late_seen`). A second poll says nothing, a new head SHA gives a new event, and the entry
     goes when the ticket leaves In Progress. The reads are those of the waits (item 5): the PR
     list and the CI reads of [Evidence](tickets.md#evidence), kept for the run, so a head costs the same once for
     `green-not-done` and `ci-finished`. A head with its event is not read again. `ci-finished`
     has its own throttle, apart from the one of the waits: while the agent has a ticket In
     Progress, `poll` reads the PR list and, for a head without its event, the CI at most once
     per 60 seconds (`late_ci_checked`). A CI of ten seconds then gives its event within about a
     minute and not after the 5 minutes of the waits. The waits keep their 5 minutes. Nothing is
     read for a ticket that is not In Progress with an agent.
   - **Wake.** `poll --quiet` exits 0 for every event, so `crewboard wake` wakes the
     dev agent for this event with `CREWBOARD_WAKE_REASON=events`. No loop changes.
   - **What the agent does.** Push, then end the wake: do not wait in `gh run watch`. On
     `ci-finished green`: run `done` with the head of the event. On `red`: fix the cause and
     push (the next head gives a new event). On `none`: run `done` if the ticket declares
     `CI: none`; else check the workflow triggers. `green-not-done` stays the fallback for a
     `done` that was forgotten (`green` limit after the green time).
8. **Who hears.** One rule, `late::tells`. A wait that is over its expected time `E`
   goes to these ones:

   | Wait | Dev | Coordinator | Owner |
   |---|---|---|---|
   | `take`, `push`, `done` | at `E` | at `E + escalate`, again every `repeat` | never |
   | `verify` | never | at `E` | at `E`, when the coordinator is late (`heartbeat` or `dead`) |
   | `owner-merge`, `owner` | never | never | at `E` (default 1 day) |
   | `answer` | never | at `E` | at `E + escalate` |
   | `heartbeat` of a dev agent | never | never (shown only) | never |
   | `heartbeat` of the coordinator | never | never | at late (3 intervals) |
   | `dead` | never | the reassign to Backlog (an action, no message) | at dead (5 intervals) |

   A dev wait goes only to the dev agent first. If the dev does not act, or is not woken,
   nobody else sees it. So the coordinator's `poll` and `watch` give ONE event when a dev wait is
   still there `escalate` after its time. With the defaults, `no-push` reaches the dev at 2h and the coordinator at 2h30, and
   `green-not-done` reaches them at 30m and 60m.
   - **Keys.** `escalate` (default `30m`, the same for every wait) and `repeat` (default `2h`) in
     `[late]` (or `[stale]`). Same duration syntax and the same refusal of a bad value as the other keys.
   - **Event.** Class TICKET, `kind: stale-work`, `to` the coordinator. The first line of the
     body is `stale-work: <case>` (or `late: <wait>`), then these lines: `ticket:`, `escalated: true`, `agent:`,
     `age:`, `limit:`, `escalated after:`, and for `green-not-done` also `head:` (7 characters)
     and `pr:`. The line `escalated: true` is on its own line, so a prompt can match it. The dev's
     own event has no such line.
   - **When.** The wait holds (item 1) AND the ticket is In Progress for longer than
     `E + escalate` AND, for `green-not-done`, the CI is green for longer than
     `E + escalate`. The age of the ticket is a filter for the reads: a ticket that is too
     young for both waits costs no read.
   - **Once, and again.** The state cache of the coordinator keeps `<item id>|<case>|escalated`
     -> `<head SHA or ->|<time given>`. No event while the entry has the same head SHA and is
     younger than `repeat`. A new head SHA gives an event at once. The same wait gives one more
     event after `repeat`. The entry goes when the wait is gone (a branch is pushed, a `done`
     names the head, the CI is not green any more, no open PR) or when the ticket is not old
     enough any more or not In Progress. A read that failed neither gives an event nor removes an
     entry.
   - **Cost.** The coordinator reads with its own cache and throttle (at most once per 5 minutes),
     the same reads as the dev duty (item 5). Per due poll, for N In Progress tickets that are
     old enough: one AUTHORITY.md read, N `matching-refs` reads, one PR list per repo. Then, only for a ticket with an open PR on a crew branch:
     the CI reads of [Evidence](tickets.md#evidence) once per head (the green time is cached), and one read of the ticket comments per poll while
     the wait holds. For N = 3 candidates among 5 In Progress tickets the cost is
     1 + N + 1 reads; the two young tickets cost none. A dev and a coordinator on one host have separate caches, so both read.
   - **Headless `tick`.** `crewboard tick` logs one line per escalated wait:
     `left for the interactive coordinator: <ticket>, stale-work escalated: <case> for <age>, agent <agent>; ...`
     (`late: <wait>` instead of the `stale-work escalated` words for the waits `take` and `answer`).
     It writes nothing on the board or GitHub for these lines (the owner `info` of item 9 is the one write of the tick for late waits). It keeps its own entries (`...|escalated-tick`),
     so a line comes once per wait and head SHA and
     again after `repeat`. It has its own read throttle (`late_tick_checked`), apart from the one of `poll`. The tick resolves an old In Progress ticket that the cache does not
     know with the same scan as for a Review ticket.
   - **What the coordinator does.** Ask the agent with `say`, or `reassign` the ticket, or
     `ticket split` it when it is too big for one wake.
9. **The owner hears.** The owner column of item 8 is delivered as ONE signed `info` to
   `owner`: in the registry issue for a wait of an agent (`heartbeat`, `dead`), on the ticket for a
   wait of a ticket. The first line is `late: <wait> <subject>`. The next lines are
   `late-key: <wait>:<ticket or agent>:<since>`, `since: <utc>` and `next: <command or person>`.
   Then comes the text for a person: for the coordinator, that it shows no sign of life and that
   nobody assigns, answers or verifies until it is back, and the fix (restart the
   coordinator session, or `crewboard approve <agent> --role coordinator`); for `verify`, that
   the ticket was not verified and the coordinator is away, and the coordinator heartbeat; for
   `dead`, the report of the coordinator (`agent-dead: ...`). For `dead` the last part of the
   key is the heartbeat value that the agent was last seen with, so one dead episode has one key
   and an agent that comes back and dies again has a new one. The note
   changes no Status and refuses nothing.
   - **Who posts.** Whoever computes the wait and finds no signed `info` to `owner` with the same
     `late-key:` line in the log: the coordinator's `poll` and `watch` (the waits of tickets), the
     headless `tick` (a Review ticket that waits for `verify` while the coordinator is late), the
     coordinator when it reassigns the tickets of a dead agent, and a running dev agent in each
     cycle for a late coordinator (a dev agent never posts about itself, about another dev agent
     or about a ticket, and never does coordinator work). The coordinator never posts about
     itself. With no coordinator on the board, nothing is judged.
   - **Once.** The key holds the time since which the wait holds. A ticket that leaves Review and
     returns has a new `since`, so a new key. A coordinator that comes back writes a new
     heartbeat, so its next late period has another key. The key of a `verify` note also holds
     the baseline of the late coordinator, so a coordinator that comes back and goes away again
     on the same Review ticket gives a second note, and the same late period gives none. An
     agent that goes late and then dead gets two notes (`heartbeat` and `dead` have two keys).
     A signed `info` to `owner` with the key from any approved agent counts as posted; a plain
     comment with the same text does not.
   - **The race.** Two agents that compute the same wait at the same moment both find no note
     and both post. This duplicate is accepted and the only one: a check-then-post on GitHub
     comments cannot be atomic. On their next cycle all agents see the notes and post nothing.
   - **No wake.** The `heartbeat` note of a dev agent is no event: `poll --quiet` does not exit
     0 because of it, `watch` does not print it, and no model is woken. The agent writes one line
     to stderr.
   - **Cost.** No extra read for the agent waits: the `State` and `Heartbeat` are on the agent
     items that the cycle reads, and the approval is in the registry comments it loaded. The
     check for an earlier note reads the ticket comments (the `tick` has them already) and only
     when the owner must hear of a wait now.

## Event receipts


`poll` moves the cursor, so an event that a wake with exit 0 did not act on is lost. The wake
`crewboard wake` (with `--events-file F`) keeps a receipt of each event. It works for every
runtime, because it runs in the wake command of crewboard and in the CLI, not in a hook of the runtime.

1. **Delivered.** When the wake starts, the log has one line for each event that F hands to the
   wake: `event delivered to wake: <repo>#<n> <kind>`. An event without a ticket logs its kind only.
   A finished detached job logs `#<n> detached-job-finished`.
2. **Which events.** F holds the events of the first `poll`, any section that a hook appends later
   (`== Events delivered during the wake (crewboard poll, <utc>):`) and the lines of finished
   detached jobs. The check reads all of them. It reads the blocks of the `poll` output
   (`== [n] LABEL ==`, then `from: ... to: ... kind: ...`, `where: <repo>#<n> ...`, `ts: ...`).
3. **What needs an action.** A ticket event `assignment` or `stale-work`; a message to the agent
   (not to `all`) of kind `request` (a task, a failed verify, a re-delivery), `question`, `answer`,
   `done` or `blocked`; an owner message; a finished detached job. No action is needed for `ack`,
   `report`, `info` (for example a merged PR or a closed ticket), `ping`, `pong` (retired kinds,
   old comments only), `status-changed`, `unassigned`, `ci-finished`, registry events and liveness events.
4. **What counts as acted.** After the later of the event time and the start of the wake, the agent
   posted a signed message on that ticket. Each command that writes a Status (`take`, `done`,
   `block`, `ask`, `unblock`) and `say` post one, so a Status change by a crewboard command is
   seen without a board read. A Status change that no signed message follows is not seen.
5. **Not acted on.** After a wake with exit 0, `crewboard acted --since <wake start>` names each
   such event. The script appends the event once to `<F>.unhandled`, behind the marker line
   `(crewboard: this event was delivered to an earlier wake, which did not act on it. It is
   delivered again once.)`, and logs `event not acted on: <repo>#<n> <kind>; delivered again`. The
   next wake gets it at the top of its F, as for a failed wake.
6. **Second miss.** The marker line in F says that the event came once already. If the agent
   still does not act, the log has `event not acted on twice: <repo>#<n> <kind>; not delivered
   again` and nothing more is written. No state file is needed: the marker travels in F.
7. **Failed wake and budget stop.** They keep their own path (the whole F, or the unread events,
   to `<F>.unhandled`) and run no check. An event of a failed wake that comes again and is missed
   is delivered once more, because the marker is written by the check only.
8. **Read cost.** No event that needs an action: no read. Else the check loads the agent and the
   registry (the reads of any command, 4 requests in the fake GitHub) and reads the comments of
   each ticket that an event names, ONE request per ticket (at most 25 tickets; a ticket of a repo
   outside the initiative is skipped). `--list` reads nothing. A ticket that cannot be read counts
   as handled and gives one line on stderr. A check that fails logs
   `event check failed: crewboard acted exit <n>: <reason>` and delivers nothing again.
