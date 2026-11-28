# Agent operation

Part of the [crewboard specification](README.md).

## Toolchain check of `start`

A dev agent may run in a harness sandbox that allows only its work folder. The build tools of
the repo (in the user's home) are then not on its PATH. Without a check, such an agent passes
`start`, gets tickets and blocks at its first `take`.

`.crewboard.toml` has two optional keys:

- `required_tools = ["git --version", { cmd = "go version", tags = ["go"] }]`: commands with a
  version flag. An entry is a plain string (every agent) or a table `{ cmd = "...", tags = [...] }`
  (only an agent that declares at least one of the tags). Both forms can be mixed in one list.
  `tags` uses the tag syntax of [profiles](profiles.md) and must not be empty. A bad entry is a
  config error that names the entry;
- `smoke_cmd = "make test"`: one command that proves the build works.

The commands come from the repo's own config: crewboard knows no stack. Other examples: `node
--version` and `npm run build`, `python3 --version` and `python3 -m pytest -q`, `./gradlew
assemble`, `cargo check --workspace`.

`start` runs the tools, then the smoke command, from the current work folder of the agent, with
the PATH of the agent. Each command runs through the platform shell: `sh -c` on Unix, `cmd /C`
on a non-Unix host. One command may run 30 s (tools) or 300 s (smoke); a command that hangs
fails. If one fails, `start` refuses with rule `toolchain-unreachable` (exit 2). The agent
does not become `active`. The agent writes the fact token `toolchain:unreachable` into its
`Capabilities` field, and `status` shows a note for that agent, so the coordinator does not
assign build tickets to it. The next `start` that passes removes the token (facts are replaced
at every `start`, [Facts](profiles.md#facts)). No keys: no check.

The check uses the declared tags of the agent that runs `start` (from its `Capabilities` field;
facts such as `tool:node` are not tags). A tagged tool that the agent has no tag for is not run.
`start` and `doctor --setup` list `checked:` and `skipped by tag:` (each skipped tool with the tags
that would need it). An agent without tags checks only the untagged entries. `smoke_cmd` has no
tags and runs for every agent.

`start` reads the keys from the file `.crewboard.toml` on the default branch of the main repo
when that file sets at least one of them. Otherwise the copy that `join` saved applies.

## Unattended dev agents keep working


An unattended dev agent must not stop after it has handled its events: an In Progress ticket must
not stand still, and a busy agent must keep its heartbeat during one long wake.

1. **Skill.** The sections for an unattended wake (`skill/references/dev.md`, `docs/guides/runtimes/hermes-cron.md`) say: handle every
   event, then continue the In Progress ticket until `done` or `block`, then `take` the next Ready
   ticket (lowest `Order` first), and stop only when there is none or the run budget ends.
2. **`crewboard mine [--quiet]`** lists the own In Progress and Ready tickets, In Progress first,
   then Ready in queue order, and then one verdict line per owned ticket (`work: ...` or
   `wait: ...`, see [Owned work that only waits](#owned-work-that-only-waits)). Exit 0 if a
   ticket needs the agent now, exit 1 if there is nothing to do now (the exit code 1 means
   "nothing", as for `poll`; see [Exit codes](cli.md#exit-codes)). `--quiet` prints nothing.
   **`crewboard mine <n>`** prints the signed thread of one own ticket since the current
   assignment of the agent (the latest signed assignment to it, or the reassign notice right
   before it), newest first: the messages of the coordinator, the owner comments that count
   ([verified messages](tickets.md#status-flow-and-writers)), and the agent's own messages but
   the `ack`. Each message has one header line (time, sender, `to`, kind, act and the
   `#<n>/c-<id>` form that `say --re` takes) and its body, cut at 800 characters with a note.
   The newest 15 messages are shown; one note counts the older ones, and one line counts the
   comments that do not count (not signed by the crew, a copy, or edited). The events of a wake
   start at the cursor, so an answer or a verify note before it is only here. A ticket that the
   signed log does not assign to the caller is refused (`mine-not-your-ticket`). It reads the
   context and the trust check of every agent command and one comment list, writes nothing and
   exits 0. `--json` gives `ticket`, `agent`, `since`, `messages`, `left_out` and
   `not_counted`.
3. **Wake for owned work.** `crewboard wake`: when `poll --quiet` finds no events, it asks
   `mine --quiet`. If `mine` exits 0 and the last wake ended at
   least `--work-backoff` seconds ago (default 900, `CREWBOARD_WORK_BACKOFF`, `off` disables), it
   wakes the agent (`CREWBOARD_WAKE_REASON=work`). Every wake starts the back-off. A quiet board
   with no owned ticket, or with owned tickets that only wait, starts no model and prints
   nothing.
4. **Heartbeat during work.** `take`, `say` and `done` write the heartbeat when it is due (once
   per heartbeat interval, only in a running State), as `watch` and `poll` do. While a wake
   command runs, `crewboard wake` sends `crewboard heartbeat` every `--heartbeat-every` seconds
   (default 300, `CREWBOARD_HEARTBEAT_EVERY`, `0` disables) and stops the ticker when the wake
   ends. The default heartbeat interval of the initiative stays 10 minutes.

## Owned work that only waits


In one measured run, 53 of 64 owned-work wakes (83 %) did no ticket work. Most came while the owned
ticket waited on CI, on an answer or on the owner. Each such wake started a model and cost a
median of 125k to 158k tokens. `mine` now decides with board and GitHub data, so the
wake script starts no model for a ticket that only waits.

1. **Work now (exit 0).** At least one owned ticket needs the agent:
   - In Progress after a failed verify, when the PR head is still the head that verify saw
     (`head_now:` of the verify report).
   - In Progress with red CI, green CI (the agent runs `done`), no CI run for the PR head, no
     PR yet, or a PR or CI that cannot be read.
   - Blocked with an answer after the block or the question (the rule of
     [Status flow and writers](tickets.md#status-flow-and-writers): an owner message, or a
     coordinator `answer` after a `blocked` message; a question needs an owner message or a
     coordinator `answer` with the line `withdraws: question`). The agent runs `take <n>`.
   - Ready, or parked by the agent itself, while the agent has no In Progress ticket.
2. **Only waits (exit 1).** Every owned ticket waits:
   - In Progress while a run, check or status of the PR head is not finished.
   - Blocked without an answer after the block or the question (`Blocked, waits for the owner`
     while a question to the owner is open).
   - Review (the coordinator verifies; `Review, waits for the owner's answer` while a question to
     the owner is open).
   - Ready, or parked, behind the In Progress ticket of the agent (`take` refuses a second
     In Progress ticket).
3. **The PR.** The PR that the latest `done` of the agent names, else the open PR whose branch
   is `<prefix><agent>/<issue>-<slug>`. `mine` matches any prefix and reads no AUTHORITY.md.
4. **Read errors.** A read error keeps the earlier verdict: an In Progress ticket counts
   as work; a Blocked ticket counts as waiting. The errors of the board read keep
   their exit codes (3, 4).
5. **Cost.** One comment read for each Blocked, Review and In Progress ticket, and for each
   In Progress ticket one PR read (or one open-PR list per repo) and the CI reads of `done`
   (workflow runs, check runs, statuses). A dev agent has one In Progress ticket, so this is
   about 7 REST reads more per `mine`, and the wake script asks only after the back-off, with no
   events. One model wake that is not started saves far more.

## Coordinator failure: one owner alert


Only the coordinator reassigns the tickets of a dead agent. If the coordinator goes offline, no
agent and no event notices it, and there is no failover: the coordinator role needs the owner's
approval (`crewboard approve <agent> --role coordinator`). So the owner is told. The alert is the
`heartbeat` wait of the coordinator in the table of late work ([Stale work](events.md#stale-work)):
no heartbeat for 3 intervals ([Fields](board.md#fields)), judged from the last heartbeat or, if
there is none, from the newer of the approval and the start. Every running dev agent judges it
in each `poll` and `watch` cycle (also `poll --quiet`) from the data of the cycle, and posts ONE
signed `info` to the owner in the registry issue if no `info` with the same `late-key:` is there
([item 9 of Stale work](events.md#stale-work)). The note is no event and wakes no model. A dev agent
never alerts about itself or about another dev agent, and never does coordinator work; the
coordinator never posts about itself.

## Restart of an agent: `resume`


A session of an interactive agent can restart (model switch, account switch, app restart). A
restart kills the watch loop and the CI watchers. Nothing told the resumed agent what to re-arm
or what waited meanwhile.

1. **`crewboard resume`** (agent, no `--agent`) does these steps in order:
   - If the agent is `paused`, it lifts the pause as before ([Lifecycle](board.md#lifecycle)). A pause by the owner stays:
     the rule `resume-owner-paused` refuses it.
   - It runs the checks of `start` (approval, single coordinator, AUTHORITY.md, selftest,
     toolchain, facts). An agent that is `active` stays `active`; the heartbeat is written.
   - It reads the board and the feeds. It moves no cursor and posts no message.
   - It prints the to-do block of the role and the re-arm command.
   `resume --agent <name>` (owner) is unchanged: it posts `/crew resume <name>`.
2. **Coordinator block.** Done messages without a verify (a ticket in Review whose latest `done`
   has no verify result for exactly that message); tickets in Review and Blocked;
   open PRs of crew branches (head branch starts with `dev_branch_prefix`) with the CI state of
   the head (`green`, `no run yet`, `not green: ...`, `unreadable`); the number of events since
   the cursor; the re-arm command.
3. **Dev block.** The own In Progress ticket; the own Ready queue, next ticket first; the own
   tickets in Blocked (asks and blocks that wait for an answer); the number of
   events since the cursor; the re-arm command.
4. **`[loop]` table.** The agent config (`<config-dir>/<initiative>.toml`) may have a table
   `[loop]` with the optional keys `interval`, `timeout` (durations like `60s`, `30m`) and `log`
   (a path). `watch` uses `interval` and `timeout` when its flag is not given; a flag wins. The
   re-arm command is built from the table; without it `timeout` is 30m, no `--interval` is
   passed and the log is `<config-dir>/watch-loop.log`. A value that does not parse is a config
   error (exit 4), never a silent default. An unknown key in `[loop]` is an error that names the
   file, the table and the key ("unknown key `intervall` in [loop]; allowed: interval, timeout, log"). `join` keeps the table. `crewboard monitor` passes
   `--timeout` only when it is given, so the table applies to it too.
5. The re-arm command is `crewboard monitor` with
   `--env-file` (only if `<config-dir>/<initiative>.env` exists), `--config-dir` (only if it is
   not the default), `--initiative`, `--timeout`, `--log`, and `-- --interval I` if set.
   If `<config-dir>/agent.env` exists (an agent dir), the
   command is `crewboard --config-dir <config-dir> monitor`
   with the same `--initiative`, `--timeout`, `--log` and `-- --interval I`. The binary loads
   `agent.env` itself, so the line names no env file.

## Headless coordinator


A scheduler can run the coordinator without a person. The default is
`crewboard wake --tick`: it runs `crewboard tick` under a lock and writes
the tick lines to the log, with no model. That coordinator has the power to merge, so the binary
limits it. The prompt does not.

1. **The scope is a setting of the config dir.** The agent config
   (`<config-dir>/<initiative>.toml`) has the top-level key `headless = true`. The owner or the
   setup writes it once, as the first line of the file (before any `[table]`). The wake loop
   uses this config dir (`--config-dir`, exported as `CREWBOARD_CONFIG_DIR`). The model passes
   no flag, so it cannot leave the scope by leaving a flag out. This key is a guardrail against
   mistakes and prompt drift, not a security boundary: the headless process runs as an OS user
   that can write its own config dir, so a model that wants to leave the scope can remove the
   key or use another config dir. Without the key it has the rights of the coordinator that
   the owner approved, not more. The boundary is the OS user and its token (item 9). A value
   that is not `true` or `false` is a config error (exit 4), never a silent "not headless".
   Every agent config of the directory counts: one `headless = true` is enough. `start` prints
   a `scope:` line when the scope is on.
2. **The interactive coordinator stays free.** It uses its own config dir, which has no
   `headless` key (the same agent name, a copy of the config and the key). `crewboard` reads
   no environment variable and no flag for this, so nothing in the interactive session is
   restricted. The limit of the per-dir key: an agent that can read the interactive config dir
   can point `--config-dir` at it. Keep the interactive config dir out of reach of the
   headless OS user (mode 700, owned by the interactive user), because it holds a signing key.
3. **Verdicts.** The first thing `crewboard` does, before it loads a config or sends a
   request, is to check the command. A refused command prints `refused [rule=headless-scope]`,
   exits 2 and sends no request at all. A judgment of the unattended coordinator is the one
   exception: such a command loads its context and reads `AUTHORITY.md` and the ticket, then it
   decides. With the mode `off` it prints `refused [rule=unattended-off]`, exits 2 and writes
   nothing.

   | Command | Headless |
   |---|---|
   | `status`, `mine`, `audit`, `retro`, `agent profile show` | allowed (read-only) |
   | `watch`, `poll` | allowed. They are not read-only: each cycle, also with `--quiet`, runs the coordinator duties (item 6) |
   | `start`, `resume` (own), `agent selftest`, `heartbeat` | allowed (liveness of the agent itself) |
   | `tick` | allowed (it only runs `verify` and `merge`, closes owner-merged tickets and notes a PR merged without a verify; see 8). With `daily_digest` it posts one status update per day (see 8a). With `--work-dir` it also runs the checks of a ticket whose Gate is `checks: ...` (see 5a) |
   | `verify` of a ticket whose signed Gate is `ci` and that does not declare `CI: none` | allowed (no `--run`, no `--body`; it counts the named runs of the evidence, [ci](ci.md#which-ci-runs-count) point 10); a ticket with an open question to the owner is refused, also after the owner answered in a chat ([Verify after the owner's go](tickets.md#verify-after-the-owners-go)) |
   | `verify` of a ticket with any other signed Gate or with none, or of a ticket that declares `CI: none`; `verify --fail` | a judgment of the [unattended coordinator](#unattended-coordinator): refused with `unattended-off` unless `AUTHORITY.md` allows it. The command reads the ticket first, because the signed Gate decides |
   | `verify --body` of a ticket whose signed Gate is `ci` | refused (`headless-scope`): such a verify only reads the evidence and the CI again, and a free text on it is counted by nothing |
   | `verify --run-check` | refused as a command (`headless-scope`, no request): it runs code of the PR on this host. `tick` runs the checks itself, and only for a Gate that names nothing but checks (see 5a) |
   | `say`, `ask`, `unblock` with `--body-file <path>` | refused (`headless-scope`, no request): the command would publish a file of this host. The text comes from `--body` or from standard input (`--body-file -`) |
   | `merge` | allowed, with the checks of 4 (no `--run`); a ticket with an open question to the owner is refused, as for `verify` |
   | `close` (bare) | allowed (the owner merged the PR by hand: a fact, not a judgment; [Close after an owner merge](tickets.md#close-after-an-owner-merge)); a ticket with an open question is allowed too |
   | `close --verified`, `close --why` | refused (a judgment of the interactive coordinator; [Close without a PR](tickets.md#close-without-a-pr)) |
   | `assign <n> --to <agent>`, `unblock`, `ask`, `say --kind answer --to <agent>` | a judgment of the [unattended coordinator](#unattended-coordinator): refused with `unattended-off` unless `AUTHORITY.md` allows it |
   | `assign` with `--order` or `--override-capabilities`, `assign <n> --order <number>`, `reassign` | refused |
   | `ticket new`, `ticket adopt`, `ticket split` | refused |
   | any other `say`, `take`, `done`, `block` | refused |
   | `report-status`, `prune`, `agent profile set`, `agent token` | refused |
   | `pause`, `stop` (own or `--agent`), `resume --agent` | refused |
   | `init`, `join`, `approve` | refused (owner and setup) |
   | `lock take`, `release`, `status`, `break`, `ci-*` | allowed (files of the host, no board write) |
   | `lock run <dir>` | allowed only for a lock dir that the wake names with `--host-lock` (`CREWBOARD_LOCK_RUN_DIRS`); else refused with `lock-run-not-granted` ([host lock](host-lock.md)) |
   | `check` | allowed. It runs the checks of `AUTHORITY.md` on the default branch as `tick` runs them (no check whose command line names a file outside `protected_paths`) and writes nothing ([Checks before a delivery](ci.md#checks-before-a-delivery)) |
   | `check --authority-file` | refused (the command lines of a local file; a draft of `AUTHORITY.md` is owner work) |
   | `repeat` | allowed only with `--lock <dir>` for a lock dir that the wake names with `--host-lock`, as `lock run`; else refused with `lock-run-not-granted` |
   | `verify --run`, `merge --run` | refused (a named run belongs to a Gate that is not CI only) |
   | `verify --local-run` | refused (a local run is a judgment of the interactive coordinator, [Local run](ci.md#local-run-ci_mode-local)) |
   | `merge` of a local-run ticket | refused (the evidence is `ci: local-gate`, so the Gate is not CI only) |

   A new command has no verdict until someone writes one: the match has no catch-all arm.

   `join` is refused in a headless dir before it reads or writes anything (test in `tests/component/fake_rejoin.rs`). A board move needs a join from an interactive config dir; copy the new config to the headless dir by hand. As a second line of defense, the writer of the agent config keeps `headless` and every other key it does not own.
4. **`merge` and the Gate.** Besides all checks of [`merge`](tickets.md#merge) (AUTHORITY.md `coordinator_may_merge`,
   no protected path in the PR, the verify, the evidence, green CI), a headless `merge` needs:
   (a) a Gate that is CI only, and (b) a counted CI run that is green for the verified SHA (a
   ticket with `CI: none` is refused). Both checks run inside the merge operation, before the
   PR is merged. They refuse with `headless-scope` and write nothing. Evidence whose [CI proof](ci.md#ci-proof) is `none`, `skipped` or
   `local` is not CI only: the headless `merge` refuses it before any read, and `tick` leaves it
   with a "left for the interactive coordinator" line. The same holds for `runs` evidence that
   names a CI run by hand (`done --run`, a `named` entry in its `ci-runs:` line): a run by hand
   is part of a Gate that is not CI only.
5. **CI only, exactly.** The Gate is the signed `gate:` line of the latest assignment message
   (a coordinator `request`; the first `assign` of a ticket writes it from the ticket body,
   every later assignment copies it forward; [Ticket content](tickets.md#ticket-content)). The issue body is NOT read: any holder
   of an agent token can edit it after the assignment. The typed CI-only Gate is the value `ci`
   (`Gate: ci` in the body; one parser, `Gate::parse`, reads it): the signed text, trimmed, is
   `ci` in any letter case. No new signed line is needed: the signed `gate:` text is the value,
   and wire format `v=1` does not change. Phrases such as `CI green` or `CI only` are not
   CI only: such a ticket is left for the interactive coordinator. Any other text is not CI only: a
   command (`make test passes`), a by-hand check, a second condition (a body with a second
   `Gate:` line is signed as `a | b`), an empty Gate. A ticket without a signed `gate:` line is not CI only
   (`no signed gate line on <key>`). A ticket that declares `CI: none` is not CI only (the declaration is the signed `ci: none` line of the assignment; an old ticket without a signed `ci:` line uses the live body). When in
   doubt, the answer is "not CI only". `ticket new --gate-ci` writes `Gate: ci` (a
   body with another `Gate:` line is refused, so no real condition is lost), and the `left`
   line of `tick` names the signed Gate and the flag.
   5a. **A Gate of named checks.** The second typed Gate is `checks: <name>, <name>` (`Gate:
   checks: tests, format` in the body): the key in any letter case, then one name or more of
   lower-case letters, digits and `-`, no name twice, and nothing else. One more word makes
   the Gate prose, and a person reads it. The names are checks of `checks` in `AUTHORITY.md`
   ([Checks that the tool runs](ci.md#checks-that-the-tool-runs)). For a ticket in Review with
   such a Gate and a local-run `done`, `tick --work-dir <git checkout>` does what `verify
   --run-check` does: it runs the named checks on a clean checkout of the head of the `done`
   and writes the verify with the line of the tool. Rules:
   - **It never merges such a ticket.** Its next line for the ticket is "its checks passed
     (verified); a ticket with a `checks:` Gate is merged by a person". The headless `merge`
     still needs the Gate `ci`.
   - **A check that fails is a fact, not a judgment.** `tick` records a failed verify of the
     `done` that was tested (one read of the ticket serves the run and the result). Its failure
     text is `the tool ran the checks of the Gate: check <name> exited <rc> on a clean checkout
     of <sha> (<n>s). Log: <path>`; a check that ran longer than `check_timeout` reads "ran
     longer than its limit". The output of the check is text of the PR and stays in the log.
     The ticket goes back to its agent.
   - **Once per `done`, five times per ticket.** The tick notes the `done` in its state cache
     before it starts the checks. It does not start them again for that `done`, also when the
     verify could not be written after the run; the ticket is then left for the interactive
     coordinator. After five starts for one ticket the tick leaves the ticket: a `done` costs
     the dev one line, and a run costs the host minutes.
   - **Nothing runs** without `--work-dir`, for a `done` with CI evidence, for a `done` with a
     `protected-paths:` line, for a closed or merged PR, or when the checks cannot start (no
     checkout, a failed fetch, a program that is not on the host: `check-not-run`). The ticket
     then gets a "left" line, nothing goes to the dev, and the next tick tries again.
   - **No check whose file a PR can change.** A word of a command line that is a file of the
     checkout must be in `protected_paths`. In an interactive run this is a warning; here it is
     `check-not-run`, because nobody reads a warning and nobody reads the diff. A file that a
     command reads without a word for it (a test that `cargo test` runs) is not seen.
   - **It runs code of a PR with nobody watching.** The checks run as the OS user of the
     headless coordinator, with its files and its signing key (see
     [the limits](ci.md#checks-that-the-tool-runs), item 6). Give that user nothing else, and
     list the files that the checks run in `protected_paths`: a PR that changes one is left
     for the owner, and no check runs for it.
6. **What the headless coordinator does with the rest.** `tick` does nothing on the board for
   a ticket it leaves. It writes one line per ticket to its own log ("left for the interactive
   coordinator: <ticket>, <reason>") and posts no message. The `watch` and `poll` cycle of a
   coordinator is different: it runs the coordinator duties, in a headless dir too. It returns
   the tickets of a dead agent to Backlog and posts signed `info` notes (for example the notice
   to the owner). This is wanted. It is a board write that the scope allows.
7. **The headless dir has its own event cursor.** The `Cursor` field of the agent item
   is shared by every config dir of one agent name. If the interactive session and the
   headless loop moved it both, whoever polled first would consume the events of the other:
   a `done` could wait unverified, and a question that the headless loop may not answer could
   never reach the interactive session. In a headless config dir `poll` and `watch` therefore
   read and move a cursor in the local state cache (`local_cursor`, one per config dir) and
   NEVER write the board field. The first headless cycle without a local cursor starts from
   the value of the board field (read, not written), so no old history arrives. The only
   code that writes the board `Cursor` field is the cycle of `poll` and `watch` (in a
   non-headless dir) and the first `join` (a field that is empty; `join` is refused
   headless). The heartbeat writes only `Heartbeat`. `--reset-cursor` resets the local
   cursor. An interactive config dir behaves as before. A question stays with the
   interactive session: the headless cycle may print it, but it does not consume it.
8. **`tick`: the work that needs no event.** `crewboard tick` reads the tickets in
   Review. For each ticket whose Gate is CI only (item 5) it runs `verify` unless a verify result
   for the latest `done` exists, and then `merge` with the checks of 4. It has no check of
   its own: AUTHORITY.md, evidence, CI and the Gate rule are those of `verify` and `merge`. A
   ticket that is not CI only, has no `done`, has a `done` with a `protected-paths:` line (a PR
   that changes `AUTHORITY.md`, `.crewboard.toml`, `.github/workflows/**`, `.github/actions/**`,
   `.github/CODEOWNERS`, `CODEOWNERS`, `docs/CODEOWNERS` or a glob of `protected_paths`: the owner merges it,
   [Authority](board.md#authority)), or whose
   merge is refused gets one line,
   `left for the interactive coordinator: <repo>#<n>, <reason>`, and nothing is written for
   it. A lost event is therefore recovered on the next tick. The tick is idempotent: a merged
   ticket is Done; a verified ticket is not verified again; a refused merge writes nothing.
   One exception is the request that `merge` sends to the dev for a conflict: the tick sends
   it once per `done` (`tick_requested` in the state cache). `tick` is refused with
   `tick-headless-only` in a config dir that is not headless: it merges without a model that
   reads the Gate, which only a headless dir may do. It finds the Review tickets on
   the board itself on every run: one item listing, then the items by id, filtered by
   Status Review. It also looks at the tickets in Review, In Progress, Blocked and Ready whose issue is closed (the issue state comes with the items, no extra read):
   when the owner merged the PR by hand, it closes the ticket with the same checks as
   `close` ([Close after an owner merge](tickets.md#close-after-an-owner-merge)).
   That close is a fact, not a judgment, so it needs no Gate rule. The ticket index of the state cache never decides which tickets exist; it
   only maps an item to its issue, and an item it does not know is resolved by a full scan
   (the signed info comments). It needs the coordinator role. The tick also reads the Done
   tickets that its cache knows, for the owner-route safety net
   ([Workflow files: the owner route](tickets.md#workflow-files-the-owner-route), item 8). A
   finding gets one line, `left for the interactive coordinator: <repo>#<n>, ...`; the tick
   does not reopen, ask or write (the reopen needs a question to the owner, which is not
   in the headless verdict table).

   The tick also tells the assignee of an In Progress ticket, by one `info`, that the PR of its
   latest `done` was merged ([A PR merged while the dev works](tickets.md#a-pr-merged-while-the-dev-works)).
   That is a fact, too. It costs one PR read per tick for each In Progress ticket that has a
   PR in its `Evidence` field, until the dev is told.

   The tick also posts ONE `info` to the owner on a Review ticket that is in Review for longer than
   the `review` limit while the coordinator heartbeat is late
   ([Stale work](events.md#stale-work), item 9). That makes a stall visible when the
   interactive session is away. It changes no Status and refuses nothing. The signal is the
   `Heartbeat` of the coordinator item; a headless loop that writes that heartbeat itself
   hides the stall.

   The tick also posts the one `info` of a Review ticket whose PR was merged
   without a passed `verify` ([A PR merged without a passed verify](tickets.md#a-pr-merged-without-a-passed-verify)).
   That is a fact, not a judgment, so it needs no Gate rule. It costs one PR read for
   each Review ticket that has a `done`, and no read for a ticket that `close` ends
   (the PR is read already).

   The tick's `verify` follows the [round limit](tickets.md#round-limit) of `AUTHORITY.md`
   (`max_rounds`, default: off). A failed verify of the tick is a round, because CI or a check
   of the tool failed. At the limit the tick posts the signed `question` to the owner, sets
   the label `needs-owner` and moves the ticket to Blocked. That is a fact, not a judgment: the
   count comes from the signed reports, and the limit comes from the owner. The question is the
   one that `verify` writes; a question that a coordinator types (`ask`) stays outside the
   headless scope. After the Status write the ticket is not in Review, and the next tick does
   nothing with it. When a write of the stop failed, the ticket is still in Review: the next
   tick finishes the stop before it reads the Gate. It posts no second report and does the
   writes that are missing, also while the question of the stop is open. That writes no
   judgment, because the judgment is in the log. The tick uses an entry that only finishes a
   stop: when the stop ended meanwhile, it writes nothing and prints one `left` line. It never
   reaches the checks of `verify`, so it cannot pass a ticket outside the Gate rules of 8.
   8a. **The daily digest.** With `daily_digest: true` in `AUTHORITY.md` (default: off), the
   first tick of a UTC day posts one project status update
   ([Status updates](board.md#status-updates)). No model writes it. Its text has the facts that
   the tick read anyway: the tickets by Status, the agents that are late and those that are
   dead, and the Review tickets that this tick left for a person, each with the reason of its
   line (at most 15 lines, each cut at 140 characters). Its health value follows a fixed rule:
   `OFF_TRACK` when an agent is dead; `AT_RISK` when an agent is late or a ticket waits for a
   person; else `ON_TRACK`. The tick notes the day in its state cache before the post: it looks
   at the key once per day, so a change of the key takes effect on the next day, and a post
   that fails is one line of the log ("digest: not posted today: ...") and is not tried again
   that day. A status update that a coordinator types is still outside the headless scope: its
   health value is a judgment.
9. **The source of the scope.** The scope is on when an agent config of the config dir has
   `headless = true` (item 1, a guardrail). Every rule of this section (verdicts, `merge`,
   own cursor, `tick`) follows from it. `crewboard doctor` says whether the key is on.
   `start`, `resume` and `doctor` print a note when `/etc/crewboard/scope.d/` holds any
   `*.toml` file: older versions read such a file as a system scope file, and this version
   does not. The note says to set `headless = true` in the agent config and delete the files.

   The headless user holds a GitHub token and a signing key. A process of that user that runs
   its own code (not `crewboard`) can call the GitHub API directly, or sign messages with the
   key, and the key does not limit that. Limit the token itself: an agent account (the shared
   `crew_login` or an own one) with write access only to the initiative repos, branch protection on the main
   branch that requires the CI checks, and no admin rights. See
   [Headless setup](../../guides/headless-setup.md).

## Unattended coordinator

The key `unattended_coordinator` in `AUTHORITY.md` has the values `off` (default), `verify` and
`verify-and-merge`. The key `unattended_budget: <N>` is a number of verdicts per UTC day
(1 to 50, default 10). crewboard reads both keys from the default branch only. The keys are
reserved for a coordinator that judges with nobody watching.

The unattended coordinator is every `crewboard` command that runs in a headless config dir
(`headless = true`, [Headless coordinator](#headless-coordinator) item 1), except the `tick`
itself. No flag and no environment variable makes a process the unattended coordinator or
frees it: the key of the config dir decides. crewboard starts no model; the owner decides what
runs in the headless config dir.

With the mode `off`, a headless config dir makes no judgment. With `verify`, it may make the
judgments of [the scope](#the-scope-of-the-unattended-coordinator) below, inside a budget. The
value `verify-and-merge` does what `verify` does: the `merge` of a headless config dir is for a
ticket whose Gate is `ci` only, in every mode. An interactive config dir is not restricted and
not counted, whatever the keys say.

With a value that is not `off`, the headless `tick` prints two more kinds of line after its
other lines:

- one line `needs judgment: <repo>#<n>, <reason>` for each ticket on the judgment list;
- one line `unattended coordinator: mode <mode>; verdicts <n>/<budget> today`.

The tick writes nothing to GitHub for these lines. Its `left for the interactive coordinator`
lines stay. With the key absent or `off`, the tick prints none of these lines, makes no request
for them and keeps nothing about them in its state. When `AUTHORITY.md` cannot be read or its
front matter does not parse, the tick takes the mode as `off`; `verify` and `merge` refuse such
a file themselves.

**The list gives no right.** It is a view for a reader. A command that writes reads the ticket
again and decides from the signed log, so a ticket on the list can be refused there
([the scope](#the-scope-of-the-unattended-coordinator)).

**What the tick reads for a ticket.** The Status of the board says which rule to try; a board
field is not signed. Everything else comes from the verified messages of the ticket:

- For a Review ticket the tick has the issue, its labels and its comments.
- For a Blocked ticket, a Ready ticket without an Agent and a Backlog ticket without an Agent
  that has a Gate, the tick reads the comments. It does not read the issue, so it does not know
  the labels of such a ticket.
- The assignee is the agent of the signed log: the agent of the latest signed assignment, until
  a signed reassign. When the board `Agent` names another agent, or names one while the log has
  none, the ticket is not on the list, and the tick prints no line for that (`audit` and
  `status` report such a board field). An empty board `Agent` hides nothing: the questions and
  the `done` of the signed assignee still count.
- The tick puts no ticket on the list from board fields alone.

**Never on the list.** One of these facts keeps a ticket off the list, for every rule below:

- a question to the owner is open, or the [round limit](tickets.md#round-limit) stopped the
  ticket. Both come from the signed log. For a Review ticket the label `needs-owner` of the
  issue has the same effect;
- the latest `done` of the signed assignee is on the owner route (a line `owner-route:` or
  `protected-paths:`); for a ticket without an assignee, the latest `done` of the log;
- the ticket has 2 unattended verdicts (see the count below);
- the issue is closed;
- the board `Agent` is not the agent of the signed log.

Three more facts keep a ticket in Review off the list, because they are about the verdict on a
`done`: the ticket has no signed gate, no assignee or no `done` with evidence; the evidence is
a local gate and the Gate names no checks, or the ticket declares `CI: none`; the latest `done`
has a verify result.

**The judgment list.** A ticket that passed the facts above is on the list when one of these
rules holds. The reason of the line is the name in quotes.

1. "prose Gate": the ticket is in Review, and its signed Gate is prose (not `ci`, not
   `checks: ...`). Its latest `done` then has CI evidence and no verify result.
2. "question": the ticket is in Review or Blocked, and its assignee sent a `question` to the
   coordinator. No `answer` of a coordinator and no owner comment came after it.
3. "blocked": the ticket is Blocked, and the latest block is a plain `blocked` message of its
   assignee: no park, no question to the owner. No answer came after it.
4. "ready, no agent": the ticket is in Ready, the board has no Agent, and the signed log has no
   assignee: a reassign came after the last assignment. The signed log must allow the Status
   Ready. A Status that somebody set on the board alone puts no ticket on the list.
5. "returned": the ticket is in Backlog, has no Agent and has a Gate, and the last assignment
   or reassign of a coordinator in its signed log is a reassign.

For the rules 4 and 5 the signed log must hold an assignment of a coordinator with a `gate:`
line. The next assignment then copies the signed Gate, the CI line and the guard. A ticket that
was never assigned, and a ticket whose assignment comment is gone, are not on the list: their
next assignment signs these lines from the issue body, which is not signed, so a person reads
the body first. For these two rules a question to the owner of any agent also keeps the ticket
off the list until an owner comment follows it: the agent that asked can be gone, and its
question still waits for the owner.

A Review ticket is on the list only when the tick wrote nothing for it in this run.

**The verdict count.** An unattended verdict is a verify result of a coordinator that has the
line `unattended: <n>/<budget>` ([Acts](messages.md#acts)). Only a judgment of the unattended
coordinator gets that line ([the stamp](#the-scope-of-the-unattended-coordinator)). The count of
the day comes from the signed messages:

- The tick reads the comment feed of each repo of the initiative from 00:00 UTC, and it takes
  the verified messages of coordinators that GitHub created at or after that time. A message
  that was edited is not verified.
- The count is the number of verify results among them that have the line.
- The count is never below the highest `n` of a valid line in these messages. So the deletion
  of a stamped message does not lower the count while a later stamped message is left.
- The signed log alone does not protect the count when the latest stamped message is deleted
  or edited. Two values of the local state of the config dir do, on the host that has them.
  The tick and each judgment remember the count of each repo for the day and never go below
  what they read before. A second value is a floor: a judgment notes the count with its own
  message in the state before it posts. A host without that state counts what the log has
  left.
- A comment of the owner or a message of a dev agent with such a line counts for nothing.

When the tick cannot read the feed, it does not print a count of 0. It prints one line
`unattended coordinator: mode <mode>; the verdicts of today could not be read, so no ticket is
listed: ...`, it lists no ticket, and it reads no ticket for the list.

The count of one ticket is the number of unattended verdicts in its signed log. It starts
again after an owner comment, and after an assignment of a coordinator that has no
`unattended:` line. A reassign does not start it again: the duty for a dead agent posts one
with nobody watching. At 2 the ticket is off the list. The local state holds a floor for this
count too: a judgment notes the count of the ticket before it posts a verdict, with the highest
comment id of the ticket at that time. A verdict that somebody deletes or edits later stays in
the floor. The floor ends when a comment with a higher id starts the count again.

**Read cost.** With a mode that is not `off`, a tick makes these requests in addition:

- one read of the comment feed of the day per repo. The tick sends the ETag of its last read,
  so a feed that did not change is a conditional request (a 304). A feed of more than one page
  costs one request per page;
- one read of the comments for each Blocked ticket and for each Ready ticket without an Agent;
- one read of the comments for each Backlog ticket that has a Gate and no Agent. When the
  signed log says that no reassign returned the ticket, the tick notes the `updatedAt` of the
  board item and does not read the ticket again while the item is unchanged. A ticket that is
  on the list as "returned" is read at every tick;
- one scan of the open tickets when a board item of the list is new and the state cache does
  not know its ticket. A Blocked or Ready item that a scan does not find gets the line
  `left for the interactive coordinator: <item id>, a board item in <Status> has no signed
  info comment with its ticket`.

A read for the list that fails does not fail the tick. The tick prints one line
`unattended coordinator: <what> could not be read for the list: ...`, leaves that ticket off
the list of this run, prints its other lines and ends as it would without the failure.

### The scope of the unattended coordinator

A judgment is one of the commands of this table, in a headless config dir. Any other command
has the verdict of the [headless scope](#headless-coordinator).

| Command in a headless config dir | Mode `off` | Mode `verify` or `verify-and-merge` |
|---|---|---|
| `verify` (no `--body`) of a ticket whose signed Gate is `ci` and that does not declare `CI: none` | allowed, not stamped, not counted | the same |
| `verify --body` of such a ticket | `headless-scope` | the same |
| `verify --body` of any other ticket; `verify --fail --body` | `unattended-off` | allowed for a ticket that is "prose Gate" on the judgment list; a verdict. Without `--body`: `headless-scope` |
| `verify --run`, `verify --local-run`, `verify --run-check` | `headless-scope`, no request | the same |
| `merge` | a ticket whose Gate is `ci` only | the same |
| `say --kind answer --to <agent>` | `unattended-off` | allowed when `<agent>` is the signed assignee |
| `unblock` | `unattended-off` | allowed for a Blocked ticket that is "blocked" or "question" on the judgment list |
| `assign <n> --to <agent>` | `unattended-off` | allowed for a ticket that is "ready, no agent" or "returned" on the judgment list |
| `assign` with `--order` or `--override-capabilities` | `headless-scope`, no request | the same |
| `ask` | `unattended-off` | allowed for an open ticket that has no open question to the owner |
| `say`, `unblock`, `ask` with `--body-file <path>` | `headless-scope`, no request | the same |

The tick's own `verify` of a `ci` Gate or a `checks:` Gate, and its `merge`, are no judgments:
they are not stamped and not counted.

**Each judgment decides again.** The command reads the ticket and its comments, and it does not
use the list of a tick. These checks come first, and they refuse before any write:

1. The mode. With `off`, or with an `AUTHORITY.md` that cannot be read, the refusal is
   `unattended-off`. The command reads no count.
2. The budget of the day (below). A count that cannot be read refuses the command with
   `unattended-budget`: the command never takes the count as 0.
3. The ticket. The facts under "Never on the list" hold for `verify`, the answer, `unblock`
   and `assign`: an open question to the owner, a stop at the round limit, the label
   `needs-owner`, a `done` on the owner route, a closed issue, and a board `Agent` that is not
   the agent of the signed log. The refusal is `headless-scope`. A ticket with 2 unattended
   verdicts is refused with `unattended-budget`.
4. The board Status. When it contradicts the signed log, the refusal is `headless-scope`. The
   signed log allows Review after a `done` with evidence of the assignee, Blocked after its
   `blocked` message, and Ready or Backlog after a reassign.
5. The rule of the command, from the table.

**`verify`.** A verdict needs `--body`: the text says what was checked and what was found, and
the owner reads it as the reason. `verify` keeps every check of an attended `verify`. It reads
the files of the PR again, so a PR that changes a protected path is refused with
`protected-path-owner-merge`, also when the `done` names no such path. A ticket whose Gate is
`checks: ...`, a ticket without a signed Gate and a ticket that declares `CI: none` get no
unattended verdict (`headless-scope`).

**`assign`.** The signed log has no assignee, whatever the board `Agent` says. A signed message
of a coordinator names the board item of the ticket; the command puts no issue on the board.
The signed log holds an earlier assignment with a `gate:` line, and the new assignment copies
its signed lines. So the unattended `assign` never signs a Gate, a guard or a CI line from the
issue body: a ticket that was never assigned is for a person. On a ticket without an assignee,
a question to the owner of any agent refuses the `assign` until an owner comment follows it.

**The owner keeps its questions.** `unblock` never withdraws a question to the owner and never
releases a stop at the round limit: such a ticket is refused (check 3). An answer of `say` or
`unblock` whose body has the line `withdraws: question` is refused with `headless-scope` on
every ticket. `ask` is refused on a ticket that has an open question to the owner already.

**The stamp.** Every signed message of a judgment gets a last body line
`unattended: <n>/<budget>`. `n` is the number of verdicts of the day, with this message when it
is a verify result. One place sends every signed message, and that place writes the line. In a
headless config dir, a text with a line that starts with `unattended:` is refused with
`headless-scope`: the body of `say`, `unblock`, `ask` and `verify`, and every message before it
is signed.

**The budget.** The refusal is `unattended-budget`, and nothing is written.

- A verdict is refused when the count of the day is `unattended_budget` or more.
- Every judgment is refused when the day has 30 stamped messages of coordinators. This number
  is fixed. It is the limit for the messages that are no verdicts.
- `ask` is refused when the day has 5 stamped questions of coordinators to the owner. This
  number is fixed.
- A verdict, an answer, `unblock` and `assign` are refused for a ticket that has 2 unattended
  verdicts.
- Each message is reserved before it is posted. The command takes the lock of the state file,
  reads the counts of the state again, refuses at a limit, and writes the counts with its own
  message: the verdicts, the stamped messages and the questions of the day, and for a verdict
  the count of the ticket. Then it posts. So two judgments that run at the same time in one
  config dir cannot both take the last verdict. When the lock or the write fails, the message
  is not posted. `assign` reserves its message before its first board write.
- A message that follows in the same command is counted and not refused: the question to the
  owner that a failed verify at the round limit posts after its result.
- Every other writer of the state file keeps these counts: a save takes the higher count of a
  day and the newer count of a ticket from the file.

Each judgment makes one more request than the attended command: the read of the comment feed of
the day, per repo (a conditional request when the feed did not change; one request per page).
The answer also reads the issue and the comments of its ticket.

**No heartbeat.** An attended `say` refreshes the heartbeat of its agent when one is due. In a
headless config dir it does not, so an answer of the unattended coordinator does not hide a
late interactive coordinator from the owner alert (item 8 of the headless coordinator).

**Limits.** These stay, and the owner who switches the mode on accepts them:

- The text of an answer, of an `unblock`, of a question and of a verdict is free text. It comes
  from the coordinator account, and a dev model reads it as the word of the coordinator. The
  scope limits which message is posted and how many, not what the text says.
- A question of `ask` stops the tick's own work on that ticket: `verify` and `merge` refuse a
  ticket with an open question to the owner until the owner answers. The cap of 5 per day
  limits how many tickets one day can stop.
- The floors are in the state of one config dir on one host. A host without that state, and a
  second headless config dir, count what the signed log has left. Two judgments in two config
  dirs can both take the last verdict of a day.
- What a headless config dir merges is still a ticket whose signed Gate is `ci`, with green
  counted CI. No command of the scope writes or changes a signed Gate. A person who assigns a
  ticket signs its Gate from the body, with the warning of `assign`.
- The scope limits what a process does through crewboard. The headless OS user holds the token
  and the signing key of the coordinator. Outside crewboard it can call the GitHub API and
  sign a message without the line. The token rights and the branch protection are the
  boundary, as for the headless scope (item 9).

## Board config refresh


`join` stores the board table of `.crewboard.toml` in the agent config. Without a refresh, a later
change of the file never reached an agent that had joined. Now:

1. **Refresh.** `start` and so `resume` read `.crewboard.toml` from the default branch of the main
   repo (one `GET contents`; the same function as `join`). If the file differs from the copy, the
   `board` table of the agent config is replaced and the output has one line:
   `board config updated from <repo>@<short blob sha>: stale, required_tools`. The line names each
   changed top-level key. A file that is equal to the copy writes nothing.
2. **Identity keys.** These keys are never changed by a refresh: `initiative`, `project_owner`,
   `project_number`, `owner_login`, `registry_issue`, `crew_login` and the first entry of `repos`
   (the main repo). They say which board this agent works on and whose messages it trusts. A
   difference is refused with rule `board-identity-changed` (exit 2) before `start` changes
   anything. The copy stays. A moved board is a deliberate `crewboard join --repo <repo>`. The other
   repos of the list are not identity: a repo that joins the initiative is a normal edit. Every
   other key (`stale`, `required_tools`, `smoke_cmd`, `heartbeat_interval`, `authority`) is applied.
3. **Write.** The agent config is written to a temp file next to it and renamed. Keys that are
   not the board table (`headless`, `[loop]`, `tools`) are kept as they are. The agent config also
   keeps `board_sha`, the blob SHA of the file that the copy came from (`join --repo` and a refresh
   write it).
4. **Errors.** A read that fails (404, 500, network, a file that does not parse) keeps the copy,
   prints one line `note: cannot read .crewboard.toml from <repo> (<reason>); the local copy of
   the board config stays in force`, and never stops `start`. The toolchain check of `start` uses
   the file just read, so `start` costs one read of the file, not two.
5. **`status`.** For a joined agent, `status` reads the file once and prints
   `board config differs from <repo> default branch; run crewboard resume (<keys>)` when the file
   differs from the copy (the same `board_sha` means equal; else the parsed values are compared).
   `--json` has `board_config_differs` (`true`, `false`, or `null` when there is no agent config or
   the read failed).
6. **Cost.** One contents read per `start` or `resume`; one per `status`; none elsewhere.
