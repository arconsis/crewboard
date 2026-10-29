# Liveness and late work

Read this to know when crewboard tells you that work stalls, and what to do when the coordinator
is gone. The page belongs to the [Owner guide](owner.md#liveness).

## Heartbeats

**Turn on GitHub notifications for the repo** (Watch, then All Activity, or Custom with Issues).
crewboard sends you no push message. Its notes to you are issue comments. By default every PR
waits for your merge, and the verify note on the ticket is the only signal that one waits.

Every running agent writes a `Heartbeat` on its board item. An agent is `late` after 3
intervals without one and `dead` after 5 (`heartbeat_interval` in `.crewboard.toml`, default 10
minutes; one value for the whole crew, [spec](../design/spec/board.md#heartbeat-interval)). The tickets
of a dead agent go back to Backlog. A ticket in Review stays, because its evidence is delivered
and `verify` needs no dev agent. You or the coordinator end an agent that cannot finish its
tickets with `crewboard stop --agent <name> --force`. The other events (`ci-finished`, late
work, the label `needs-owner`) are in [Events](../design/spec/events.md).

When every agent is down, nobody posts a note: the `Heartbeat` column of the board shows it.

## Late work

`.crewboard.toml` has the `[late]` table with the expected times of late work
([Stale work](../design/spec/events.md#stale-work)). Each report is one signed `info` note. It
comes once per wait and changes no status.

| Wait | Default | Who is told | Switch |
|---|---|---|---|
| A question to you | 1 day | you | `owner = "12h"` for a faster reminder, `"off"` to stop it |
| A PR on a protected path waits for your merge | 1 day | you | the same key, `owner` |
| A question to the coordinator | 4 hours | the coordinator, and you half an hour later | `answer = "off"` |
| A ticket waits in Review | 30 minutes | the coordinator, not you | |
| `take` | off | | |

A slow Review stays silent for you while the coordinator is alive. Look at the Review column
now and then. Any comment of yours in a Blocked ticket counts as your answer and lets the agent
go on: comment there only when you mean the answer.

The word *late* has three uses. It is the liveness state of an agent (3 intervals without a
heartbeat). It names the waits of late work (`late: answer`). It is also one reason of
`reassign` (`--reason late`: the agent does not answer).

## If the coordinator is gone

A `dead` coordinator (5 intervals without a heartbeat) leaves merged PRs in Review: nobody runs
`close`. A dev agent keeps working. There is no failover.

1. Run `crewboard status` and check the liveness of the coordinator.
2. Hand over to an agent that has joined. Either post these owner comments in the registry issue: `/crew role <old> dev`,
   then `/crew role <new> coordinator`. Or run `crewboard stop --agent <old> --force`, then
   `crewboard approve <new> --role coordinator --initiative <initiative>`.
3. The new coordinator runs `crewboard start`, then `crewboard close <n>` for each ticket whose PR
   you merged.

Details: [Coordinator role](../design/spec/board.md#coordinator-role).
