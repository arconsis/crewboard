# Agent profiles and capability matching

Part of the [crewboard specification](README.md).

Purpose: the coordinator picks an agent for a
ticket by what the agent can do, not by memory. An agent has **facts** (detected, never
typed) and a **declared profile** (tags, a background text, a tier). A ticket can state what
it **needs**. `assign` refuses a mismatch (*loud*) unless the coordinator overrides it with a
recorded reason.

## Facts

`join` and every `start` detect these on the host of the agent, and write them again. A fact
is never edited by hand: the next `start` replaces any fact that someone typed into the field
(*impossible* to keep, *loud* in `agent profile show` for unreadable tokens).

| Fact | Token | Source |
|---|---|---|
| Architecture | `arch:x86_64` or `arch:arm64` (`aarch64` and `amd64` are normalised) | `uname -m`, else the build value |
| OS and major version | `os:macos-15`, `os:linux-ubuntu-24` | `sw_vers`, `/etc/os-release` |
| CPU cores | `cpu:10` | the number of logical CPUs |
| RAM | `ram:32g` (GiB, rounded) | `sysctl hw.memsize`, `/proc/meminfo` |
| Tool | `tool:node` (the tag says "installed"; versions are in the item body) | `<tool> --version` (5 s limit; `xcodebuild -version`, `go version`, `java -version`) |

The tool list has three sources. The default is a list of common tools of many stacks, not the
build tools of one project: `git, docker, make, node, npm, pnpm, yarn, bun, deno, python3, uv,
go, cargo, rustc, dotnet, xcodebuild`. `tools = [...]` in the agent config
(`<config-dir>/<initiative>.toml`) or `join --tool` (repeatable) replaces the default for one
agent. `detect_tools = ["java", "gradle"]` in `.crewboard.toml` adds names for every agent of the
board, so the owner sets them once; a bad name is a config error that names the file and the
entry. The final list is the default (or the agent list) plus the board list, each name once.
The default leaves out tools that can open an install dialog on a host without them (`java`,
`swift` on macOS) and tools that start a JVM (`gradle`, `mvn`); put them in `detect_tools` when
the stack needs them. The list only probes: a tool that is not installed gets no tag and is no
error (`required_tools` is the list that blocks `start`). The tools are probed in parallel, so the
probe takes as long as the slowest tool. A corepack shim (`yarn`, `pnpm`) gets
`COREPACK_ENABLE_NETWORK=0`, so a probe never downloads a package manager.
A tool that is not installed, fails, or does not answer in 5 seconds gets no tag. A fact that
the host cannot tell (for example RAM on a host without `sysctl` and `/proc/meminfo`) gets no tag.

## Declared profile

| Part | Format | Set with |
|---|---|---|
| Capability tags | `[a-z0-9-]`, 1 to 32 characters, no `-` at the start or end, at most 24 per agent, for example `x86-builds`, `e2e`, `measurements`, `gpu`, `ui-review` | `agent profile set --tag t`, `--untag t` |
| Tier (optional) | written like a tag; `low`, `mid`, `high` sort cheapest first | `agent profile set --tier t`, `--clear-tier` |
| Background text | prose, at most 4000 characters | `agent profile set --about-file f` (`-` reads standard input; an empty file clears it) |

A tag with a colon is a fact and is refused as a tag (`profile-tag-invalid`).

## Storage

- Field `Capabilities` of the agent item: one line of space-separated tokens: the declared
  tags, `tier:<t>`, and the fact tokens of [Facts](#facts). Example:
  `e2e x86-builds tier:high arch:x86_64 os:macos-15 cpu:10 ram:32g tool:git tool:node`.
- Body of the agent's draft item (markdown): the sections `Profile` (tier, tags), `Background`
  (the prose) and `Facts` (the full facts with tool versions).
- Limit: the field holds at most 1024 bytes ([Fields](board.md#fields)). Declared tags, the tier, `arch:` and `os:`
  always stay; if they alone do not fit, the write is refused (`capabilities-too-long`).
  `cpu:` and `ram:` follow, then the `tool:` tokens. Tools are cut from the end, and the cut is
  marked with `tools-cut:<n>`, so a missing tool tag is never mistaken for a missing tool.
- `init` creates the field on an existing board (idempotent, [Fields](board.md#fields) option rules unchanged). A
  board without the field still works: `join` and `start` print a warning and write nothing;
  `agent profile set` refuses with the command to run. The Crew view of a new board shows the column;
  an existing view keeps its columns (REST cannot change them).

## Who may change a profile, and the history

| Actor | May change |
|---|---|
| The agent itself | its own tags, tier and background text (`agent profile set`) |
| The owner | the tags, tier and background text of any approved agent (`agent profile set --agent <name>`, the owner's own `gh` login) |
| Another agent, the coordinator | nothing of another agent's profile (`owner-command-not-owner`; the field rule `field-wrong-writer`) |

Every change is recorded before it is written (*loud*), so the history is auditable:

- by the agent: a signed `kind=info` message in the registry issue (to the coordinator, or to
  the owner if the agent is the coordinator). The body starts with `Profile change.` and lists
  the added and removed tags, the new tier, whether the text changed, and all tags after the change;
- by the owner: an owner message in the registry issue, `/crew profile <name> [+tag] [-tag]
  [tier=<t>|tier=none] [about=set|about=clear]`. With `about=set` the lines after the first line are
  the new text. Agents see it as a registry event.

`agent profile show` lists the valid changes of an agent in order. A forged, edited or unsigned
message is not part of the history ([Signature](messages.md#signature)). A change that is already in place posts nothing.
In phase 1, an agent on a host with the owner's token can act as the owner ([Phases](identity.md#phases)); the rule
above holds in full from phase 1b (`crew_login`) on.

## Requirements and matching

A ticket states its requirements in two places; both count, as a union:

- a body line `Needs: x86_64, x86-builds` (also `**Needs:**`, a list item, any letter case;
  several lines are merged; separators are commas and spaces);
- labels `needs:<requirement>`, for example `needs:gpu` or `needs:tool:docker`.

A **requirement** is one of:

| Form | Met when |
|---|---|
| a bare architecture: `x86_64`, `amd64`, `arm64`, `aarch64` | the agent has `arch:` with the same normalised value |
| `arch:<a>`, `cpu:<n>`, `ram:<n>g`, `tool:<name>` | the agent has exactly this fact token |
| `os:<name>` or `os:<name>-<major>` | the agent has `os:<name>` or an `os:` token that continues the value after a `-` (so `os:macos` meets `os:macos-15`, `os:macos-15` needs macOS 15, `os:mac` meets nothing) |
| `tier:<t>` | the declared tier is `<t>` |
| any other word (a declared tag) | the agent declared exactly this tag |

Everything is lower case. A bare tool name such as `go` is **not** a requirement (a tag called
`go` would have to be declared); write `tool:go`. A requirement that does not parse
refuses `assign` with `needs-invalid`: a requirement that is dropped silently would let an
agent without it take the ticket (*loud*).

`assign` (and `reassign --to`) refuses with **exit 2, rule `capability-mismatch`**, and names every
missing requirement, when the agent does not meet all requirements. The check runs before anything
is written to the board. If the tool list of the agent was cut ([Storage](#storage)) and a `tool:` requirement is
missing, the message says so.

The coordinator can override only with `--override-capabilities --why "..."`. The reason is
required (`capability-override-needs-reason`, also when nothing is missing, so the flag never
works by accident). An override that was used is recorded in the signed assignment message as
`capability-override: <missing>` and `override-reason: <text>`. For `reassign --to`, the reason of the
reassign is the reason of the override; `--derived` cannot override.

## `crewboard status --agents`

`crewboard status --agents [--can <requirement>...] [--json]` lists the agents
that are `active` and not stale or dead, and that meet every `--can` requirement (same forms and rules as [Requirements and matching](#requirements-and-matching)).
Columns: name, role, state, heartbeat age, load (the number of tickets of the agent that are not
`Done`), tier, declared tags. Order: fewest open tickets first, then the cheaper tier (`low`,
`mid`, `high`, any other name by name, no tier last), then the name. Agents that are not listed are
named with the reason (`state paused`, `liveness late`, `lacks gpu`). Coordinators are listed
with their role; they take no tickets ([Coordinator role](board.md#coordinator-role)).

## Tags in `status`

`status` shows the declared tags and the tier of each agent (`tags=...`), and `--json` adds
`tags`, `tier` and `capabilities`.

## Rule levels

| Rule | Level |
|---|---|
| Facts are detected, a hand edit does not survive `start` | *impossible* to keep |
| Tag and tier syntax, 1024-byte limit, needs syntax | *loud* (`profile-tag-invalid`, `capabilities-too-long`, `needs-invalid`) |
| `assign` refuses a capability mismatch | *loud* (`capability-mismatch`, exit 2) |
| An override needs a reason and is recorded | *loud* (`capability-override-needs-reason`) |
| Only the agent itself or the owner changes a profile | *loud* (`owner-command-not-owner`, `field-wrong-writer`) |
| The coordinator looks at `status --agents --can ...` before it assigns | *stated* (skill) |
