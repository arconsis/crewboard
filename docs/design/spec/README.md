# crewboard specification

The specification describes what the code of crewboard does now. It is the source of truth for
behaviour. It is split into one file per area. Each section has a name and a stable anchor.
Sections have no numbers, so a new section never renumbers the others.

The command list is not here. It is generated from `--help` in the
[CLI reference](../../reference/cli.md). [cli.md](cli.md) keeps only what `--help` does not say.

## Files

| File | What it specifies |
|---|---|
| [overview.md](overview.md) | Purpose, terms, architecture, runtime integration, the skill, quality, build and distribution. |
| [identity.md](identity.md) | Agent identity: the GitHub author, signing keys, phases, agent names, the identity of the agent's own `gh`. |
| [messages.md](messages.md) | The message protocol: format, signature, owner messages, untrusted input, secrets, `selftest`. |
| [board.md](board.md) | The board model (fields, views, status updates), the registry and the agent lifecycle, `AUTHORITY.md`. |
| [tickets.md](tickets.md) | Tickets: content, status flow and writers, evidence, verification, branches, queue and priority, labels, `ticket new`, `merge`, `ticket split`, the owner route for workflow files, the queue order. |
| [ci.md](ci.md) | CI: the four kinds of CI proof, triggers for crew branches, crewboard-only commits, which CI runs count. |
| [cli.md](cli.md) | The rules behind the [exit codes](../../reference/exit-codes.md), and what `status` shows. The commands are in the [CLI reference](../../reference/cli.md). |
| [events.md](events.md) | `watch` and `poll`: event semantics, polling cost, event checkpoints, unread messages, stale work and `ci-finished`. |
| [profiles.md](profiles.md) | Agent profiles and capability matching. |
| [agents.md](agents.md) | Agent operation: the toolchain check, unattended dev agents, the coordinator alert, `resume`, the headless coordinator, the board config refresh. |
| [security-details.md](security-details.md) | Detail behind the security guide: the signed data and how to check it by hand, edited owner comments, token handling, the trust roots, `selftest`, `audit` and `status`. |
| [host-lock.md](host-lock.md) | The host lock helper and repeated Gate runs. |

## How to refer to a section

In Markdown, link to the file and the anchor: `[Lifecycle](board.md#lifecycle)`, or from
outside this folder `docs/design/spec/board.md#lifecycle`.

In code comments, error texts and tests, write the short form `spec/<file>#<anchor>`, for
example `spec/board#lifecycle`. It means `docs/design/spec/<file>.md#<anchor>`. `spec/<file>`
without an anchor names the whole file, for example `spec/profiles`. A text that crewboard
prints to a user (an error, a warning, an issue body) uses the full path
`docs/design/spec/<file>.md#<anchor>`, because the reader may not know the short form. The skill
(`skill/SKILL.md` and `skill/references/`) is installed without `docs/`, and the agent account may
not read the crewboard repo, so the skill names a docs path as plain text for a reader with a clone
of crewboard (`docs/design/spec/<file>.md`) and links only its own files. The anchor is the
GitHub slug of the heading: lower case, spaces become `-`, punctuation and backticks are
dropped (`` `watch` semantics `` becomes `watch-semantics`).

Do not change a heading without a reason: the anchor is part of the reference. If a heading
must change, update every reference in the same PR (`git grep 'spec/<file>#<old anchor>'`).
The link check (`tests/md_links.rs`) fails on a Markdown link to an anchor that does not exist.
