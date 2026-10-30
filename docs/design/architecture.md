# Architecture

This page maps the code in `src/` and shows how data moves in one cycle. The behaviour is defined in
the [specification](spec/README.md). The module comments in each file name the spec section they
implement, in the short form `spec/<file>#<anchor>` (for `docs/design/spec/<file>.md#<anchor>`).

crewboard is one binary (`src/main.rs`) on top of a library (`src/lib.rs`). The library holds all
modules, so the integration tests in `tests/` can reach them. Only `cli` and `exit` (used by the
binary) and the modules that the integration tests need are `pub`, and those are `#[doc(hidden)]`:
they are not a supported API. The others are `pub(crate)`, so rustc reports dead code in them.

## Layers

```text
  cli/ (command tree in args.rs, one module per command group, scope.rs for the headless verdict)
     |
  wake/ (the wake flow; calls the commands of ops)
     |
  ops/*.rs (one file or family per command; reads, decides, writes)
     |      \
     |    lock/ (host lock; ops and wake import it), repeat/ (`repeat` verdicts; pure, ops imports it)
     |
  ctx/ (Global, Ctx, AgentCtx, World, token checks; ops, wake and unread import it)
     |
  services/*.rs (read from GitHub, then judge)
     |            \
     |         rules/*.rs, domain/*.rs, ticket/, events.rs, registry.rs, late/ ...  (pure logic)
     |
  board.rs, feed.rs, labels.rs, builtin_workflows.rs  (board and comment IO)
  items.rs, comments.rs  (the types of the answers; no client)
     |
  github/ (GraphQL and REST client, `projects.rs` for the raw calls of `init`)
```

`ops/` also imports `rules/`, `ticket/`, `board.rs` and `feed.rs` directly, not only through
`services/`. `services/` imports the GitHub layer, the pure modules, `config`, `state` and `base`,
and no command. Only `ops/` and `ctx/` import `services/` (`ctx/` for `Ctx::reader`). `ops/`
imports neither `cli/` nor `wake/`. `tests/layering.rs` fails when:

- a module imports another one that is not on its list of allowed imports, or an allowed import is gone;
- a service imports upward, or anything but `ops/` and `ctx/` imports a service;
- `domain/` imports anything but `base`, `rules` and `authority`;
- the command layer (`cli`, `wake`, `ops`, `ctx`, `headless`, `unread`) or the module graph has an import cycle;
- the core modules (`agent_env` up to `ticket`) import upward;
- the files of `ops/` or of `lock/` have an import cycle, or one of them uses `super::super::`.

The rule of the design: modules that decide are pure. They take facts and return a verdict. The
modules that call GitHub are `github/`, `board.rs`, `feed.rs`, `labels.rs`,
`builtin_workflows.rs`, `services/`, `ctx/` and `ops/`. `domain/` imports no module that does
I/O. `tests/layering.rs` freezes the import graph.

## Module map

### Protocol

- `src/protocol/names.rs`: typed header fields (agent names, kinds, addressees, references, timestamps).
- `src/protocol/header.rs`: parse and print the hidden header, the visible line and the body.
- `src/protocol/canonical.rs`: the canonical form that is signed.
- `src/protocol/signature.rs`: Ed25519 `sshsig` signatures in namespace `crewboard`, made in-process.
- `src/protocol/interop_tests.rs`: proof that crewboard and `ssh-keygen` accept each other's signatures.
- `src/protocol/former.rs`: former names of the main repo with a cutoff, so old signatures still verify after a move.
- `src/protocol/replay.rs`: the replay window inside one issue.
- `src/base/ids.rs`: the catalog of refusal ids. A `Refusal` names its rule with a `RuleId` of this catalog, so a misspelt id does not compile. The [refusal ids](../reference/refusals.md) page is generated from it.
- `src/base.rs`: base types (`Role`, `Actor`, `Refusal`, `Verdict`, `GhError`, and `TicketRef`: one ticket as repo and number, printed `owner/repo#n`, compared with the repo in lower case), and `base/ignore.rs` with the named reasons to drop a result on purpose (`best_effort_output`, `best_effort_cleanup`, `child_may_have_ended`). Every module may import it. It imports nothing.
- `src/secrets.rs`: secret scan for every signed post and `ticket new`. It reports the kind and the line, never the secret.
- `src/clock.rs`: UTC time as `YYYY-MM-DDTHH:MM:SSZ`, without a time crate.

### Rules

- `src/rules/mod.rs`: the rules as data and pure functions. An answer is `Ok(())` or a refusal that names the rule.
- `src/rules/fields.rs`: the board fields as a type (`Field`, with the name on the board) and which role may write which field (one writer per field). `Board::write` in `src/board.rs` asks this table before every field write.
- `src/rules/status.rs`: ticket status transitions and their writers. `status/table.rs` is the test table of every move.
- `src/rules/reassign.rs`: the reasons of `reassign` and the status they lead to.
- `src/rules/protected.rs`: a PR that changes an authority file is merged by the owner only.
- `src/rules/text.rs`: text helpers (issue reference, `ci:` value).
- `src/rules/lifecycle.rs`: agent lifecycle, single coordinator, coordinator cannot take.
- `src/rules/evidence.rs`: what `done`, `verify` and `merge` need as evidence (PR, SHA, CI).
- `src/rules/limits.rs`: size limits for text fields and message bodies.
- `src/rules/options.rs`: single-select options, written only as a complete list that keeps every id.
- `src/authority.rs`: parse the `AUTHORITY.md` front matter. Unknown syntax is refused.
- `src/capabilities/mod.rs`, `src/capabilities/facts.rs`, `src/profile.rs`: agent profiles, detected host facts, the history of profile changes.

### Operations (`src/ops/`)

One file or family per command group. `src/ops/mod.rs` is the index. A file may import the
modules on the `ops` list of `tests/layering.rs`.

- Setup and join: `init.rs`, `init_ci.rs` (`init --with-ci`), `init_rule.rs` (the branch rule of `init`), `join.rs`, `approve.rs`, `start.rs`, `restart.rs` (the `resume` command), `selftest.rs`.
- Agent setup: `agent/` (`agent add`, `exec`, `env`, `with-env`, `shim`; `add.rs`, `coordinator.rs`, `install_wake.rs`, `wait.rs`, `scheduler.rs`). `install_wake.rs` imports no `wake`: the command layer passes the installer. `crew_token/` (`agent token`, `tty.rs` reads without echo).
- Messages: `say.rs` (signed post), `watch.rs` (`watch` and `poll`), `monitor.rs` (a silent loop around `watch`), `acted.rs` (hidden: the event receipts of a wake), `duties.rs` (the duties of each cycle: owner commands, draining, the dead-agent path).
- Cycle duties: `registry_post.rs` (two writes that the duties share), `log_check.rs` (board `Status` against the signed log), `needs_owner_sync.rs` (keeps the `needs-owner` label true).
- Tickets: `tickets/mod.rs` (the shared pieces) with one file per command or command group:
  `tickets/assign.rs`, `take.rs`, `done.rs`, `block.rs` (`block`, `unblock`, `ask`), `verify.rs`,
  `close.rs`, `reassign.rs`, `merge.rs`, `new.rs`, `order.rs`, `split.rs` and `tick.rs`. The shared
  reads and checks live in `pr_evidence.rs` (PR head, files and named CI runs), `local_gate.rs`
  and `in_progress.rs` (the busy and idle checks). `assign_checks.rs` (checks before an
  assignment) and `board_put.rs` (put an issue on the board) serve `assign`, `new`, `order` and
  `reassign`. `earlier_work.rs` finds the work that an earlier agent left. `pr_read.rs` reads a
  merged PR for `merged_pr.rs` (merged while the dev works), `merged_unverified.rs` (merged
  without `verify`) and `owner_merge.rs` (`close` without `--verified`). `owner_net.rs` and
  `owner_applied.rs` are the safety net of the owner route.
- Reading state: `status.rs`, `queue.rs` (`status --queue`), `agent_row.rs` (one row per agent), `pr_state.rs` (the PR column), `review_wait.rs` (the wait for `verify`), `behind.rs` (how far a PR head is behind its base), `mine.rs`, `agents.rs`, `audit.rs`.
- Late work: `ops/late.rs` (the GitHub reads, the `poll` duty and the `status` section for the waits of `src/late/mod.rs`).
- Board config refresh: `board_refresh.rs` (`start` re-reads `.crewboard.toml` from the default branch).
- Agent and crew admin: `lifecycle.rs` (`pause`, `resume`, `stop`), `force_stop.rs` (`stop --force`), `profile.rs`, `prune.rs`, `report_status.rs`.
- Local checks: `doctor.rs` (`doctor`: the headless scope and the branch protection), `repo_migration.rs` (`doctor --repo-move`, a read-only dry run).
- Host tools: `lock_cmd.rs` (`crewboard lock`: the environment and the board probe for `src/lock/`), `repeat_cmd.rs` (`crewboard repeat`: runs a command N times, `src/repeat/` judges), `skill_cmd.rs` (`skill install`, `skill path`), `update_cmd.rs` (`crewboard update`: replace the binary, then `skill install`).

### Host lock and repeat

- `src/lock/`: the host lock, a directory with an owner file that a dead agent cannot hold. It imports only `base`, `clock`, `fsutil` and `spawn`.
  - `mod.rs` (the lock), `run.rs` (the dispatch, `--pid runner`, `run`), `args.rs` (the command line, exit 64), `help.rs`, `log.rs` (log and answer lines).
  - `owner.rs` (the owner file, a public contract), `snapshot.rs` (all disk reads), `fsmeta.rs` (change time), `judge.rs` (is the lock stale; pure), `takeover.rs` (one atomic winner).
  - `procs.rs` (process facts behind a trait), `board.rs` (the board probe trait: owner dead or gone), `ci.rs` (CI first), `sys.rs` (signals, new session).
  - `detach.rs` (`run --detach`) and `result.rs` (the result file that `wake` reads).
- `src/repeat/` (`mod.rs`, `verdict.rs`, `evidence.rs`): the verdict of one run and the evidence block of `crewboard repeat`. Pure. It imports no crate module.

### Services (`src/services/`)

- `src/services/ticket_index.rs`: the one place that finds tickets. It lists the `crew:work` issues
  (open, newest closed, or all), takes the item id from the signed info comment, learns it into the
  local ticket index, and reads items by id. An id that GitHub cannot resolve any more is dropped and
  the read repeats. It never finds tickets from the item listing.
- `src/services/ticket_loader.rs`: the one way to load a ticket. `load_ticket` reads the issue, its
  valid signed messages and the board item by id. `read_messages` is the message half for a command
  that needs no item.

### GitHub

- `src/github/mod.rs`: the client for GraphQL and REST.
  - A GraphQL answer with HTTP 200 and an `errors` array is a failure.
  - A temporary conflict on a write is retried up to 3 times (`retry_conflict`).
  - A transient network failure is retried a bounded number of times (`retry_transient`).
  - Conditional requests use ETags. A 304 is a result, not an error.
  - The base URL is `https://api.github.com`. `CREWBOARD_API_BASE` may replace it only with an `http://`
    loopback host (`parse_api_base`), so the token never leaves the machine. `get` follows only absolute
    URLs under the base.
  - The token of an agent command comes from the env file of the agent, else `GH_TOKEN` or `GITHUB_TOKEN`, else `gh auth token` (`github/token.rs`, `resolve_token`). An owner command takes `--env-file`, else the owner's own `gh auth token` (`resolve_owner_token`, through `ctx::new_owner_client`). `Debug` hides it.
- `src/github/paging.rs`: bounded paging of REST lists, the one place that follows `Link: rel="next"`.
- `src/github/projects.rs`: the raw calls that `init` makes to find or create a project, link repos, create fields and views. `ops/init.rs` decides what is missing.
- `src/board.rs`: the project schema, item reads by id, field writes (every text write checks the size limit).
- `src/items.rs`: the type and parser of project items read by id. It imports only `base`.
- `src/feed.rs`: comment IO. The per-repo feed, one issue's comments, posting (`post_once_safely`) and deleting.
- `src/comments.rs`: the comment type as read from the REST feed.
- `src/spawn.rs`: start a child process and retry on "Text file busy".

### Events, state and liveness

- `src/events.rs`: `classify` turns one comment into deliver, security or ignore. Pure.
- `src/cursor.rs`: the watch cursor in the `Cursor` field of the agent item.
- `src/unread.rs`: the unread-message check of `say`, `done`, `block` and `ask`.
- `src/registry.rs`: the registry issue as the source of truth for who may speak.
- `src/late/liveness.rs`: `late` and `dead` are derived from the age of the `Heartbeat` field alone (no ping, no pong), never written.
- `src/ticket/`: ticket facts derived from the board and from signed comments. `messages.rs` (verified messages, questions, blocks, splits), `evidence.rs` (`done` evidence, signed gate and CI lines, verify result), `rows.rs` (creation `info`, board rows, events), `log.rs` (the status that the signed log shows) and `queue_order.rs` (the queue of an agent and the `Order` of a new ticket).
- `src/protocol/acts.rs`: what a message means: the one parser of message acts and the writers of their bodies.
- `src/domain/ci/evaluate.rs`: judge the CI facts of one commit from the answers that were read. Pure.
- `src/domain/ci/triggers.rs`: read the `on:` key of a workflow and judge whether workflows run for crew branches. Pure.
- `src/services/ci_evidence.rs`: `fetch_ci` reads the CI sources of a commit (one `CiQuery`) and hands them to `evaluate`.
- `src/services/ci_triggers.rs`: read the workflow files of a repo (`check_repos`, `judge_no_run`) and hand them to `triggers`.
- `src/domain/branch_guard.rs`, `src/services/branch_guard.rs`: is the default branch protected against a direct push of a crew account. The service reads it once per run, the domain judges it.
- `src/late/mod.rs`: pure rules for late work: the table of expected times, `scan()` (what waits too long) and `tells()` (who hears of it: dev, coordinator or owner). It also judges the heartbeat of agents. The reads sit behind a probe.
- `src/headless.rs`: the headless scope (rule, reasons, Gate checks); the verdict of each command is in `src/cli/scope.rs`. A scheduled coordinator may only `verify` and `merge` a CI-only ticket.
- `src/owner_cmd.rs`, `src/owner_route.rs`: the owner commands `/crew ...`, and the owner route for workflow files.
- `src/labels.rs`: the labels that make the board readable.
- `src/audit.rs`: what the signed comments show.

### Configuration and checks

- `src/config/` (`board.rs`, `agent.rs`, `dir.rs`, `keys.rs`): `.crewboard.toml` (board config), `<config-dir>/<initiative>.toml` (agent config) and the key folder.
- `src/agent_env.rs`: the env file of an agent (`agent.env`) and the crew token in it. It imports `base`, `error` and `fsutil`.
- `src/state.rs`: the local state file `<config-dir>/state/<initiative>.<agent>.json`. It imports `base`, `clock`, `comments`, `error`, `events` and `fsutil`.
- `src/fsutil.rs`: private-mode check, atomic writes, executable checks. It imports `base` and `clock`.
- `src/skill_embed.rs`: the skill files that `build.rs` embeds, with their SHA-256 and version line. It imports no crate module.
- `src/toolchain.rs`, `src/agent_gh.rs`: checks of `start` for build tools and for the identity of the agent's own `gh`.
- `src/error.rs`, `src/exit.rs`: one error type, mapped to the exit codes.
- `src/cli/`: `args.rs` (the clap types), `mod.rs` (`run`, the outcome and the routing of `dispatch`), `scope.rs` (the headless verdict of each command), `overview.rs` (the top-level help) and one module per command group (`crew.rs`, `dev.rs`, `plan.rs`, `review.rs`, `watch.rs`, `host.rs`, `agent.rs`).
- `src/ctx/`: the contexts of a command (`Global`, `Ctx`, `AgentCtx`, `World`); `ctx/token.rs` has the token checks (`judge_agent`, `judge_owner`, `require_*`). `authority_file.rs` reads `AUTHORITY.md`, and `registry_view.rs` has the registry reads that several commands share.

### Wake (`src/wake/`)

The unattended wake for a scheduler. It imports `agent_env`, `base`, `clock`, `config`, `ctx`, `error`, `fsutil`, `headless`, `lock`, `ops`, `protocol` and `spawn`, never `cli`.

- One run: `cmd.rs` (`crewboard wake`), `flow.rs` (the phases and what they share), `preflight.rs` (old version, stop file, net back-off), `precheck.rs` (one `poll --quiet` before the lock), `finish.rs` (the wake and its end), `tick.rs` (`--tick`, the headless coordinator without a model).
- The wake command: `job.rs` (options and argument vector), `adapters.rs` (one row per runtime), `runtime.rs` (process group, time budget, signals), `heartbeat.rs`, `procs.rs` (`stop_group`), `words.rs` (shell words), `describe_head.rs` (where the work is after a time-out).
- Events: `events.rs` (the text of the events file), `wake_events.rs` (the file, the receipts, the `.unhandled` copy), `receipts.rs`, `results.rs` (result files of detached jobs), `hook.rs` and `hook_cmd.rs` (`crewboard hook post-tool-use`).
- State and rules: `settings.rs` (flags and `Settings`), `policy.rs` (pure rules, time as an argument), `state.rs` (the state dir), `lockdir.rs` (the wake lock), `steps.rs` (the board calls behind a trait: in process or as a subprocess).
- `install/`: `wake install`, `uninstall` and `status`. `plan.rs` makes the plan, `apply.rs` writes it, `units.rs` has the unit texts, `sys.rs` is the host behind a trait, `foreign.rs` finds jobs under other names.

## Data flow of one cycle

### Message: `say` to `poll`

```text
agent A                                     GitHub                         agent B
-------                                     ------                         -------
say (ops/say.rs)
  secrets.rs scan, unread.rs check
  protocol/header.rs + canonical.rs
  protocol/signature.rs  -> sig
  feed.rs post_comment  ------------------> issue comment (header + body)
                                                                 poll / watch (ops/watch.rs)
                                                                   registry.rs: who may speak, which key
                                                                   feed.rs feed_since (ETag, cursor.rs)
                                                                   events.rs classify:
                                                                     verify signature, to=B, not seen
                                                                   deliver -> printed event
                                                                   cursor.rs moves after printing
```

A comment with a bad signature does not reach the agent as a message. `classify` returns a security
event instead.

### Ticket: `take` to `merge`

```text
coordinator: assign   -> Agent field + signed message          (ops/tickets/assign.rs)
dev:         take     -> Status In Progress                    rules/status.rs checks the transition
dev:         done     -> signed done with pr, sha, ci          rules/evidence.rs + domain/ci judge
coordinator: verify   -> signed verify for that head SHA       (no Status write on a pass)
coordinator: merge    -> PR merge call carries the SHA         (ops/tickets/merge.rs)
                         branch deleted, Status Done, signed message to the dev
```

Every step reads the facts from the board and the signed comments (`services/ticket_loader.rs`,
`ticket/`, `items.rs`), asks the pure rules, and only then writes. A merge is the one write
that cannot be undone, so it is the first write of `merge`.
