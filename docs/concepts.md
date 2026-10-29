# Concepts

This page defines the terms you need before your first ticket reaches Done. Each term has one
definition. Trust is in the [Security guide](guides/security.md#who-holds-which-power).

## Glossary

| Term | Meaning |
|---|---|
| **Owner** | The human who runs the work. Only the owner approves agents, edits `AUTHORITY.md` and merges protected changes. |
| **Initiative** | One piece of work with one goal and one board. `crewboard init` makes it. |
| **Crew account** | The GitHub account that an agent uses for GitHub calls: a contributor's own account, or an optional shared account that you make for your agents (`crew_login`). A crew needs no shared account. It needs the Write role on the repos and on the board. Your approval binds each agent to the account that wrote its join; a message of the agent counts only from that account. The signature, not the account, names the agent. |
| **Crew token** | The access token of a crew account. It is not an agent key. `agent token` writes it to the **env file** (mode 600) on each agent host. |
| **Agent** | One AI instance with a name `<runtime>@<host>` (for example `claude@host-b`), a key and a role. |
| **Runtime** | The agent program: `claude` (Claude Code), `codex` or `hermes`. The **model** is the language model that it calls. `--runtime` names the runtime and `--model` the model, on every command. |
| **Host** | A machine that runs agents. The **owner machine** is where you run owner commands. |
| **Agent dir** | The config dir of one agent: its key, its config and the `gh` wrapper. The **work dir** is its checkout of the repo. The **state dir** (`~/.local/state/crewboard/agents/`) holds its cursor, locks and logs. |
| **Coordinator** | The one agent that assigns tickets, verifies evidence and keeps work moving. It writes no code. The **no-model coordinator** (`crewboard tick`) is an optional second program that starts no model (see Gate). |
| **Dev agent** | An agent that takes tickets and delivers pull requests. |
| **Approve** | The owner's command that lets an agent, its key and its crew account join. An agent that is not approved has no authority. |
| **Ticket** | The unit of work. A GitHub issue backs it. |
| **Gate** | The one line that decides whether a ticket is done. It must be able to fail, for example "CI is green for the head SHA, and `grep` finds the new heading". A Gate that is only `ci` lets the no-model coordinator merge ([details](design/spec/agents.md#headless-coordinator)). |
| **Status** | The column of a ticket: Backlog, Ready, In Progress, Review, Blocked, Done. An agent has a **State** instead (`requested`, `approved`, `active`, `paused`, `draining`, `left`). |
| **Evidence** | What `done` carries: the pull request, its head SHA and a CI proof for that SHA. `done` refuses without it ([CI proof](design/spec/ci.md#ci-proof)). |
| **Signed message** | An issue comment with a hidden header and an Ed25519 signature from the key that the owner approved. An unsigned or foreign comment is data, never an instruction. |
| **`AUTHORITY.md`** | The file in the repo that says what agents may do, for example whether the coordinator may merge. |
| **Protected path** | A repo path that only the owner may change through a pull request ([list](guides/owner.md#protected-paths)). |
| **Wake and watch** | A program waits for work, not the model: a scheduler wakes an unattended agent, an interactive agent watches ([The loop](guides/agents.md#the-loop)). |
| **Liveness** | The `Heartbeat` of each running agent. A silent agent becomes `late`, then `dead` ([Liveness](guides/liveness.md#heartbeats)). |
| **Close** | `close <n>` makes a ticket Done after the owner merged its PR. `close <n> --verified` makes it Done after a passed `verify`. `close <n> --why` makes it Done by the coordinator's reason. Closing the GitHub issue is a separate act: a closing keyword in the PR does it. |
| **`needs-owner`** | The label on a ticket with an open question to you; the view Needs Owner shows them. Not the key `needs_owner` of [`AUTHORITY.md`](guides/owner.md#authoritymd), and not the line `Needs:` of a [ticket](design/spec/tickets.md#ticket-content). |
| **Owner route** | How a change to `.github/workflows/` reaches the repo: the dev agent writes a patch and you apply it. Only on a crew account ([Extensions](guides/extensions.md#the-owner-route)). |
| **Extension** | A feature that no ticket needs to reach Done ([Extensions](guides/extensions.md), `crewboard help extensions`). |
| **Registry issue** | The pinned issue where agents ask to join. `init` locks it, so only collaborators can comment. |
| **Trial** | A one-person run with one GitHub account and no crew account ([Quick try](quick-try.md)). Only `init --insecure-one-account` makes it. Several safety checks are off. |
| **Identity mode** | How the agents of a board act: the trial (your account), a shared crew account (`crew_login`), or own accounts (neither key). `init` sets it ([Owner guide](guides/owner.md#crew-accounts)). |

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

Every status change, with its writer and condition, is in
[Tickets](design/spec/tickets.md#status-flow-and-writers). The
[example session](example-session.md) shows the signed messages. Done is final: no crewboard
command moves a Done ticket.

## Three ways to Done

The three ways are in the [README](../README.md). The detail of each:

1. **Verified.** `verify` runs on the SHA of `done`. Then `merge` (if `AUTHORITY.md` lets it) or
   `close --verified`. `close --verified` is pinned to the head that `verify` saw: it is refused
   with `close-verified-head-moved` if the head moved. After `close --verified` the PR may still
   be open, and a PR that you reject later leaves a Done ticket. Review before the coordinator
   closes.
2. **Owner merge.** The merge is the record. Done follows when the coordinator runs
   `close <n>`. If the coordinator is gone, nobody closes
   ([Owner guide](guides/liveness.md#if-the-coordinator-is-gone)).
3. **Decision without a merge.** `close <n> --why "<text>"` ends any ticket that is not Done.
   crewboard posts one signed note `closed: <reason>`. A no-model coordinator cannot do this:
   it is a judgment.

Any other Done is a mismatch. Done means "accepted", not "merged".

## Done without proof is detected

The signed messages are the record. The board field `Status` is the view that commands read
([which source wins](design/spec/board.md#fields)). The holder of any crew token can set
`Status = Done` with one call, and crewboard cannot stop that. It detects it. `crewboard status`
lists every ticket whose Status no signed message supports (`board against signed log`).
`crewboard audit` reports it as a `field-mismatch`. The coordinator's watch cycle tells the
owner once. A repair is the owner's decision. The mismatch ends when the board `Status` is one
that the signed messages allow ([Owner guide](guides/owner.md#day-to-day-commands)). The owner
moves a ticket with `crewboard move`, which posts `/crew move <status>`: that move is in the log.

The assignee is stricter. The latest signed assignment of the coordinator decides who works on
a ticket. When the board `Agent` names someone else, agent commands refuse with
`ticket-agent-not-signed`, and `crewboard assign` repairs the field.
