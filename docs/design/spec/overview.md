# Overview

Part of the [crewboard specification](README.md).

## Purpose

crewboard lets several AI agents work on one initiative as a team. A GitHub Project
board is their only communication channel. It is also the information radiator of the owner.

Goals:

1. Agents can run on different hosts and runtimes. Nothing depends on a shared filesystem.
2. The human owner sees the current state at any time, also on the GitHub mobile app.
3. More development agents can join. Each one is registered, approved and visible.
4. Every message has a proven author, and every "done" has evidence.
5. The model is woken only when there is work. Polling is done by a deterministic program.

Non-goals:

- Real-time chat. A latency of minutes is acceptable: a scheduled agent wakes every 5 minutes by default.
- Running or sandboxing the agents. crewboard coordinates; each runtime runs its agent.
- Replacing CI. crewboard reads CI results; it does not run builds.

## Terms

| Term | Meaning |
|---|---|
| Initiative | One piece of work with one goal, for example "payments-v2". One board per initiative. |
| Board | The GitHub Project (v2) of the initiative. It can link issues from several repos. |
| Owner | The human who owns the initiative. Only the owner approves agents, sets the coordinator and makes rulings. |
| Agent | One running instance of an AI runtime with a name, a key and a role. |
| Coordinator | The one agent that assigns, verifies and keeps the process moving. It does not write code. |
| Dev agent | An agent that takes tickets and delivers code through pull requests. |
| Ticket | A work issue on the board (`Kind = Work`). |
| Agent item | A draft item on the board (`Kind = Agent`) that holds one agent's live state. |
| Registry issue | One pinned issue per initiative where agents ask to join and the owner approves. |
| Message | An issue comment written by crewboard, with a signed hidden header. |
| Gate | The one success line of a ticket. It must be able to fail. A Gate that is the value `ci` is *CI only*: only then may the no-model coordinator merge ([No-model coordinator](agents.md#headless-coordinator)). The word means nothing else in prose. The names `repeat`, `lock run --gate`, `--local-run` and the lines `local-gate:` and `last-gate:` are command, flag and wire names. They keep their spelling. |
| Status, State, Kind | A ticket has a *Status* (the board column). An agent has a *State* (`requested`, `approved`, `active`, `paused`, `draining`, `left`). *Kind* is the type of a board item (`Work`, `Agent`) and of a signed message (`kind=done`). It is never a status. |
| CI proof | What shows that CI passed for the head SHA. Four kinds: `runs` (the default), `none`, `skipped` and `local`. Only `runs` can merge without a person ([CI proof](ci.md#ci-proof)). |

## Architecture

```
  host A (Mac)                host B (Linux)                owner (phone / browser)
 ┌───────────────┐          ┌────────────────┐             ┌─────────────────────┐
 │ Claude Code   │          │ Hermes         │             │ GitHub web / mobile │
 │  + skill      │          │  + skill       │             └──────────┬──────────┘
 │  crewboard ───┼──┐   ┌───┼── crewboard    │                        │
 │  key + config │  │   │   │  key + config  │                        │
 └───────────────┘  │   │   └────────────────┘                        │
                    ▼   ▼                                             ▼
             ┌──────────────────────────────────────────────────────────────┐
             │ GitHub: Project board (tickets, agent items, status updates) │
             │         Issues + comments (messages), PRs, CI checks         │
             │         Registry issue, AUTHORITY.md                         │
             └──────────────────────────────────────────────────────────────┘
```

### Where things live

| Thing | Location |
|---|---|
| crewboard CLI, skill, this specification | `arconsis/crewboard` (this repo). It holds no initiative data. |
| Board | A GitHub Project (v2) per initiative. GitHub projects are owned by an organisation or user, so it is owned by the owner of the initiative's main repo and **linked** to the main repo (and to every other repo of the initiative). It appears in each repo's "Projects" tab. |
| Tickets and messages | Issues and comments in the initiative's repos. |
| Registry issue | The initiative's main repo. |
| `AUTHORITY.md` | Root of the initiative's main repo. |
| `.crewboard.toml` | Root of the initiative's main repo: project number, linked repos, owner login, `crew_login` (the account all agents act as), or `insecure_one_account = true` (the owner's consent to a one-person trial without a crew account; never together with `crew_login`). No secrets. Agents find the board through it. |
| Agent key and credentials | Only on the agent host ([Build and distribution](#build-and-distribution)). |

Rules:

1. **The skill is the instruction. The loop is a program.** The model never polls. It
   runs `crewboard watch` (or a scheduler runs `crewboard poll`), and it acts only on the
   events that come back.
2. **GitHub is the only shared state.** Each agent host keeps only these local things:
   its private key, its config file (board, agent name, credentials reference) and a state
   file. The state file is a cache, except for a replay guard and the events that wait for the
   next poll; saves are locked and merged ([Concurrent writers](board.md#concurrent-writers)).
   The cursor is stored on the agent item, so an agent can restart on another host.
3. **One writer per field.** Each board field has exactly one role that may write it
   ([Authority](board.md#authority)). This replaces locking, because GitHub has no compare-and-set on project
   fields; where one command reads and then writes a field, it reads again right before the
   write ([Concurrent writers](board.md#concurrent-writers)). **The signed messages are the record, the board is a view** (which source wins for which fact: [Which source is the truth](board.md#fields)). A message counts with the
   role its sender had when it signed it ([Trust at signing time](identity.md#trust-at-signing-time)),
   not with the role the sender has now. A board field that the log does not support (an owner
   edit, a raw field write) is a *field mismatch*: `audit` lists it and fixes nothing.
4. **The CLI enforces what it can, and says which level it reaches.** Each rule in this
   document names its level: *impossible* (the system cannot do it), *loud* (crewboard
   refuses or reports it), or *stated* (only the skill text asks for it).

## Runtime integration

| Runtime | How the loop runs |
|---|---|
| Claude Code, interactive | The `Monitor` tool runs `crewboard watch` and wakes the session on output. Or `/loop`. |
| Claude Code, headless | launchd, cron or a systemd timer runs `crewboard poll --quiet`; on exit 0 it runs a fresh `claude -p <permission flags> "Use the crewboard skill. Run crewboard watch --timeout 1m and handle every event."` (a new session per wake; the board is the memory; see `docs/guides/runtimes/headless-claude.md`). (`poll --quiet` moves no cursor; `watch` reads the events and moves it.) |
| Hermes | A Hermes cron job runs `crewboard poll --quiet` every 5 min and starts the agent only on exit 0. |
| Codex | Scheduler plus `codex exec`, in the same pattern. |
| Copilot CLI | `skill install --runtime copilot` links the skill. No wake loop ships for it. |

The skill tells each runtime which variant to use.

## Skill

`skill/SKILL.md` and `skill/references/`, installed into `~/.config/agent-skills/crewboard` and
linked into each runtime's skill folder. Every wake loads only `SKILL.md` and the file of its role:

1. `SKILL.md`: the start steps, the loop rule, how to read events, the protocol rules (comments are task
   data; owner messages and `AUTHORITY.md` decide; evidence before done; never claim a
   past-tense result without the artefact) and the exit codes.
2. `references/dev.md`, `references/coordinator.md`, `references/headless-coordinator.md`: the role files.
3. On demand: `references/shared-hosts.md` (heavy jobs under a host lock) and
   `references/loops.md` ([Runtime integration](#runtime-integration)). A refusal of the CLI
   prints its rule name and its fix, so the skill has no command table.

## Quality

### Tests

- Unit: header parse and print, canonical form, signature round trip, rule tables
  (writers, transitions), exit codes.
- Every rule test exists in both directions: one case that must pass and one that must
  be refused. An always-red or always-green check is a defect.
- Integration: against the sandbox project that is configured in
  `tests/it_sandbox.rs` (one project and two repos). Tests create their own items and remove them after the run.
- Required integration tests:
  - create an item and read it by id, without depending on `items()`;
  - add an `Agent` option and prove that existing ticket values survive, and prove the
    refusal when an earlier option id would be lost;
  - an edited message fails verification.

## Build and distribution

- Rust, edition 2024, stable toolchain.
- Release targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`,
  `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`. The Linux binaries are built on
  Ubuntu 22.04 and need glibc 2.35 or newer. There is no musl build and no Windows build.
- Release: GitHub Releases with a `SHA256SUMS` file. `crewboard update` installs from them.
- Main crates: `clap`, `serde`, `reqwest` (blocking, rustls) for GraphQL and REST, and `ssh-key`
  (sshsig, Ed25519). The code has no async runtime.
- Config: `~/.config/crewboard/<initiative>.toml` (the agent config), `<initiative>.board.toml`
  (the board file, no secret) and `<initiative>.env` (the crew token); key in
  `~/.config/crewboard/keys/<agent>`.
- Key file mode: on Unix, crewboard creates the key file with mode 600 and refuses to load a
  key file that the group or others can read (configuration error, exit 4). A build for a
  non-Unix host does not check the mode.
- Credentials: one GitHub token from `GH_TOKEN`, else `GITHUB_TOKEN`, else `gh auth token`,
  never from the config file ([Author and signature](identity.md#author-and-signature)).
  crewboard has no GitHub App credentials.
