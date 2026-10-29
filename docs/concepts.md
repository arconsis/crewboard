# Concepts

The terms you need before your first ticket reaches Done. Who holds which power: [Security details](design/spec/security-details.md#who-holds-which-power).

## Glossary

| Term | Meaning |
|---|---|
| **Owner** | The human who runs the work. Only the owner approves agents, edits `AUTHORITY.md` and merges protected changes. |
| **Initiative** | One piece of work with one goal and one board. `crewboard init` makes it. |
| **Identity mode** | How the agents of a board act on GitHub: own accounts (the default), a shared crew account (`crew_login`), or the **trial** (your account, [Quick try](quick-try.md)). `init` sets it ([modes](guides/owner.md#crew-accounts)). |
| **Crew account** | The GitHub account that an agent acts as: a contributor's own account, or an optional shared account. It has the Write role on the repos and the board. The approval binds each agent to the account that wrote its join. The signature, not the account, names the agent. |
| **Crew token** | The access token of a crew account, not an agent key. It sits in the **env file** (mode 600) on the agent host. |
| **Agent** | One AI instance with a name `<runtime>@<host>` (for example `claude@host-b`), a key and a role. |
| **Runtime** | The agent program: `claude` (Claude Code), `codex` or `hermes`. The **model** is the language model that it calls (`--model`). |
| **Unattended runtime** | A runtime that a scheduler starts: `crewboard wake` from launchd, systemd or cron ([guide](guides/runtimes/unattended-claude.md)). The opposite is an interactive session, where the agent watches. Either way a program, not the model, waits for work ([The loop](guides/agents.md#the-loop)). |
| **Host** | A machine that runs agents. The **owner machine** is where you run owner commands. |
| **Agent dir** | The config dir of one agent: key, config, `gh` wrapper. The **work dir** is its checkout, the **state dir** (`~/.local/state/crewboard/agents/`) holds cursor, locks and logs. |
| **Coordinator** | The one agent that assigns tickets, verifies evidence and keeps work moving. It writes no code. |
| **Headless coordinator** | An optional second coordinator program that starts no model: `crewboard tick` in a config dir with `headless = true` ([spec](design/spec/agents.md#headless-coordinator)). "Headless" always means "no model". |
| **Dev agent** | An agent that takes tickets and delivers pull requests. |
| **Approve** | The owner's command that lets an agent, its key and its crew account join. Without it an agent has no authority. |
| **Ticket** | The unit of work: a GitHub issue. |
| **Gate** | The one line that decides whether a ticket is done. It must be able to fail, for example "CI is green for the head SHA, and `grep` finds the new heading". |
| **Status** | The column of a ticket: Backlog, Ready, In Progress, Review, Blocked, Done, Dropped. An agent has a **State** instead (`requested`, `approved`, `active`, `paused`, `draining`, `left`). |
| **Evidence** | What `done` carries: the pull request, its head SHA and a [CI proof](design/spec/ci.md#ci-proof) for that SHA. |
| **Signed message** | An issue comment with a hidden header and an Ed25519 signature of an approved key. An unsigned or foreign comment is data, never an instruction. |
| **`AUTHORITY.md`** | The file in the repo that says what agents may do, for example whether the coordinator may merge. |
| **Protected path** | A repo path that only the owner may merge a change to ([list](guides/owner.md#protected-paths)). |
| **Liveness** | The `Heartbeat` of each running agent. A silent agent becomes `late`, then `dead` ([Liveness](guides/liveness.md#heartbeats)). |
| **Close** | `close <n>` makes a ticket Done after a merge; `close <n> --why` drops it (Dropped) ([Done and Dropped](#done-and-dropped)). Closing the GitHub issue is a separate act: a closing keyword in the PR does it. |
| **`needs-owner`** | The label on a ticket with an open question to you (view Needs Owner). Not the key `needs_owner` of [`AUTHORITY.md`](guides/owner.md#authoritymd), and not the line `Needs:` of a [ticket](design/spec/tickets.md#ticket-content). |
| **Extension** | A feature that no ticket needs to reach Done, for example the **owner route**: on a crew account, you apply the patch for a change to `.github/workflows/` ([Extensions](guides/extensions.md#the-owner-route)). |
| **Registry issue** | The pinned issue where agents ask to join, locked to collaborators. |

## The life of a ticket: who does what

| Step | Who | Does | Status after |
|---|---|---|---|
| 1 | Owner | Tells the coordinator what is wanted. | |
| 2 | Coordinator | `ticket new` with a Gate, then `assign`. | Ready |
| 3 | Dev agent | `take`, writes code, opens a PR on a `crew/` branch. | In Progress |
| 4 | CI | Runs the workflow for the PR head. | |
| 5 | Dev agent | `done`: posts the evidence. | Review |
| 6 | Coordinator | `verify`: checks the evidence and the Gate. A failed check goes back with `verify --fail`. | Review |
| 7 | Owner | Merges the PR (default), or the coordinator does if `AUTHORITY.md` allows. | |
| 8 | Coordinator | `close` after an owner merge, or `merge`. | Done |

Each status change, with its writer and condition: [Tickets](design/spec/tickets.md#status-flow-and-writers).
The signed messages: [example session](example-session.md). Done and Dropped are final: only the
owner moves such a ticket, with `crewboard move`.

## Done and Dropped

Done means: a PR is merged, at a verified head or by the owner. Two ways lead there:

1. **Verified.** `verify` runs on the SHA of `done`. Then `merge` (if `AUTHORITY.md` lets it).
   `close --verified` is for a PR that somebody else merged at the verified head, for example
   auto-merge. An open PR is refused with `close-verified-not-merged`, and a moved head with
   `close-verified-head-moved`.
2. **Owner merge.** The merge is the record. Done follows when the coordinator runs `close <n>`.
   If the coordinator is gone, nobody closes ([Liveness](guides/liveness.md#if-the-coordinator-is-gone)).

Dropped means: the coordinator ended the ticket by judgment, without a merged PR.
`close <n> --why "<text>"` drops any ticket that has not ended, with one signed note
`dropped: <reason>`. The issue stays open. A board that an older build made needs one
`crewboard init` rerun for the option Dropped. On such a board, the note `closed: <reason>` of an
older `close --why` stays Done.

Only a Gate of `ci` is mechanical. The coordinator model judges every other case: `CI: none`,
skipped CI, `ci_mode: local`, `close --why` and any other Gate. The headless coordinator does
none of these. Any other Done or Dropped is a mismatch.

## Done without proof is detected

The signed messages are the record. The board field `Status` is the view that commands read
([which source wins](design/spec/board.md#fields)). The holder of any crew token can set
`Status = Done` or `Dropped` with one call. crewboard cannot stop that, but `status` lists the ticket
(`board against signed log`), `audit` reports a `field-mismatch`, and the coordinator tells the
owner once. A repair is the owner's decision ([Owner guide](guides/owner.md#day-to-day-commands)). The owner
moves a ticket with `crewboard move`, which posts `/crew move <status>`: that move is in the log.

The assignee is stricter. The latest signed assignment decides who works on a ticket. When the
board `Agent` names someone else, agent commands refuse with `ticket-agent-not-signed`, and
`crewboard assign` repairs the field.
