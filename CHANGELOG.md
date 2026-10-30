# Changelog

This file lists the changes of crewboard, one section for each release.

## Unreleased

Added:

- `crewboard mine <n>`: the signed thread of your own ticket since your current assignment,
  newest first. It shows the answers, infos and verify notes of the coordinator, the owner
  comments that count and your own reports. Comments that do not count are not shown; one line
  counts them. A long message is cut with a note. Only the newest 15 are shown; a note counts
  the older ones.
  `--json` gives the same. A ticket of another agent is refused (`mine-not-your-ticket`). It
  writes nothing. The dev skill and the wake prompt tell a dev agent to read it before it acts
  on a ticket and before it asks the coordinator, so a wake does not ask again what was
  answered.

Changed:

- **Contract change:** a usage error exits 64 for every command: an unknown option, a missing or
  bad argument (for example an issue number that is not `12` or `owner/repo#12`), a bad
  combination of options. It was 4 for every command except `lock`, `repeat` and `wake`. Exit 4
  keeps its meaning: a configuration or key problem that stops the agent until a person fixes
  it. The skill tells the agent: on 64, fix the command and run it again. A script that read 4
  as "bad command line" reads 64 now. `--help` and `--version` stay 0. The one line of
  `monitor` for a bad option says `exit 64`.
- A bare `crewboard` prints the overview to standard output and exits 0, like `--help`. It
  exited 4 before.
- `crewboard doctor` starts with a host block: the `gh` version (missing or older than 2.17 is
  the one failure of the report), the `git` version and the installed skill with its version
  and path. The lines of the headless scope follow as before.
- The "no board config" error names the visible ways first: a checkout with `.crewboard.toml`,
  `--initiative <name>`, `crewboard init` on a fresh host; the hidden `--initiative-file` is the
  last option.
- The owner line of the overview puts the steps in the order of the guide: `agent token`,
  `init`, then for each agent `agent add` on its host and `approve`.

Fixed:

- `doctor --setup`: the FAIL line of a missing agent config names one fix, `agent add` (the
  join is what `agent add` runs on the host); it also said "run `crewboard join` first".
- `init -h` says "crew token" for `--env-file`, as every other help text does.
- Docs and skill: Done needs a verified merge or the owner's merge. A PR that the crew
  account merged without a verify gives no Done; the coordinator does not merge its own
  ticket work, and such a ticket can only be dropped.

Other:

- Internal: no behaviour change. Comments say what the code does, three named reasons for a
  dropped result, test-only code in the test modules, one set of test builders, and the module
  names `repeat` and `wake::precheck` instead of two more meanings of "gate".
- Internal: no behaviour change. Board fields are a type, and every field write asks the
  field-writer table first (the table now names the forced stop as a writer of `Doing`, which
  `stop --force` already wrote without a check); the refusal ids are one catalog in the code; a
  ticket is one type (`TicketRef`) in the commands, the state and the late checks.
- Docs: [Refusal ids](docs/reference/refusals.md) lists every rule id that a refused command can
  print, with its meaning. The page is generated from the catalog.

## v0.7.1 (2026-10-30)

Upgrade: if you ran `crewboard init` again with v0.7.0, check your local
`<initiative>.board.toml` and the `.crewboard.toml` of the init PR: that rerun dropped every key
that `init` does not set, for example `heartbeat_interval` and `required_tools`. Add them back.
The copies in the agent dirs were not changed by that rerun.

Fixed:

- An `init` rerun on an existing board kept only its own keys: it rewrote the local
  `<initiative>.board.toml` and the `.crewboard.toml` of the init PR, and dropped
  `heartbeat_interval`, `required_tools` and every other key that `init` does not set. Now `init`
  sets only its own keys and keeps every other key and the comments of both files. A local file
  or a `.crewboard.toml` on the default branch that is not valid TOML stops `init` before the
  first write.
- An `init` rerun without an identity flag switched a board with `crew_login` or
  `insecure_one_account` to own accounts. Now it keeps the identity mode of the board config and
  prints `identity: ... kept`. A flag of another mode than the board config names is refused
  before any write (`init-identity-mode-change`); the refusal says how to switch on purpose.
- A comment flood (more than 5000 comments) on a ticket made the coordinator forget its notes on
  that ticket for good, so "flood, delete the close note, delete the flood, set the board back"
  was never seen. Now the notes stay. The ticket is read again when its board row changes and 6
  hours after the last read, not in every cycle. A Status that cannot be judged (a false Done
  too) stays under the Status check. The owner gets one signed note for each such ticket
  (`event=<repo>#<n>/notes-not-checked:too-large`, and `...:no-board-item` for a ticket whose
  board item is gone). The coordinator skill now names the ticket to the owner.
- `notes-not-checked` was lost when `watch` repeated a failed cycle. It now changes the memory
  only after it was shown and reported, as `deleted-message`. A `poll --quiet` no longer makes
  the next cycle show it twice, and the Status check no longer raises it at each status flip.
- A ticket that the owner set to Done by hand while its PR was open had no way out:
  `ticket new --continue-pr` named `close --why`, which refused the board Done. Both refusals
  now name the way that works: the owner's `crewboard move <n> Backlog`, then `close <n> --why`.
- The Status check reads at most 5 moves out of Done or Dropped per cycle, the oldest first (the
  others wait in the state key `setback_queue`). Its owner note comes once per ticket and state
  of the signed log (`event=<repo>#<n>/status@c-<id>`), not once per board status, so a board
  that flips between statuses gives one note.
- A brand-new ticket was taken for a deleted board item when a long `watch` did not know it yet;
  its notes were then forgotten. A ticket without a row is now judged only when its newest note is
  older than 10 minutes.
- A recheck mark that a long `watch` moved and saved could undo a later clear by a ticket command.
  A save now counts the written file as read.
- The `event=` keys of security events are in lower case for the repo, and a key of another
  spelling in the registry issue matches. Before, `Org/Repo` and `org/repo` gave two owner notes.
- `tick` forgot its reported deleted notes before its output was printed. Now it forgets them
  after the print.
- A note kind of a later build was saved back as `other`. It now keeps its name.

## v0.7.0 (2026-10-30)

Upgrade: run `crewboard init` once on an existing board; it adds the Status option `Dropped`
and keeps every other option. A coordinator merges a verified PR with `merge`: `close --verified`
now needs a PR that is merged already.

Added:

- The ticket Status `Dropped`, final like Done. Done now always means a merged PR: merged at a
  verified head, or merged by the owner; Dropped means that the coordinator ended the ticket by judgment, without a merged PR.
  Only the owner moves a ticket out of Dropped (`crewboard move`); any other act on it is
  refused with `status-dropped-final`. `status`, `audit`, `mine`, the queue, the coordinator's
  `poll` and `watch` and the views treat Dropped as ended, like Done. A follow-up ticket can
  continue the open PR of a Dropped ticket (`ticket new --continue-pr`).
- `init` adds the Status option `Dropped` after `Done`, and the views By Agent and Needs Owner
  filter `-Status:Done,Dropped`. **An existing board needs one `crewboard init` rerun** for the
  option; the rerun keeps every other option id. A view that exists keeps its old filter; the
  rerun prints how to add `Dropped` to it in the web UI.
- The security event `deleted-message`. The coordinator remembers the comment ids of its own
  signed notes on a ticket (close, verify, assignment, reassign, creation) in its state file.
  When `poll`, `watch` or `tick` reads the complete comment list of such a ticket and a note is
  gone, it raises the event once and tells the owner once. A fresh host or a lost state file
  remembers nothing, and `audit` cannot see a deletion. No wire change: the format stays `v=1`.
- `doctor --setup` warns when the Status field has no option Dropped.

Changed:

- `close --why "<reason>"` drops the ticket: Status Dropped, never Done. Its signed note starts
  with `dropped: <reason>` (the wire format stays `v=1`). The note `closed: <reason>` of an older
  build still reads as Done, so the history of a board stays valid. On a board without the
  option, `close --why` is refused with `dropped-option-missing` before any write. The issue
  stays open.
- `close --verified` needs the PR merged at the head of the passing verify. An open PR is
  refused with `close-verified-not-merged` before any write; use `merge`. A moved head and a PR
  closed without a merge stay `close-verified-head-moved`. A passed `verify` now names
  `crewboard merge <n>` as the next command.
- `crewboard move <n> Dropped` is refused with `move-dropped-needs-close`: the coordinator drops a
  ticket with a signed reason.
- The owner-route question to the owner says that the coordinator closes the ticket after the
  owner's merge (it named a second `done` of the dev agent, which the route does not use).
- Docs: `docs/design/spec/tickets.md` rewritten as a short spec without history. The status
  table `src/rules/status/table.txt` is the matrix; a test keeps its codes and the spec in step.

Fixed:

- `deleted-message`: a gone note is forgotten only after its event was shown and its owner
  report was posted (or was there already). A failed post in `watch`, `poll` or `tick` lost the
  event for good; now the next cycle raises it again.
- The note check reads at most 5 marked tickets per cycle, the oldest mark first, in rotation.
  A board Status that leaves Done or Dropped without a signed message is reported once as a
  Status mismatch (`status-mismatch`, one note to the owner), not as a security event.
- The note check reads a ticket whose board item was deleted once by its issue number, and
  stops at a ticket with more than 5000 comments; both give one `notes-not-checked` event. Before,
  the first was never read and the second was read again in every cycle.
- `ticket new --continue-pr` judges the old ticket by its signed log, not by the board Status. A
  board Dropped or Done that a crew token wrote no longer hands over the open PR of a running
  ticket.
- The state file merges the recheck marks of the note check on save, and keys the note memory
  by the repo in lower case (`Org/Repo#7` and `org/repo#7` are one ticket).

## v0.6.1 (2026-10-29)

Added:

- `crewboard move <issue> <status> [--why <text>]`: the owner moves a ticket by hand, in the
  signed log. It posts the owner command `/crew move <status>` on the ticket and then writes the
  board `Status`. The log reads the move as the owner's, also after a close: a Done ticket moved
  back to Ready counts as open again. `status` and `audit` show no mismatch after it; a drag on
  the board without the command is still reported. Ready, In Progress, Review and Blocked need a
  signed assignment (`move-needs-assignee`); Done is refused (`move-done-needs-close`), and a name
  that is not a board status too (`move-bad-status`). It is an owner command (the owner's token,
  `owner-command-not-owner`). The wire format stays `v=1`.
- One GitHub account per agent. A contributor joins with an own account and token; the shared
  crew account (`crew_login`) is optional and keeps working, and both mix on one board. A board
  has three identity modes: `insecure_one_account` (the one-person trial), `crew_login` (a
  shared crew account), or neither key (each agent its own account). `init` without
  `--crew-login` and without `--insecure-one-account` no longer refuses; it makes an
  own-accounts board. The registry reads a join request from any account. The account that
  wrote the join is the account of the agent.
- `approve` shows the account of each join and binds it with the key: the approval comment
  carries `login=<account>`. A hand-written approval without `login=` binds the account of the
  join. On both crew modes, a join of the owner's account is refused
  (`approve-owner-account`). `status` and `doctor --setup` show the account of each agent.
- A signed message counts only from the account that the approval bound to its sender. The same
  text posted by another account, also by another agent's, is the security event
  `foreign-author`.
- `start` checks the token and plain `gh` against the agent's own account
  (`agent-token-not-crew`, `agent-gh-not-crew`). Agent commands no longer refuse a token of
  another account than `crew_login`; on both crew modes they refuse the owner's token, also
  with `--insecure-one-account`. The rule `one-account-needs-flag` is gone: only the board key
  makes a trial.
- `init` locks the conversation of the registry issue, so only collaborators can comment. A
  board made before this release: lock the issue by hand (Lock conversation).

Changed:

- `--runtime` has one meaning: the runtime (`claude`, `codex` or `hermes`), on `join` as on
  `agent add`. `join` takes the model with the new option `--model`, and builds the runtime
  label from both, with the same function as `agent add`. The label in the join request is the
  same as before (`claude-code/<model>`, `codex/<model>`, `hermes/<model>`, or the first word
  alone without a model), so the wire format and existing registries do not change.
- `join --runtime <runtime>/<model>` (the old form) is refused as a usage error (exit 4). The
  message shows the new form, for example `--runtime claude --model sonnet`. `doctor --setup`
  prints the join line in the new form.
- Owner commands use your own `gh` login: `init`, `approve`, `audit`, and `pause`, `resume`,
  `stop` and `agent profile set` with `--agent`. They ignore `GH_TOKEN` and `GITHUB_TOKEN`, and
  print one note that names a token variable that was set. `--env-file` still wins. The prefix
  `env -u GH_TOKEN -u GITHUB_TOKEN` is gone from the docs. A token that is not the board
  owner's is refused with `owner-command-not-owner` (before: `not-owner`); the message names
  the login. In the config dir of an agent, an owner command keeps the token of the agent and
  never runs `gh`. Agent commands keep their token order.

Fixed:

- The board `State` of an agent no longer mutes it or returns its tickets. The registry log
  decides: a `State` that contradicts it is reported (`status`, `audit`, one note to the owner).
- The owner's `stop --force` posts `/crew stop <name>`, so the stop is in the log. The
  coordinator returns the tickets of an agent that the log stops.

Docs:

- Shorter README, concepts, quick try, owner, agents and security guides. "Headless" now means
  only a coordinator without a model; Claude Code without a UI is "unattended"
  (`docs/guides/runtimes/unattended-claude.md`).

## v0.6.0 (2026-10-29)

The first public release.

crewboard lets several AI coding agents work as a team on one GitHub repo. One agent
coordinates and the others write code. They talk only through signed issue comments and a GitHub
Project board. There is no server and no database: one binary and one agent skill.

Highlights:

- Proof before Done. Through the CLI, a ticket reaches Done only when the coordinator verified a
  pull request, the owner merged it, or the coordinator closed it with a signed written reason.
  `close --verified` closes only the head that `verify` saw. A Done written on the board by hand
  (a crew-token holder can do that) is detected, not prevented.
- Trust from the signed log. The signed log decides who an agent is and what a message says. A
  board field that contradicts the log is reported by `status` and `audit`. The board `State` of
  an agent still decides whether it may act now (see [Security](docs/guides/security.md)). A
  signed message that is posted again inside the 15-minute window is refused while the original
  is still on the issue. An owner approval without `key=`
  binds no key that the owner did not see.
- Defaults that limit damage. `init` sets a branch rule on the default branch: pull request
  required, no force push, no deletion. Protected paths are merged only by the owner.
- Fewer lost writes. Concurrent writers are guarded. The open pull request of a replaced agent is
  kept, and `take <n> --continue-pr <url>` continues on it. Common stalls are reported by default.
  `crewboard stop --agent <name> --force` returns the tickets of an agent that cannot drain.
- A smaller CLI. The overview shows 26 commands in four groups. The rest is under
  `crewboard help extensions` and `crewboard help scheduler`.
- One queue position. `Order` is the queue position of an agent, lowest first, and the ticket
  views sort by it. There is no `Priority`. `assign` also moves a queued ticket to another agent
  or position, with a signed assignment.
- `Gate: ci` in a ticket body is the typed form of a ticket that needs CI only.
- Repository moves. `former_repos` in `AUTHORITY.md` keeps the signatures of earlier messages
  valid after a rename or transfer. `crewboard doctor --repo-move` is a read-only dry run. See
  [Move the repository](docs/guides/owner.md#move-the-repository).
- No assumed stack. `detect_tools` in `.crewboard.toml` names the tools of your project, and the
  examples cover several stacks.
- Runtimes that run the model's commands in another process get the agent dir and the `gh` shim
  path in the wake prompt.
- A newcomer path. The Quick try has 9 steps, the concepts page is a glossary plus the life of a
  ticket, and the docs show a real example session.

### The owner route

- The owner route ties the proof to the ticket (patch name and PR file). It reads the hunks of
  the patch on the base branch: each hunk must be in the workflow file as one block, as
  `git apply -R --check` sees it. The check fails closed. `close` applies the same checks.
- `close` ends an owner-route ticket after the owner merged its pull request. No closing keyword
  is needed.
- The safety net does not reopen a ticket in a loop: an answered finding is settled.
- `stale-work` reports `review-not-verified` only when the scan could read GitHub, and never for
  an owner-route ticket. `tick` sends no verify note for such a ticket.

### Security

- Branch names that an agent controls are checked (rule `evidence-branch-name`), quoted in the
  shell commands that crewboard prints, and percent-encoded in GitHub URL paths.
- `done` refuses a pull request with more than 1000 files: it cannot prove that no protected path
  is in it.
- Seven built-in protected paths: the authority file, `.crewboard.toml`, three `CODEOWNERS`
  locations, `.github/workflows/` and `.github/actions/`. `protected_paths` in `AUTHORITY.md` adds
  paths and cannot remove one.
- CI actions are pinned by commit SHA. `cargo-deny` checks advisories, licenses and crate sources
  in CI. Dependabot updates the cargo crates and the actions every week.
- Release tarballs carry a build provenance attestation and the third-party license list.

Boards from earlier private builds need a new board. See
[Boards of earlier builds](docs/guides/upgrade.md#boards-of-earlier-builds). crewboard reads an
old board and its signed messages, but it has no old command names and no shell shims: the binary
does all of it. Every wire format stays `v=1`.
