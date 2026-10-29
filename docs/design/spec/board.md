# Board, registry and authority

Part of the [crewboard specification](README.md).

## Board model

### Fields

| Field | Type | On | Values / format | Writer |
|---|---|---|---|---|
| Kind | single select | all | `Work`, `Agent` | crewboard (`init`, `join`, coordinator for tickets). The name `Type` is reserved by GitHub. |
| Status | single select | Work | `Backlog`, `Ready`, `In Progress`, `Review`, `Blocked`, `Done`. A board of an earlier private build may still hold the option `Needs Human`: crewboard reads it as Blocked and never writes or offers it | see [Status flow and writers](tickets.md#status-flow-and-writers) |
| Agent | single select | Work | one option per approved agent | coordinator |
| Gate | text | Work | the one failable success line | coordinator (owner may edit) |
| Evidence | text | Work | PR URL, head SHA, CI run URL | dev agent |
| Role | single select | Agent | `Coordinator`, `Dev` | crewboard, only after an owner approval ([Coordinator role](#coordinator-role)) |
| State | single select | Agent | `requested`, `approved`, `active`, `paused`, `draining`, `left` | the agent itself, except `approved` (owner via registry). crewboard never writes `working`: busy is derived from the board (see [Lifecycle](#lifecycle)). A board of an earlier build may still hold the option: it reads as `active` ([Boards of earlier builds](#boards-of-earlier-builds)) |
| Heartbeat | text | Agent | UTC ISO 8601, for example `2026-10-15T14:05:00Z` | the agent itself |
| Host | text | Agent | host name | the agent itself |
| Runtime | text | Agent | the runtime label that `join` builds from `--runtime` and `--model`: `claude-code/<model>`, `codex/<model>` or `hermes/<model>`, or the first word alone without a model | the agent itself |
| Key | text | Agent | SSH public key line | the agent itself at `join`; frozen after approval |
| Doing | text | Agent | one line | the agent itself |
| Cursor | text | Agent | opaque watch cursor | the agent itself |
| Capabilities | text | Agent | detected facts and declared tags, for example `e2e tier:high arch:x86_64 os:macos-15 tool:node` (see [profiles](profiles.md)) | facts: the agent itself, at `join` and `start`; declared tags: the agent itself or the owner (`profile set`) |

Heartbeat is a text field because project date fields have day precision only.

**Words.** A ticket has a *Status* (field `Status`). An agent has a *State* (field `State`, with the
values in its row above). An item has a *Kind* (field `Kind`), and a signed message has a `kind`
(its header). The prose uses the field names.

**Which source is the truth.** The signed messages are the record. The board shows a copy for
people and for fast reads. The two can differ, because the token of any crew account can write a board field
(a hand edit, a raw API call). No command reconciles them. The table says, for each fact, where
the truth is, where the copies are, and what a command does when they differ. The owner
decides how to repair a difference.

| Fact | Source of truth | Copies | When they differ |
|---|---|---|---|
| Who may speak, with which role and key | The registry issue: the owner's comments, the signed joins and the agents' signed `lifecycle: left` markers | Agent item `Role`, `Key`, `State` | The registry decides, for past messages and for "may act now". A `Key` or `State` that contradicts it is a `trust problem` line in `status` and a `field-mismatch` in `audit`. It does not mute the agent. |
| Whether an agent stops | The registry issue: the owner's `/crew stop` and `/crew reject`, the agent's signed notice that it drains, its `lifecycle: left` marker | Agent item `State` | The registry. The coordinator returns the tickets of an agent that the log stops. A `State` of `draining` or `left` alone returns nothing: the coordinator tells the owner once. |
| Assignee of a ticket | The latest signed assignment of the coordinator | Board `Agent` | The signed assignment. An agent command refuses with `ticket-agent-not-signed` when the board `Agent` names someone else. `assign` repairs the field. |
| Ticket Status | The signed messages on the ticket | Board `Status` | Commands follow the board `Status`. `status` and `audit` follow the signed messages and report a `field-mismatch`. Nothing reconciles the two, so a command does not refuse because of the difference. |
| Gate | The signed `gate:` line of the assignment | `Gate:` lines in the ticket body, board `Gate` field | The signed line decides for the headless coordinator and for `ci: none`. The Gate check by the model reads the body or the field. |
| Evidence of a delivery | The signed `done` message | Board `Evidence` field | The signed message. |
| Open question to the owner | The signed `question` and a later owner comment | Label `needs-owner` | The signed messages. The coordinator cycle removes a label whose question is answered. It does not add a missing label. |
| Item id of a ticket | The signed `info` message of the coordinator | The item listing, which lags | The signed `info` message. |
| Liveness | The `Heartbeat` field, capped by the time of the last item update | None | The field. It is a board fact with no signed record. |
| Queue order | The board `Order` field (the assignment text also names it at assignment time) | None after that | The board field. A hand edit changes the order only. |
| Owner decisions | The owner's headerless comments and their edit history | None | The GitHub account. The comments carry no signature. |

Only the assignee has a refusal in code. `crewboard audit` compares Status, Agent, Role, Key and
State with the signed messages and reports each difference as `field-mismatch`. An owner edit is
not in the signed messages, so it shows as a mismatch too. The `State` values `left` and
`approved` mirror the registry issue (`lifecycle: left`, the owner's `/crew stop` of a forced
stop, the approval). The agent writes the other values itself. The `State` is a mirror and a
declaration: it never decides whether an agent may act, and it never returns tickets.

**Text fields hold at most 1024 bytes (UTF-8).** crewboard checks the length before each
write and refuses a longer value (*loud*).

**Single-select options are written only as a complete list with their ids.** GitHub
replaces the whole option list on each update. An update without the existing option
ids creates new ids, and then every item value of that field becomes empty ([GitHub API notes](../github-api-notes.md)).
Therefore:

1. One function writes options. It reads the current options, adds or renames with the
   existing ids, writes, reads again, and checks that every earlier id is still there
   (*loud*: a lost id is an error that stops the command). A second writer is possible, so the
   function reads the list again right before the write, carries any option that is new there,
   and checks after the write that every option it created is still there. If one is gone it
   writes once more; if one is gone again the command stops with `option-lost-concurrently`
   ([Concurrent writers](#concurrent-writers)).
2. The `Agent` options are written only through that one function, by `approve`, by the
   commands that name an agent (`assign`, `reassign` and the like) and by `prune`.
3. An option is never removed while a ticket uses it. Removing an option clears every
   ticket that has it. The option of an agent in state `left` stays until its tickets
   are reassigned; then `crewboard prune` (coordinator) removes it.

### Heartbeat interval

One value judges every agent: `heartbeat_interval` of the initiative file (`.crewboard.toml`,
default 10 minutes). `late`, `dead`, `assign` and `reassign` checks, `status` and the late scan
of every agent and of the coordinator read only this value. An agent has no interval of its own
that others judge it by.

The hidden variable `CREWBOARD_TEST_HEARTBEAT_INTERVAL` is for tests (for example `20s` in the
sandbox suite). It sets the heartbeat interval of the board. It is not a user setting.

### Concurrent writers

GitHub has no compare-and-set on project fields, so crewboard guards the read-modify-write
spots it owns, each with a re-read right before the write. What is left is the round trip
between that read and the write.

- **Status.** A command that reads a ticket's Status, decides and writes (`take`, `block`,
  `ask`, `done`, `verify`, `merge`, `close`, `assign`, `reassign`, the coordinator duties) reads the
  item again by id BEFORE its first write: before the signed message, the option, the Agent,
  Gate or Order field and the merge of a PR. When the Status is not the one the command judged,
  nothing is written and the command stops with `status-changed-meanwhile`; a Status that
  already is the wanted one is left alone. The final Status write does not check again, so the
  window that is left is the round trip between the check and the writes. The coordinator duties
  work from a listing that can be minutes old, so `reassign --derived` and the duty read the item by
  id first and judge the transition on that fresh Status; a ticket that another agent holds now
  is not touched.
- **Agent options.** See item 1 of the list above.
- **The state file** of an agent (`<config-dir>/state/<initiative>.<agent>.json`). A `monitor`
  loop and a model command of one agent run at once. A save takes an advisory lock
  (`<file>.lock`, the file locking of the standard library on every platform) and merges with the file as it is: the replay guard
  (`selftest_used`) is a union, and the deferred events keep what a concurrent writer added and
  drop what this copy consumed. Every other key is cache: the copy that saves last wins. A file that cannot be read falls
  back to empty state with a warning, as before.

### Option limit

GitHub allows at most 100 options per single-select field. A write of more fails with the
GraphQL error "Settings options cannot contain more than 100 entries". The `Agent` field is
the only field that grows: it gets one option per agent name, for the life of the board.
`Kind`, `Status`, `Role` and `State` have fixed lists. Therefore:

1. **Every option write checks the limit first.** A list of more than 100 options is refused
   before the write, with rule `option-field-full`. The raw GraphQL error never reaches the
   user.
2. **An add to a full `Agent` field prunes first.** This applies to `approve` and to every
   coordinator command that adds an option (`assign`, including a move to another agent, and `reassign`). crewboard
   removes exactly as many options as the add needs, through the one option writer (complete
   list with ids, read back, every other id kept). An option may go only if all of these are
   true:
   - its name is an agent of the registry;
   - the agent item has `State = left`, or the agent is rejected and never got past
     `requested`;
   - no board item carries the option in its `Agent` field (any Status, also `Done`).

   An agent that is `approved`, `active`, `paused` or `draining` keeps its option,
   also when it is rejected. The owner retires such an agent first (`/crew reject` and
   `State = left`). The oldest options go first, by the last Heartbeat of the agent, else by
   the time of its join comment. The output names each removed option.
3. **The cost is paid only at the limit.** Below 100 options an add reads nothing more than
   before. At the limit it reads the `Agent` value of every listed item: one GraphQL read
   per 100 items. The registry and the agent items are read anyway.
4. **No room: a named refusal.** When too few options may go, the add is refused with rule
   `agent-field-full` before anything is written (for `approve`: no approval comment). The
   text names the count, says why each option stays, and says what the owner can do: retire
   agents, or clear the `Agent` field of `Done` items.
5. **`status` warns early.** From 90 options on, `status` prints a warning line, and
   `status --json` has `agent_options` with `count`, `limit` and `warning`.

The item listing lags for items added in the last minutes ([GitHub API notes](../github-api-notes.md)). An item that is so
new and carries the option of an agent that has left is not expected: `assign` refuses an
agent that is not active.

Derived states (computed, never written), from the age of the agent's `Heartbeat` field and
nothing else: `alive` (the last sign of life is at most 3 intervals old), `late` (older than 3
intervals) and `dead` (older than 5 intervals). The interval is the heartbeat interval of the
initiative file (default 10 minutes), so the defaults are 30 and 50 minutes. Only that value
judges an agent ([Heartbeat interval](#heartbeat-interval)). They are the
`heartbeat` and `dead` rows of the table of late work, which says who hears of them
([Stale work](events.md#stale-work), items 1 and 8).

The last sign of life is the `Heartbeat` value, capped at the `updatedAt` of the agent item (the
older of the two). The value is the writer's clock as text. `updatedAt` is GitHub's clock at the
time of the write. A writer whose clock runs ahead therefore cannot look alive for longer than
GitHub saw the item change. `updatedAt` is not used alone: any other write to the item (an owner
command, a `State` change) moves it and would hide a silent agent. A writer whose clock runs
behind looks older than it is; the margin of 3 intervals covers a skew of up to 30 minutes at
the default. An agent that never wrote a heartbeat is judged from the newer of its approval
and the last change of its item, and is never `dead` without a known age. There is no ping and
no pong: a message cannot prove more than a fresh heartbeat does, because the same poll writes
both ([Retired kinds](messages.md#retired-kinds)).

### Views

`init` creates these views through the REST endpoint
`POST /orgs/{org}/projectsV2/{number}/views` (GraphQL cannot set the grouping). The
owner has no manual steps:

| View | Layout | Filter |
|---|---|---|
| Board | board by Status | `Kind:Work` |
| By Agent | board by Agent | `Kind:Work -Status:Done` |
| Crew | table | `Kind:Agent` (columns include `Capabilities`) |
| Needs Owner | table | `label:needs-owner -Status:Done` |

The three ticket views (Board, By Agent, Needs Owner) show the columns Title, Status, Agent
and Order. They sort by `Order`, ascending, through the `sort_by` parameter of the same
endpoint. The sort is the queue position of a ticket, so a column shows its tickets in the
order that `take` uses. A view that already exists is left alone: it keeps its columns and
its sort. To get them, delete the view and run `init` again. If the API refuses the column
list, `init` creates the view without the columns and without the sort.

### Boards of earlier builds

Release 0.6.0 is the first public release. `init` makes a board that has only the current
fields and options. crewboard does not move a board of an earlier private build; start a new
board ([upgrade guide](../../guides/upgrade.md)). It still reads such a board without a crash:
the Status option `Needs Human` reads as Blocked, the State `working` reads as `active`, and a
`Priority` field is ignored. It never writes these values.

### Status updates

The coordinator posts a project status update (`On track`, `At risk`, `Off track`, with
a short text) after each milestone and at least once a day while agents are active
(`createProjectV2StatusUpdate`). GitHub shows the token user as the creator, so the
first line of the text names the agent: `from=<agent name>`.

### Built-in project workflows

A new GitHub Project has built-in workflows, and all six are on by default: "Item
added to project", "Item closed", "Pull request linked to issue", "Pull request merged",
"Auto-close issue" and "Auto-add sub-issues to project". Several of them write `Status`.
crewboard treats `Status` as its own state, so such a workflow is a second writer behind
crewboard. Two cases:

- A PR body named a Blocked ticket with a closing keyword by accident. "Pull request linked
  to issue" then moved the ticket to In Progress. No agent worked on it.
- "Item closed" moved a ticket that waited for the owner to Done when a PR closed its issue. The coordinator
  had to reopen the issue and set the status again.

GitHub has no mutation that turns a workflow on or off or creates one. The only write is
`deleteProjectV2Workflow`, and it works on the built-ins: a deleted workflow stops acting (checked
live on 2026-10-25).

`init` deletes these four workflows, by name:

| Workflow | What it does behind crewboard |
|---|---|
| Item added to project | sets the Status of every new item |
| Item closed | sets the Status when an issue or PR closes |
| Pull request linked to issue | sets the Status of an issue when a PR body links it |
| Pull request merged | sets the Status when a PR merges |

Two built-ins stay on purpose:

- "Auto-add sub-issues to project" adds the sub-issues of a board item as new items. They have
  no `Kind`, no `Gate` and no signed info comment, so `status` does not list them. They are
  inert for crewboard, and with the four above gone nothing writes their `Status`.
- "Auto-close issue" writes no field and no item: it closes the issue when `Status` becomes
  Done. crewboard writes Done only for finished, verified work, so a closed issue is the
  expected end state.

Other workflows exist only when the owner added them by hand: "Item reopened", "Code changes
requested", "Code review approved", "Auto-archive items" and "Auto-add to project". `doctor`
warns about them; crewboard does not delete them.

The delete step:

1. The command reads the workflows
   (`projectV2(number:){workflows(first:30){nodes{id number name enabled}}}` under `organization`
   or `user`, as the owner type of the board says). A workflow is one of the four when its name
   equals one of them, ignoring case and spacing. A workflow that is not there is already gone.
2. It calls `deleteProjectV2Workflow(input:{workflowId})` for each one, enabled or not, and
   prints `deleted project workflow: <name>` after each, then one summary line. The delete is
   one way: the API cannot create the workflow again, so the owner who wants one back adds it
   in the web UI.
3. `init` runs the step after the views, on a new and on an adopted project. If the read or a
   delete fails (a token without the permission), `init` fails at this step. The error names
   the manual fallback: open `<board URL>/workflows`, delete or switch off the four, run the
   command again. Earlier steps stay done, as for every step of `init`; a second run continues.
The guard: `agent selftest` and `doctor --board` read the workflows and print one warning per enabled
workflow that writes `Status`. For one of the four, the warning names `crewboard init`
as the fix. For any other writer, it names the page where the owner turns it off:
`<board URL>/workflows`. A board with none on gives no warning. If the query fails (a token
without the scope, an old token), they print one warning that crewboard could not check the
workflows. The command does not fail, and its exit does not change.

The watch marks a status change that GitHub automation made
([Status changes by GitHub automation](events.md#status-changes-by-github-automation)).

## Registry and lifecycle

### Registry issue

One issue per initiative, pinned, label `crew:registry`, title
"Crew registry - <initiative>". Join requests, approvals, role changes and departures
are comments there. This gives a complete, ordered audit trail of who was allowed to do
what, and when.

A join request is read from any GitHub account; the account that wrote it is the account of
the agent ([Accounts](identity.md#accounts)). Every other comment of the registry counts only
from the owner (commands) or from the account of the agent (its signed markers). `init` locks
the conversation of the registry issue, so only collaborators of the repo can comment
(`registry lock: locked`). If the lock fails, `init` says so and the owner locks it by hand
(Lock conversation, on the issue page). On a public repo without the lock, any GitHub user can
post a join, and `approve` shows the account of each request.

### Lifecycle

```
            join                  owner: /crew approve
 (none) ─────────► requested ───────────────────────────► approved
                                                        start │   │ stop
                                                              ▼   ▼
                                              ┌────────► active  left
                                              │             │
                                         resume│       pause │
                                              │             ▼
                                              └──────── paused
                                active / paused ──stop──► draining ──► left
                     any running state ──(heartbeat older than 3 intervals)──► late ──(older than 5)──► dead
 owner: /crew reject, in any state: rejected in the registry; the item State stays
```

| Transition | Who | Effect |
|---|---|---|
| `join` | agent | Creates the agent item (`State = requested`) with Key, Host, Runtime and the detected facts ([Facts](profiles.md#facts)). Posts a signed join request in the registry issue. A re-join into a config dir that already holds the agent config replaces only the keys that `join` owns (`name`, `runtime`, `host`, `item_id`, `board_sha`, `board`, `tools`) and keeps every other key (`headless`, `[loop]`, `insecure_one_account` of the agent config, and any other). The write is atomic (temp file, rename) and keeps `headless` as the first line. A join for another agent name than the one in the file is refused with rule `one-agent-per-config`; the name is read from the file, even if the file does not load. |
| `/crew approve <name> [role=dev\|coordinator] [key=<fingerprint>] [item=<id>] [login=<account>]` | owner, as an owner comment in the registry issue | crewboard sets `State = approved`, sets Role, adds the name as an option of the `Agent` field. Key and account are frozen. `crewboard approve` writes `key=`, `item=` and `login=`; an approval without `login=` binds the account of the join. On a board with `crew_login` a join of the owner's account is not approved (`approve-owner-account`). |
| `/crew reject <name>` | owner, as an owner comment in the registry issue | The registry marks the agent rejected. Its trust ends: crewboard no longer delivers its messages (security event `unknown-sender`, see [Signature](messages.md#signature)), `approve` refuses it (`agent-rejected`), and a later join or approval of that name is ignored. crewboard does not change the item `State`. `status` shows the agent as `REJECTED` (text) and `"rejected": true` (`--json`). |
| `start` | agent | `approved` or `paused` -> `active`. Starts heartbeats. |
| take a ticket | agent (dev) | Writes no State. The agent must be `active`: `agent-paused` for a paused agent, `agent-not-active` for every other State (the message names `crewboard start`). Doing = ticket. Refused with `take-agent-busy` while another ticket of the agent is In Progress, whatever the State is ([Status flow and writers](tickets.md#status-flow-and-writers)). |
| `done` / `block` / `ask` | agent (dev) | Write no State. Doing is set to `idle` only when the agent has no other In Progress ticket. `Doing` is a display text. |
| `pause` | agent or owner (`/crew pause <name>`) | Finishes the current step, takes no new tickets. The coordinator assigns nothing to it. |
| `resume` | agent or owner | `paused` -> `active`. |
| `stop` | agent or owner | `active` or `paused`: `draining`; open tickets go back to `Backlog` (no agent, so `assign` again) with a comment; then `left`. From `approved` (the agent never started): `left` at once. A `requested` agent cannot stop: the owner rejects it (`/crew reject`). The stop is in the registry log: the owner's `/crew stop <name>`, or the agent's own signed `info` `<name> is stopping (draining). ...` that it posts when it starts to drain. The coordinator returns the tickets of an agent that the log stops, not of a `State = draining` alone. At `left` the agent first posts a signed `info` to the coordinator in the registry issue with the lines `<name> left` and `lifecycle: left`. That marker ends the trust span of the agent ([Trust at signing time](identity.md#trust-at-signing-time)). `draining` and `/crew stop` do not end it: a draining agent speaks until its marker. An agent leaves only when the log stops it; a `State = draining` alone does not make it leave. |
| `stop --agent <name> --force` | coordinator or owner | For an agent that cannot drain (a lost host or key): no draining, no marker. The coordinator returns the open tickets of the agent as a derived `reassign` does (to `Backlog`, one signed notice each), writes `State = left` on the agent item, and posts one signed `info` to the owner. The coordinator is not the owner, so it does not stop the agent in the log: the agent's span stays open and it may still speak (a dead agent does not). The owner (an account with no signing identity) posts the owner command `/crew stop <name>` in the registry issue and writes `State = left` as the mirror; the coordinator's next cycle returns the tickets, because the log stops the agent. The owner's forced stop also works on an agent that is `left` on the board without a stop in the log (a hand edit, the coordinator's forced stop): it records the stop. A running agent whose State is `left` and that the log stops ends its loop. Refused: a dev agent as the caller (`stop-force-needs-coordinator`), the caller itself (`stop-force-self`), the coordinator as the target (`stop-force-not-dev`), an agent that the log ended, or that is `left` with nothing more to do (`stop-force-already-left`), and a `requested` one (`stop-force-not-running`). The key stays valid, because only the agent's marker and the owner's reject end a span: the owner revokes it with `/crew reject <name>`. |
| `late` / `dead` | derived | The coordinator sends nothing to a late agent; `assign` refuses it. A dead agent's tickets go back to `Backlog`, and the coordinator reports it, naming the open PR that the agent left ([Work that an earlier agent left](tickets.md#work-that-an-earlier-agent-left)). A paused agent that holds Ready tickets is a warning: `status` shows a `!` line, and the coordinator cycle gives one `agent-paused-ready` event per change of the set (nothing starts those tickets until the agent resumes; `late: take` is off by default). Who else hears is in the table of late work ([Stale work](events.md#stale-work), item 8): the owner hears of a dead agent and of a late coordinator. |

**Busy.** An agent is busy when a ticket that is In Progress is assigned to it. The board says
so, not the agent State: `status` shows `busy <key>` next to the State and `status --json` gives
`in_progress` in the agent object. An agent State `working` is never written. The legacy
value on an old board reads as `active`.

Level: approval is *loud*. crewboard refuses any message from an agent that is not
`approved`, and refuses a role that no owner comment granted.

### Coordinator role

- Exactly one coordinator per initiative. crewboard refuses a second one (*loud*).
- Any agent can be the coordinator, but only by an owner comment
  `/crew approve <name> role=coordinator` or `/crew role <name> coordinator`.
- Handover: the owner sets the old coordinator to dev (`/crew role <old> dev`) and then the
  new one to coordinator (`/crew role <new> coordinator`), or the old one leaves with `stop`.
  The two owner comments in the registry issue are the handover record. What the old
  coordinator signed before keeps the coordinator role
  ([Trust at signing time](identity.md#trust-at-signing-time)): its tickets, its answers and
  its split notes still count. A coordinator that has left no longer blocks a new one.
- The coordinator does not write code. It does not take tickets, and crewboard refuses
  `take` for the coordinator (*loud*). It does not push to code branches (*stated*).

### Item ids are the source of truth

The item list of a board is only eventually consistent. Items added in bulk can be
missing from `items()` for minutes, while each item can always be read by its
id ([GitHub API notes](../github-api-notes.md)). A missing item must never look like "nothing changed" or "not
registered". Therefore:

1. The signed join comment of an agent contains the id of its agent item.
2. When the coordinator adds a ticket to the board, it posts a signed `kind=info`
   comment in the ticket with the item id.
3. crewboard reads known items only by id (`nodes(ids:)`). It uses `items()` only to
   discover items that were added by hand (for example by the owner in the web UI).
4. `status` lists items that are known by id but missing from `items()`, so the lag is
   visible and not silent.

## Authority

Each initiative has an `AUTHORITY.md` in its main repo (path in the board config). It
has machine-readable front matter and free text for the owner. The front matter has two
blocks. Enforced keys are code: crewboard reads them and refuses what they forbid. Advice
keys are not read by crewboard; the models read them as the owner's rules.

```yaml
---
# ENFORCED: crewboard reads these keys and refuses what they forbid
initiative: payments-v2
owner: owner-login
coordinator_may_create_tickets: true
coordinator_may_split_tickets: true
coordinator_may_merge: false        # true only if the owner allows it
dev_may_open_prs: true
dev_branch_prefix: crew/
protected: [main]
# protected_paths: [tests/**, scripts/ci-*.sh]  # optional: more paths that only the owner merges
# ci_ignore: [deploy-preview]       # optional: CI jobs that crewboard does not count
# ci_mode: local                    # optional: ci (default) or local; see ci.md, Local run
# former_repos:                     # optional: after a repo transfer or rename, see messages.md
#   - { name: old-owner/old-repo, until: 2026-11-01T12:00:00Z }
# ADVICE: crewboard does not read these keys; the models read them as the owner's rules
needs_owner: [release, credentials, delete, new-scope, new-repo]
---
```

crewboard reads it at each `start` and refuses commands that it forbids (*loud*). The
free text describes the scope in words for the models (*stated*). `start` prints the list of
the enforced keys.

**Enforced keys** (`authority::ENFORCED_KEYS`):

| Key | The command that it limits |
|---|---|
| `initiative`, `owner` | `start`: the owner must be the owner of the board config (`authority-owner-mismatch`). `owner:` is deprecated: `owner_login` in the board file is the one source (see below) |
| `coordinator_may_create_tickets` | `assign` of an issue that is not on the board (`authority-no-ticket-creation`) |
| `coordinator_may_split_tickets` | `ticket split` (`authority-split-not-allowed`) |
| `coordinator_may_merge` | `merge`, `tick` (`authority-merge-not-allowed`) |
| `dev_may_open_prs` | `done` (`authority-no-prs`) |
| `dev_branch_prefix` | `done`: the branch of the PR |
| `protected` | `done`: a PR from a protected branch is no evidence (`evidence-branch-protected`) |
| `protected_paths` | `verify` PASS, `close --verified`, `merge`, `tick`: more paths that only the owner merges, added to the built-in seven (`protected-path-owner-merge`); see below |
| `ci_ignore`, `ci_mode` | the CI judgement of `done`, `verify` and `merge` ([CI](ci.md)) |
| `former_repos` | which messages verify after a repo transfer or rename: earlier names with an exclusive cutoff ([Repository moves](messages.md#repository-moves)) |

**Advice keys** (`authority::ADVICE_KEYS`): `needs_owner`. Its words (`release`, `credentials`,
`delete`, `new-scope`, `new-repo`) tell the models what to ask the owner first. crewboard does
not read them. The one exception: `merge` in `needs_owner` still refuses
`merge` (rule `authority-needs-owner`, deprecated; `start` warns). Set
`coordinator_may_merge: false` instead.

**Always owner-only, in code.** A PR that changes a protected path is merged by the owner only,
whatever the keys say. Seven paths are built in: `AUTHORITY.md` (the path in the board config),
`.crewboard.toml`, `.github/workflows/**`, `.github/actions/**` and the three CODEOWNERS files
that GitHub reads: `.github/CODEOWNERS`, `CODEOWNERS` and `docs/CODEOWNERS`. The
built-in list is a constant: no key removes a path from it, because a key in a file that a PR
can change would not protect that file. A verify PASS, `close --verified` and `merge`
(so also `tick`) check the current file list of the PR and refuse with
`protected-path-owner-merge`; a PR with more than 1000 files is refused too (crewboard cannot
prove that none is protected). The owner reviews and merges such a PR by hand, then the
coordinator runs `crewboard close <n>`. Also owner-only, always: approving agents,
setting the coordinator, rulings on a question to the owner.

**More protected paths: `protected_paths`.** The key adds path globs to the built-in seven, for
example the test configuration or the CI scripts that a `Gate: ci` ticket depends on.
It can only add: an entry never removes a built-in path, and an empty list changes nothing.
A glob is relative to the repo root: `*` matches inside a name, `**` across directories, `?`
one character, and a trailing `/` means the directory and everything below it. The match
ignores ASCII letter case and a leading `./`, and a rename out of a matching path counts, as
for the built-in paths. The file is read from the default branch, and a PR that changes it is
protected itself, so the key cannot be loosened by a PR. The refusal is the same rule
(`protected-path-owner-merge`); its text names the paths of the PR and, when the key is set,
the key and its entries. The `done` of such a PR carries `protected-paths: <paths>`, so the
headless `tick` leaves it for the owner. A bad entry (empty, with a leading `/`, a `\`, or a
`..` part) is refused when the file is read: `authority-bad-front-matter` names the key and the
entry (`protected_paths: '/etc/x' is not a relative path glob`). `start` prints the entries.

**Branch protection is set by `init` and only checked afterwards.** Every crew account has Write
on the repo, and AUTHORITY.md and `.crewboard.toml` are read from the default branch. Without a
rule that requires a pull request, an agent can push a change of either file with no review.

`init` creates the rule for the main repo (the owner token is admin), after the pull request
step. It first looks for a rule: `GET repos/{r}/branches/{b}` with `protected` true,
`GET repos/{r}/rules/branches/{b}` with a rule, or `GET repos/{r}/rulesets` with the name
`crewboard: default branch`. If one exists, it leaves it and prints a line. If none exists, it
sends `POST repos/{r}/rulesets`: target `branch`, include `~DEFAULT_BRANCH`, enforcement
`active`, the rules `deletion`, `non_fast_forward` and `pull_request` with 0 required approvals
(an author cannot approve their own pull request), no status check (crewboard does not know the
job names), and one bypass actor, the repository role Admin with mode `pull_request`. A refusal
(no admin, no rulesets on the plan) is one line `branch rule: not created ... (HTTP s: message)
Fix: ...`; `init` goes on and the exit code stays 0. `--no-branch-rule` skips the step. A
ruleset replaces classic branch protection here because one request sets all rules and
`~DEFAULT_BRANCH` follows a rename.

`start` and `doctor --board` read the default branch of the main repo (up to three requests, once
per run: `GET repos/{r}` for its name, `GET repos/{r}/branches/{b}` for `protected`, which is
true for a classic rule and for a ruleset alike, and, when `protected` is false,
`GET repos/{r}/rules/branches/{b}`, because a ruleset can apply while `protected` says false) and print one line. Nothing protecting the branch is one
warning line with the fix: a rule that requires a pull request, with owner or CODEOWNERS review
for the protected paths. A protected branch gives an information line (the token cannot read
whether the rule requires a pull request), and a branch that cannot be read gives `unknown`.
The check never warns on a guess, never changes the exit code and never refuses a command.

**A headless merge reviews no diff.** `doctor --setup` prints one warning line when the agent
config has `headless = true` and `protected_paths` in AUTHORITY.md is empty: "Gate: ci tickets
merge without a diff review; list the files that define your CI and tests in protected_paths".
The config key is the only signal of a headless coordinator that a host has; another host's
config is not visible. The warning never changes the exit code.

### Contradicting keys in AUTHORITY.md

Some keys can contradict each other. A file can have `coordinator_may_merge: true` and `merge` in
`needs_owner`; `start` printed `coordinator merges: no` and did not say why. `merge` in `needs_owner`
is deprecated and still wins: the file is
obeyed as written, so an old file does not loosen. crewboard still obeys the file, and warns:

- every command that re-reads AUTHORITY.md prints the warning to stderr, once per run;
- `start` and `agent selftest` add it to the selftest line;
- `start` prints a warning for every `merge` entry of `needs_owner`, also without the flag.

The warning names both keys, the winner and the effective rule (needs_owner wins; the coordinator
may NOT merge; nothing is loosened). Today there is one such pair: `coordinator_may_merge`
with `merge` in `needs_owner`. Behaviour is unchanged in this release. A later release makes
`start` refuse a file with this pair (rule `authority-contradicting-keys`), because removing the
`needs_owner` refusal would loosen the file silently.

### `owner:` and `owner_login`

Two files name the owner: `owner:` in AUTHORITY.md and `owner_login` in `.crewboard.toml`. Every
trust decision (owner comments, an agent's account must differ from the owner's) reads `owner_login`.
`owner:` can only agree with it, because `start` refuses a different login
(`authority-owner-mismatch`, unchanged). Therefore `owner_login` is the one source:

- `start` and `doctor --setup` print a note when the file still has `owner:` (it is not needed);
- a file without `owner:` is valid and gets no warning;
- `close` after an owner merge reads `owner_login` only (it read `owner:` first before; the two
  were equal for any crew that passed `start`, so no outcome changes);
- a later release ignores `owner:` without a note. The owner removes the line in the next
  AUTHORITY.md pull request (crewboard cannot edit a protected path).
