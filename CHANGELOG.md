# Changelog

This file lists the changes of crewboard, one section for each release.

## Unreleased

Added:

- Guarded paths per ticket: a ticket can name paths that its change must not touch, so an
  agent that must make a test pass cannot edit the test, a fixture, a golden file or the check
  script. The body has one line `Guard: <glob>` for each path, and `ticket new --guard <glob>`
  (repeatable) writes them. The globs are the ones of `protected_paths`. The first assignment
  signs them as `guard:` lines, and `reassign --to`, the move of a queued ticket and an `assign`
  after a `reassign` copy the signed lines; `ticket split` hands them to the new ticket. `done`
  refuses a PR that adds, changes, deletes or renames a guarded file with the new id
  `guarded-path-changed`. A verify pass, `close --verified`, `merge` and the headless `tick`
  refuse it too; `verify --fail` stays possible. No flag overrides a guard: the owner changes
  the file in a separate change, or the work goes on in a new ticket. With `--must-fail-at`, a
  guarded file must be the same on the red commit and on the tested one. A guard that is not
  valid (more than 20 globs, a glob of more than 200 bytes, an empty one, an absolute path, a
  `..` part, a glob that can cover no path such as `./` or `a//b`) is refused with the new id
  `ticket-guard-invalid`. A ticket whose assignment has no `guard:` line works as it did.
  `assign` names the guard in its `signed from the body:` line, as `guard none` for a ticket
  without one. Update every host that runs a coordinator before the first `--guard`: a version
  without guarded paths writes assignments without `guard:` lines (`reassign --to`, the move
  of a queued ticket, an `assign` after a `reassign`), and such an assignment ends the guard
  of the ticket without a warning. Spec: `docs/design/spec/tickets.md#guarded-paths`.

Changed:

- Protected paths and guarded paths: a base branch cannot hide a file. The file list of a PR
  is the diff against its base branch, and the author of the PR chooses that branch. For a PR
  whose base is not the default branch, `done`, a verify pass, `close --verified` and `merge`
  (so also `tick`) now also read the files that differ between the default branch and the head
  of the PR, and judge both lists. A stacked PR without such a file passes as it did. A
  comparison that cannot be read, or one with 300 files or more, is refused with
  `protected-path-owner-merge`. A PR onto the default branch makes no such request.
- A verify pass and `merge` read the head of an open PR again after the file list. A head that
  moved in that time is refused with `evidence-head-moved`, and nothing is written.

## v0.13.0 (2026-11-28)

Upgrade: run `crewboard update` on each host. Nothing changes on the board. Two behaviours
change (see "Changed"). A scheduled agent that runs `repeat` needs its wake installed with
`--host-lock <dir>` and `repeat --lock <dir>`. An `assign` after a `reassign` no longer signs an
edited Gate of the issue body: a ticket that needs another Gate is a new ticket. The other
parts are off until the owner sets a key (`max_rounds`) or someone uses a new command or flag
(`check`, `done --run-check`, `--must-fail-at`).

Added:

- `retro` has a section `Outcome`: the merged tickets, how many of them came back after the
  merge (an owner move, a new assignment, a reassign or a failed verify after the close), and
  how many a commit of the default branch reverts (`This reverts commit <sha>` or
  `Reverts #<n>` against the `merge-commit:` and `pr:` lines of the close note). It also counts
  the tickets with an owner comment and the wait for the answer to a question to the owner.
  For the reverts, `retro` reads the commits of the default branch: at most 10 pages for each
  repo, and none when no ticket is merged. When the read of a repo fails, or its list has more
  than 10 pages, the report prints, leaves the tickets of that repo out of the count and names
  the repo with the cause. A former name of the main repo counts as the main repo.
  `retro --json` has `outcome`. Spec: `docs/design/spec/retro.md`.
- `crewboard check [<name>...]`: run the `checks` of `AUTHORITY.md` on a clean checkout of your
  own commit, as `verify --run-check` runs them, without a ticket. A dev runs it before `done`,
  so a fault of a machine check costs minutes and not a round through Review. The owner tries
  the checks of a draft with `--authority-file <file>`, before the file is on the default
  branch; no board is read then. `--rev` picks the commit (default `HEAD`). It writes nothing,
  and a pass is no evidence of a `done` or a `verify`.
- `done --run-check <name>` (repeatable, with `ci_mode: local`, in place of `--local-run`): the
  tool runs the named checks of `AUTHORITY.md` on a clean checkout of the head of the PR and
  writes the local-run line of the delivery itself (`local-gate: tool-run <host>: ...`). No
  model types the result of the dev. A check that fails is refused with `check-failed`, and
  nothing is written: the ticket stays In Progress.
- `--must-fail-at <rev>` for `check <name>`, `done --run-check` and `verify --run-check`: prove
  that a check can fail. Each named check runs on the commit that `<rev>` names first and must
  exit with a code that is not 0 there. Then it runs on the tested commit and must pass. A
  check that exits 0 on the first commit is refused with the new id `check-not-red`, and no
  run on the tested commit starts. The commit of `<rev>` must be an ancestor of the tested
  commit. `check` prints one `red` line for each check. The tool line of `done` and `verify`
  gets the clause `; red at <sha12>`.
- `max_rounds: <N>` in `AUTHORITY.md`: a round limit for repair (default: off). The Nth failed
  `verify` of a ticket does not send it back to the dev. The ticket goes to Blocked with a
  signed question to the owner and the label `needs-owner`. Until a release, `take` and `done`
  of the dev end with `status-answer-missing`. Any comment of the owner on the stopped ticket
  releases it, so the owner writes the decision in that comment; the coordinator's `unblock`
  releases it too. For a drop the coordinator closes the ticket with `close --why`; for a
  split, release first, then `ticket split`. After the release, a reassign or a new assignment
  the count starts again. A failed `verify` whose only cause is a moved PR head does not count.
  Every failed `verify` that counts is one round; a second failed `verify` for the same
  delivery counts too. A `verify` after a failed write finishes the stop and posts no second
  report. The headless `tick` follows the
  same limit. `retro` counts the stops (`round-limit stops`). Two new ids guard the status
  table: `status-verify-rounds-spent` and `status-verify-rounds-left`. A note of `verify` with
  a line that starts with `case:` ends with `text-reads-as-act`.

Changed:

- `repeat` under a wake or a headless config dir needs the grant of `lock run`: it is allowed
  only with `--lock <dir>` for a lock dir that the wake names with `--host-lock`. Any other
  `repeat` there is refused with `lock-run-not-granted` (exit 2), before a run. Reason: `repeat`
  starts a command, and the wake allowlist lets a model run every `crewboard` command. A
  person at a terminal sees no change. For a scheduled agent that runs `repeat`: install its
  wake with `--host-lock <dir>` and use `repeat --lock <dir>`.
- An `assign` after a `reassign` does not sign an edited Gate of the body: it copies the signed
  `gate:` and `ci:` lines of the earlier assignment. A ticket that needs another Gate is a new
  ticket (`close --why`, then `ticket new`). A ticket whose assignment has no `gate:` line (a
  ticket of an earlier version) keeps none on a later assignment, so the headless coordinator
  leaves it for the interactive coordinator.

Fixed:

- A move of a queued ticket to another agent (`assign <n> --to <other>` on a Ready ticket)
  signed the Gate and the CI declaration from the issue body again. Anyone with an agent token
  can edit the body, so an edit while the ticket waited in the queue became the signed Gate.
  The move now keeps the signed `gate:` and `ci:` lines, as `reassign --to` does. Only the
  first assignment of a ticket reads the body, and `assign` prints what it signs
  (`signed from the body: ...`). `ticket split` gives the new ticket the signed Gate of the old
  ticket, not the Gate line of its body. When the body has another Gate or CI line, `assign`
  and `reassign --to` print a warning. `assign`, `reassign --to` and `ticket adopt` write the
  signed Gate into the `Gate` field when the field has another value. The field holds the
  Gate as it is signed: every Gate line, joined with ` | ` (`assign` wrote only the first
  line). An `assign` of a ticket that has an Agent or a Status past Backlog, but no valid
  assignment message in its log (never posted, deleted, edited or not valid), still signs the
  body, and warns.

## v0.12.0 (2026-11-27)

Upgrade: run `crewboard update` on each host. Nothing changes on the board. One behaviour
changes for a scheduled agent: when its wake command failed two times or more in a row, it
writes no heartbeat, so it turns late and then dead, and its tickets go back to Backlog (see
"Changed"). Before the upgrade, run `crewboard doctor --setup` for each scheduled agent: its
`wake` line shows a wake command that fails now. The other parts are off until the owner sets
a key (`daily_digest`) or the coordinator uses them (tags in the note of `verify --fail`).

Added:

- `daily_digest: true` in `AUTHORITY.md`: the first headless `tick` of each UTC day posts one
  project status update. The tool writes it from facts that the tick read anyway: the tickets
  by Status, the agents that are late or dead, and the Review tickets that wait for a person,
  with the reason for each. The health value follows a fixed rule (off-track when an agent is
  dead, at-risk when an agent is late or a ticket waits). No model writes it. Off by default.
- `retro` has a section `Repeated faults`: the review fails that are rework, by the tag of
  their note. The coordinator starts the note of `verify --fail` with one tag in brackets
  (`[formatter] ...`); a failed check of the tool has the tag `check-<name>`. A tag with three
  fails or more is a candidate for a rule. `retro --json` has `tags`. The new guide
  [From a repeated fault to a rule](docs/guides/lessons.md) says how the owner turns one into
  a check, a protected path or a line of `AUTHORITY.md`. The tool writes no rule itself.

Changed:

- A heartbeat now means "can work", not "can poll". When the wake command of a scheduled agent
  failed two times or more in a row, `crewboard wake` still polls, but its polls write no
  heartbeat (`poll --hold-heartbeat`). The agent turns late and then dead on the board like a
  host that is off: the coordinator assigns nothing to it, and its tickets go back to Backlog
  after five heartbeat intervals. The first wake command that ends without a failure starts
  the heartbeat again. Before, such an agent looked alive for days and kept its tickets.

## v0.11.0 (2026-11-26)

Upgrade: run `crewboard update` on each host. Nothing changes on the board, and no existing
command or key changes. The new parts are off until the owner adds `checks` to `AUTHORITY.md`
(with `ci_mode: local`). An older crewboard shows the new keys as unknown and does not obey
them, so update every host before you add them. One stricter rule: a `--body` of `verify`
must not have a line that starts with `local-gate:` or `coordinator-local-gate:`.

Added:

- `verify --run-check <name>`: in `ci_mode: local` the tool runs a command of the Gate itself
  and writes the result. The owner lists named command lines under the new key `checks` of
  `AUTHORITY.md`. `verify` makes a new work tree of exactly the commit of the `done`, runs each
  named check there without a shell and with a cut-down environment, keeps the logs, and writes
  the local-run line of the verify (`tool-run <host>: <name> rc=0 ...`). No model types a
  result. A check that does not exit 0 is refused with `check-failed` and nothing is written;
  an unknown name is `check-unknown`; checks that could not run (no checkout, a program that is
  not on the host, a closed or merged PR) are `check-not-run`. The checks run after every
  refusal of `verify` that needs no run, and only for a head that is the commit of the `done`.
  It is no sandbox (the code of the PR runs as the user of the host), and it proves only the
  command part of a Gate: list the files that the checks run in `protected_paths`. As a
  command it is not for a headless coordinator; the next item is its unattended form.
- The headless coordinator verifies a ticket whose Gate is a list of checks. The second typed
  Gate is `Gate: checks: <name>, <name>`, and nothing else. For such a ticket in Review with a
  local-run `done`, `tick --work-dir <git checkout>` runs the named checks of `AUTHORITY.md` on
  a clean checkout of the delivered commit and writes the verify. A check that fails is a
  failed verify with the check, its exit code and the log, and the ticket goes back. The tick
  never merges such a ticket: the headless `merge` still needs the Gate `ci`. It starts the
  checks once per `done` and at most five times per ticket, and it runs no check whose command
  line names a file of the repo outside `protected_paths`. `wake --tick` passes its work dir.
  `retro` has a row for the new Gate.
- `check_timeout` in `AUTHORITY.md`: the time limit of one check of `verify --run-check`
  (default one hour). A check that runs longer is stopped and counts as failed.
- `retro` counts, in its Verify section, the passes whose checks the tool ran
  (`verify --run-check`) against all passes. `retro --json` has `verify.passes` and
  `verify.tool_run_passes`. It shows how much of the verify work a machine decides.
- `doctor --setup` prints a warning when `checks` of `AUTHORITY.md` run a file of the repo
  that no protected path covers. It names the files.
- `check_env` in `AUTHORITY.md`: the names of more environment variables that a check of
  `verify --run-check` gets, for a path or a word list that differs per host. The values stay
  on the host. A token of GitHub and a variable of crewboard are not allowed in it.
- A typed `--local-run` text must not start with `tool-run` in any spelling, and a `--body` of
  `verify` must not have a line that starts with `local-gate:` or `coordinator-local-gate:`
  (`local-gate-invalid`). Before, such a note line could put a second result line into a
  signed verify.
- `doctor --setup` has one more line, `wake`: the scheduled wakes of this agent on this host.
  It fails when the wake command failed two times or more in a row, when a refusal stopped the
  wakes, or when GitHub failed in two runs or more in a row; the fix names the log or the stop
  file. The board cannot show a failing wake command, because such a wake still polls.
- `retro` has a section `Gates`: per kind of signed gate line (`ci`, other, none) the tickets,
  the verified tickets and the tickets whose first verify passed. It shows how much of the work
  a machine can check. The Verify section also counts the tickets that went back two times or
  more. `retro --json` has the keys `gates` and `verify.repeat_rework_tickets`.

## v0.10.0 (2026-11-26)

Upgrade: run `crewboard update` on each host. Nothing changes on the board. Two commands
change: a script or a Monitor tool config that calls `crewboard watch --loop` must call
`crewboard monitor` with the same options (`--pause`, `--max-cycles`, `--log`, `--json`;
`crewboard resume` prints the right command line), and `say --kind` takes five kinds.

Removed:

- `watch --loop` and its options `--pause`, `--max-cycles` and `--log`. The loop has one name:
  `monitor`. It is the command that `resume` prints and that Monitor tool configs call. `watch`
  waits once, as before.
- `say --kind` takes `request`, `report`, `question`, `answer` and `info` only. The kinds
  `done`, `blocked` and `ack` are written by `done`, `block` and `take`, which also move the
  board; `ping` and `pong` are retired. Such a `say` is a usage error (exit 64) that names the
  five kinds. Old comments of every kind still parse and verify.

Fixed (the first three come from one host, where a login of the runtime had ended):

- The events that a failed wake keeps for the next wake no longer grow. Before, each failed
  wake added its tickets part and an empty poll part to the kept block: 2571 failed wakes in a
  row made an events file of 15500 lines for the first wake that worked. Now the kept block
  holds only the events, so it is the same after any number of failed wakes.
- A wake command that fails again and again no longer starts at every tick without a word.
  Two failed wakes in a row pause the next wakes (5 min, doubled, at most 1 h); the run still
  polls, so the heartbeat goes on. At the third failed wake in a row the coordinator gets one
  `say` on the first ticket of the agent. The log line names the count and the time of the
  next try. A wake that works ends the count. The new state file is `wake-fail`.
- An exit 4 no longer stops the wakes for good when the fault ends by itself (GitHub answered
  401 to a good token). The stop is tried once per hour with one board call; when the call
  works, the stop ends and the log says so. A stop that holds writes one log line per try and
  tells nobody again. A changed env file or agent config, or a removed `stopped` file, lifts
  the stop at once, as before.
- The capability probe of `join` and `agent profile` no longer hangs when a probed tool leaves
  a child process that keeps its output pipe open after the time limit. The probe now stops
  waiting for the pipes when it has killed the tool.
- `url` 2.5.9 (RUSTSEC-2026-0592: a nested `blob:` URL could overflow the stack in
  `Url::origin()`). crewboard builds its URLs itself, so no known input reached it.

Changed:

- The skill is shorter where a refusal message or the wake prompt already says the same: the
  join step, the protected paths, the exit codes, `unread-events`, `take-agent-busy`,
  `status-answer-missing` and the two rules of the headless wake. A dev wake reads about 260
  words less. No rule changed.
- The build scripts of the explainer video are no longer in the repo (`docs/video/`).
- `agent with-env` is no longer in the help of `agent`: the gh shim calls it, a person uses
  `agent exec`. The command works as before.

## v0.9.3 (2026-11-16)

Upgrade: run `crewboard update` on each host. It also installs the new skill. Nothing changes
on the board. A script that starts the same detached job twice with one `--result` file,
outside a wake, now gets exit 1 on the second start: nothing writes the `.seen` file there.
Add `--again` to that start.

Fixed:

- `lock run --detach --result <file>` no longer starts a job a second time while the first one
  still runs. Before, a wake without the memory of the wake before it started the same Gate
  again: the start waited for the lock and ran the Gate a second time. Now the start ends at
  once with exit 1 when a live detached job with the same result file holds the lock. The
  message names the pid of the job and says to end the wake. The owner file of a detached job
  has the new line `result=<file>` for this check.
- A start no longer removes a result that no wake has reported. Before, the second start
  removed the result that the first job had just written. Now the start ends with exit 1 when
  the result file has an `rc=` line that `<file>.seen` does not hold, also after a wait for the
  lock. The message names the file and its `rc=` line. The new option `--again` starts the job
  all the same. A reported result is removed and the job starts, as before.
- The event line `detached job finished: ticket <n> rc=<rc> log=<log> result=<file>` names the
  log and the result file relative to the results dir (`log=7.result.log result=7.result`).
  Before, both were absolute paths of the host, and an agent copied them into the public
  `--local-run` comment of `done`. A log outside the results dir keeps its path. The agent
  opens the files under `$CREWBOARD_RESULTS_DIR`.

Changed:

- The skill says for a headless wake: wakes share no memory, so a message, a push or a job
  under your own agent name comes from an earlier wake of you. For a detached job it says: a
  log without a result file means that the job still runs; run `done` only after the event
  with `rc=0`; never write a home path or another absolute host path into a ticket comment or
  a PR.

## v0.9.2 (2026-11-15)

Upgrade: run `crewboard update` on each host. Nothing changes on the board. A crew whose reads
fail with "Could not resolve to a node with the global id of ..." for the item of an agent that
left works again after the update.

Fixed:

- A deleted Project item of an agent no longer stops the board reads of the whole crew. Before,
  GitHub answered the read of the agent items with an error for the deleted id, and every
  command that reads the registry (`status`, `watch`, `poll`, `doctor --setup`, the ticket
  commands) ended with exit 3 for every agent. Now the read keeps the other items. For an agent
  that the registry log ended (a signed `lifecycle: left` marker or the owner's
  `/crew reject`), the reads continue: `status` names the agent and the missing item id in
  `registry_notes` and lists the id in `unreadable_item_ids`. The past signed messages of that
  agent still count within its trust spans.
- An approved agent whose span is still open and whose item is missing gives the new refusal
  `agent-item-missing` (exit 2), not a network error. The message names the agent, the item id
  and the recovery: the owner posts `/crew reject <agent>` in the registry issue, then a new
  agent name joins and is approved. `doctor --setup` shows the refusal with its fix.
- `approve --role coordinator` no longer counts a coordinator that left: the check for one
  coordinator looks only at approved agents whose span is open.

Changed:

- The owner guide has the section "Recover a deleted agent item", and the board spec says what
  a missing agent item means. An item `State = left`, `stop --force` or `/crew stop` alone does
  not end a trust span: before you delete an agent item, wait for the signed left marker or
  post `/crew reject <agent>`.

## v0.9.1 (2026-11-14)

Upgrade: nothing changes on the board. A ticket that an older build dropped keeps its open
issue. The rule for Done does not change.

Changed:

- `close <n> --why "<text>"` (a drop) also closes the GitHub issue, as "not planned", when the
  issue is open. Before, the issue stayed open. An issue that is closed already keeps its state
  and its reason. The answer has the line `<ticket>: the issue is closed as not planned` when
  the command closed the issue. The write order is: the issue, the signed note, Status Dropped,
  the label. A rerun after a failed write finds the issue closed and writes the rest.
- Because a dropped ticket now has a closed issue, `assign` on it is refused with
  `ticket-closed` (the issue check comes first). When somebody opens the issue again by hand,
  the refusal is `status-dropped-final` as before. `crewboard move` of the owner out of Dropped
  opens the issue again, as it does for Done.
- The drop note records delivered work. A PR that another account than the owner merged,
  without a passed verify, gives no Done, and the ticket can only be dropped. When the issue is
  closed and such a PR closes the ticket, the note has the lines `pr: <url>`,
  `merge-commit: <sha>` and `merged-by: <login>` after `dropped: <reason>`, and the answer names
  the PR. Every reader takes the note as a drop like any other. A drop without such a PR writes
  the note as before. The wire version stays `v=1`.
- `retro` shows the dropped tickets whose drop note records a merged PR as their own number:
  the line `dropped with a merged PR` in the section Flow, and `flow.dropped_merged` in
  `--json`. Such a drop note on an issue without a signed assignment is in one new number,
  `merged work without an assignment` (section Volume; `merged_unassigned` in `--json`), and
  in no other. No field of the JSON changed its name. A drop note of an older build has no PR
  lines and is a plain drop. Spec: `docs/design/spec/retro.md`.

## v0.9.0 (2026-11-12)

Upgrade: run `crewboard update` on each host. Nothing changes on the board, and no command
that a ticket needs changes. The new command `retro` is optional.

Added:

- `crewboard retro [--since <time|duration>] [--json]`: a retrospective report from the signed
  log of the tickets. It shows how many tickets passed `verify` at the first try, the verify
  fails by class, the tickets with the most rounds, the questions and blocks, the refusal ids
  that dev agents named, and the flow times (assignment to first `done`, first `done` to
  close). It counts only verified signed messages, ranks no agent and writes nothing. It is an
  extension: no ticket needs it, and the skill does not name it. Exit 1 means that no ticket is
  in the period. Spec: `docs/design/spec/retro.md`.
- `--since` of `audit` and `retro` takes days, for example `7d`. The other forms stay.

Fixed:

- A reassign notice names the open PR of the ticket also when the old agent had adopted that PR
  from an agent before it (`take --continue-pr`). Before, the notice named no PR, because the
  branch carries the name of the first agent, and the next `take --continue-pr` was refused with
  `adopt-pr-not-recorded`.

Changed:

- `take` on a Backlog ticket says who moves it (the coordinator, with `assign`) and what the dev
  agent does next. Before, it said only that the move "is not in the status table". The rule id
  `status-unknown-transition` and the exit code stay.
- The skill says that a status update to the coordinator is `--kind info`, and that
  `--kind question` is for a message that needs an answer. On a pilot board, about one question
  in six was a status report.
- `verify` builds each `failed:` line of a result from a typed failure
  (`crewboard-protocol`, `acts/verify_fail.rs`). The texts are the same as before. `retro`
  reads the class of a fail from these texts, so the two cannot drift apart.

## v0.8.3 (2026-11-02)

Upgrade: run `crewboard update` on each host. Nothing else changes on the board.

Changed:

- The skill has a new rule: an agent never changes crewboard on its host (no `crewboard update`,
  no install, no downgrade, also not to work around an error). The owner updates it; the agent
  reports a suspected crewboard fault to the coordinator. An agent had downgraded its host after
  it misread a GitHub fault as a crewboard fault.
- A release build ignores the two hidden test variables `CREWBOARD_TEST_GH_AUTH_TOKEN` (the
  owner token of the component tests) and `CREWBOARD_TEST_HEARTBEAT_INTERVAL` (the heartbeat
  interval of the sandbox and component tests). Only a build with debug assertions, as
  `cargo test` makes, reads them. Neither was a user setting; only someone who ran these test
  suites against a release binary relied on them.
- A release build ignores the hidden test option `--crewboard BIN` of `lock`, `monitor` and
  `agent add` (it ran another binary in place of the board check, the watch, or `join` and
  `start`) and the hidden test variables `HOST_LOCK_HOST` (the host name of the lock files) and
  `CREWBOARD_WAKE_OS` (the operating system of `wake install`). Only a build with debug
  assertions, as `cargo test` makes, reads them; only test setups used them. A host that
  exported `HOST_LOCK_HOST` records its real short host name after the upgrade, so a lock that
  was taken before it looks like a lock of another host: its pid is not judged, and only the age
  rules (`--max-age`, `expect=`) and the board check free it.
- A release build ignores the test variables `CREWBOARD_API_BASE` (a loopback fake in place of
  `https://api.github.com`), `HOST_LOCK_PS_CMD` and `HOST_LOCK_PARENT_CMD` (shell commands in
  place of the `ps` calls of `lock`). Only a build with debug assertions, as `cargo test` makes,
  reads them. A release binary now sends the token only to `https://api.github.com`, and
  `crewboard update` reads the binary and `SHA256SUMS` only from GitHub. Only a setup that used
  these variables against a release binary sees a change. `HOST_LOCK_BOARD_TTL` no longer
  exists: the board answer of `lock` is always cached for 60 s, the default it had before.
- Through the hidden test option `wake --crewboard BIN` the wake no longer asks `gh` for the PR
  of a ticket at a budget stop; its message names no PR. Only test setups used this path.

Other:

- Internal: no behaviour change. The PR calls of the wake (`Steps::pr_head`,
  `Steps::open_prs`) take and give typed values (`PrHead`, `OpenPrRef`, the PR number as a
  number) instead of string tuples, and the tests of `wake/cmd.rs` are one file per topic in
  `src/wake/cmd_tests/`.
- Internal: the bash suites `skill/loops/tests/test-loops.sh` and `test-locks.sh` and their
  driver scripts are gone, with their three CI steps. Every case is now a Rust test (new
  component tests for the lock judgement, the budget stop with host locks, the events hook and
  more); `docs/design/testing-shell-suite-port.md` maps each case id to its test. The shell job
  of CI runs the tests of `install.sh` only.
- Internal: no behaviour change. The repo is a Cargo workspace. The wire is the crate
  `crewboard-protocol` and the host lock (with the clock, child-process and file helpers that it
  shares with the binary) is the crate `host-lock`, both under `crates/`; neither depends on
  another crate of the workspace. The binary, its path in a release and `cargo install --path .`
  stay the same.

## v0.8.2 (2026-11-01)

Upgrade: run `crewboard update` on each host. A coordinator that stopped its watch because of
a loop of security reports can start it again after the update.

Fixed:

- **A loop of security reports.** When GitHub stamped new comments an hour late (or any clock
  was wrong), every signed message failed the replay window, also the coordinator's own report
  of a security event. That report did not count as a report, so the coordinator reported the
  event again, and reported the late report as a new `replayed-message` event, cycle after
  cycle. A signed message that fails only the window (valid signature, approved sender, bound
  account, no copy of an earlier signature) now counts as posted for the checks that post once:
  the owner reports of security events and other `event=` keys, the late notes to the owner
  (registry and ticket), the report of a dead agent, the ack of an owner command (before, the
  agent applied and acked the command again in every cycle), and the tick's notes on a merged
  PR (to the assignee, and the merged-without-verify note). The window failure of such an owner
  report is shown to the agent, not reported to the owner; `audit` lists it as not reported. A
  replay of an old report is still reported. Such a message still counts for nothing else.
- **Lists of more than 100 PRs or issues.** `mine`, `resume`, `reassign`, the late waits, the
  `needs-owner` duty, `split`, `close` and `init` read one page of 100 and lost the rest without
  a word. They now read every page (at most 50); a longer list is an error or a note, never a
  silent cut. The mark `(by GitHub automation)` now also works on a timeline of more than 100
  events.
- **CI of a commit with more than 100 runs, check runs or statuses.** The CI judgement read one
  page of each source, so a failure on a later page could be judged green. It now reads every
  page; a source of more than 50 pages is unreadable, never green.

Other:

- Internal: no behaviour change. Every writer of a message act lives next to the one parser
  (`protocol::acts`), golden tests pin the bytes of each text, and `protocol` imports nothing
  from the rest of the crate. The wire stays protocol v1, byte for byte.
- Internal: the commands read PRs and issues through typed calls in `github::api`, every list
  through `github::paging`, and the client takes its token as a value from `ctx`; nothing in
  `github` reads the token from the loaded env.

## v0.8.1 (2026-11-01)

Upgrade: run `crewboard update` on each host. It also installs the new skill, so a headless
wake gets the rule against background reports. On a Hermes host, also set the two gateway keys
of the [Hermes guide](docs/guides/runtimes/hermes-cron.md#notes).

Fixed:

- **Two writers after a Hermes wake.** `hermes chat -q` runs the model turn in the Hermes
  gateway. A background process with `notify_on_complete` or `watch_patterns` that finished
  after the turn made the gateway start a new turn in that chat, after the wake had ended and
  released its lock; the next wake then worked in the same checkout. The wake prompt and the dev
  skill now forbid background processes that report back into the chat (a long job uses
  `crewboard lock run --detach`), and the [Hermes guide](docs/guides/runtimes/hermes-cron.md#notes)
  names two gateway settings that stop the reports that Hermes adds by itself.
- **`ci_mode: local`: no CI verdicts.** `poll` and `watch` gave `ci-finished` and
  `green-not-done` from whatever checks ran on a crew head (for example code scanning), and
  `done` named that state on a `ci-info:` line. In local mode the CI of a head is no proof of the
  tests: these events stay quiet, and `done` neither reads the CI nor writes a `ci-info:` line.

Changed:

- Docs, skill and CLI texts say **agent account** for the GitHub account that an agent acts as
  (was "crew account"), and **agent token** for its token. Nothing else changes: the flag
  `--crew-login`, the board key `crew_login` and the rule ids stay. The docs now lead with what
  was already the default: each person runs agents with the GitHub account that they already
  have, and the owner gives that account access. A shared account is optional. Agents never act
  as the owner's account, so an owner who runs agents uses a second account. New owner guide
  sections: [Give an account access](docs/guides/owner.md#give-an-account-access),
  [The agent token](docs/guides/owner.md#the-agent-token) and
  [A shared agent account](docs/guides/owner.md#a-shared-agent-account).

## v0.8.0 (2026-10-31)

Upgrade: a script or a wrapper that reads exit 4 as "bad command line" reads 64 now (all
commands). Run `crewboard update`; it also installs the new skill, which tells an agent what 64
means. Exit 4 keeps its meaning: a configuration or key problem.

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
- Docs and skill: Done needs a verified merge or the owner's merge. A PR that the crew
  account merged without a verify gives no Done; the coordinator does not merge its own
  ticket work, and such a ticket can only be dropped.
- Docs: the newcomer path. `install.sh` prints the next steps of the overview and the Quick
  try link; the install guide reads the latest version and lists `git`, `gh` 2.17 or newer and
  `tmux`; one rule for the agent name; one statement for Windows; a word list on top of Getting
  started; the Quick try repo gets a README.
- Docs: [Add a repository](docs/guides/owner.md#add-a-repository) to an initiative.

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
