# Refusal ids

This page is generated from the catalog of refusal ids in `src/base/ids.rs`. Do not edit it by hand. To regenerate it, run:

```sh
CREWBOARD_UPDATE_DOCS=1 cargo test --test refusal_reference
```

A refused command exits 2 and prints `refused [rule=<id>]: <message>`. The message names the fix. The id never changes, so a script or an agent can match on it. The [exit codes reference](exit-codes.md) lists the exit codes.

## Board fields and options

| Id | Meaning |
|---|---|
| `field-not-on-item` | The field is not a field of this kind of item (ticket or agent item). |
| `field-type-mismatch` | A board field exists with another type than crewboard needs. |
| `field-wrong-writer` | The field-writer table does not let this actor write this field. |
| `use-transition` | Status and State are written only through their transition tables, never directly. |
| `role-needs-owner-approval` | The Role of an agent is written only after an owner approval comment. |
| `key-frozen` | The Key of an agent is frozen after its approval. |
| `text-field-too-long` | A text field value is longer than 1024 bytes (UTF-8). |
| `option-missing` | The single-select field has no option with this name. |
| `option-duplicate-name` | An option with this name exists already. |
| `option-unknown-id` | No option has this id. |
| `option-last` | A single-select field keeps at least one option. |
| `option-field-full` | The field would have more options than GitHub allows (100). |
| `option-id-lost` | An option id is missing from the planned list or after the write; the write would empty that value on items. |
| `option-name-changed` | The write renamed an option that it did not plan to rename. |
| `option-not-removed` | An option that the write removes is still there after it. |
| `option-lost-concurrently` | Another writer changed the options at the same time; run the command again. |
| `agent-field-full` | The Agent field is full and too few options are free for the new agent. |
| `dropped-option-missing` | The Status field of the board has no `Dropped` option; the owner runs `init` again. |
| `prune-coordinator-only` | `prune` is a coordinator command. |
| `prune-agent-not-left` | `prune` removes the option of an agent that has left only. |
| `prune-in-use` | A ticket still uses the option. |
| `prune-changed-ticket-values` | The Agent value of a ticket changed during the prune. |

## Agents: join, approval and identity

| Id | Meaning |
|---|---|
| `name-taken` | The agent name is registered with another key. |
| `one-agent-per-config` | This config dir holds another agent of the initiative already. |
| `no-join-request` | The agent has no valid join request in the registry issue. |
| `bad-key` | A key cannot be read. |
| `key-mismatch` | The Key field differs from the key of the signed join request. |
| `already-approved` | The agent is approved already; the owner changes the role with `/crew role`. |
| `agent-rejected` | The owner rejected the agent. |
| `approve-ambiguous-join` | The agent has more than one undecided join request with different keys. |
| `approve-owner-account` | The join came from the owner account, which an agent never uses on a crew board. |
| `agent-not-approved` | The agent is not approved. |
| `agent-approval-pending` | The approval is in the registry log, but the item State is still `requested`; run the command again in a moment. |
| `sender-not-approved` | This agent is not approved, so it cannot sign messages. |
| `single-coordinator` | Another agent is the coordinator; a crew has one. |
| `crew-login-is-owner` | The crew account of `init` is the owner account. |
| `agent-gh-not-crew` | The registry binds this agent to the owner account. |
| `agent-token-not-crew` | The token belongs to another account than the one the registry binds to the agent. |
| `agent-uses-owner-token` | The token is the owner's; an agent loads its own token. |
| `owner-token-in-agent-dir` | An agent dir has no token of its own, so crewboard would use the owner's login. |
| `owner-command-not-owner` | An owner command was run with a token that is not the owner's. |
| `crew-token-login` | The token belongs to another account than the one that was named. |
| `crew-token-scopes` | The token lacks a scope that an agent needs. |
| `init-identity-mode-change` | `init` would change the identity mode of an existing board. |
| `board-identity-changed` | The initiative file in the repo names another board; an agent never follows a moved board. |
| `unknown-runtime` | The runtime is not one that crewboard knows. |
| `unix-only` | The command needs a Unix host (macOS or Linux). |

## Agents: lifecycle

| Id | Meaning |
|---|---|
| `lifecycle-unknown-transition` | The State change is not in the lifecycle table. |
| `lifecycle-wrong-actor` | This actor may not make this State change. |
| `agent-paused` | The agent is paused: it takes nothing new until `resume`. |
| `agent-not-active` | The agent is not `active`; `crewboard start` sets it. |
| `agent-left` | The registry log ended the agent; its loop stops. |
| `resume-owner-paused` | The owner paused the agent; only the owner resumes it. |
| `heartbeat-not-running` | Only a running agent writes heartbeats. |
| `stop-force-self` | An agent stops itself with a plain `stop`; `--force` is for another agent. |
| `stop-force-needs-coordinator` | `stop --force` is for the coordinator or the owner. |
| `stop-force-not-dev` | The coordinator is stopped only after the owner hands the role over. |
| `stop-force-already-left` | The agent is stopped already. |
| `stop-force-not-running` | The agent is not an approved, running agent. |
| `toolchain-unreachable` | The agent cannot run the build tools of the repo (PATH or a sandbox). |
| `selftest-needs-approval` | `selftest` needs an approved agent. |
| `selftest-failed` | The selftest message did not come back as expected. |

## Messages and signatures

| Id | Meaning |
|---|---|
| `malformed-message` | A signed message does not have the protocol shape. |
| `visible-line-mismatch` | The visible sender line of a message does not match its header. |
| `bad-signature` | The signature of a message does not verify. |
| `body-too-long` | A message body is longer than the limit. |
| `secret-in-body` | The text holds something that looks like a secret; nothing was posted. |
| `text-reads-as-act` | The text starts like a protocol act; a reader would take it for one. |
| `repo-not-in-initiative` | The repo is not a repo of the initiative. |
| `unread-message-on-ticket` | The ticket has a message to this agent that it has not read. |
| `unread-events` | There are events that this agent has not read. |
| `headless-scope` | A headless coordinator does not do this; the interactive coordinator does. |
| `owner-command-malformed` | An owner command (`/crew ...`) cannot be read. |
| `owner-command-bad-role` | The role in an owner command is not `coordinator` or `dev`. |

## Tickets: create, assign and queue

| Id | Meaning |
|---|---|
| `ticket-title-missing` | `ticket new` needs a title. |
| `ticket-title-too-long` | The title is longer than 256 characters. |
| `ticket-gate-conflict` | `--gate-ci` and a `Gate:` line in the body name two Gates. |
| `ticket-is-pr` | The number is a pull request, not a ticket. |
| `ticket-not-work` | The issue has no `crew:work` label. |
| `ticket-not-on-board` | The ticket has no signed info comment with its item id; the coordinator assigns it first. |
| `ticket-closed` | The issue is closed. |
| `ticket-status-unset` | The ticket has no Status on the board. |
| `ticket-status-unknown` | The ticket has a Status that crewboard does not know. |
| `ticket-agent-not-signed` | The board Agent names someone else than the latest signed assignment. |
| `ticket-already-assigned` | The ticket has an agent already. |
| `assign-needs-to` | `assign` needs `--to`, except to set the queue position of a queued ticket. |
| `order-invalid` | The order is not a whole number from 0 to 1000000. |
| `reorder-not-queued` | Only a Ready ticket that has an agent has a queue position. |
| `rebalance-not-ready` | Only a Ready ticket moves between the queues of agents. |
| `rebalance-no-agent` | The ticket has no agent to move it from. |
| `rebalance-same-agent` | The ticket is in the queue of this agent already. |
| `capability-mismatch` | The agent lacks a capability that the ticket needs. |
| `capability-override-needs-reason` | `--override-capabilities` needs `--why`. |
| `needs-invalid` | A requirement of the ticket is not valid. |
| `split-not-in-progress` | Only an In Progress or Blocked ticket is split. |
| `split-no-agent` | The ticket has no agent to queue the new ticket for. |
| `adopt-pr-parent-not-done` | The PR to adopt belongs to a ticket that is not ended. |

## Tickets: status moves

| Id | Meaning |
|---|---|
| `status-unknown-transition` | The Status move is not in the status table. |
| `status-wrong-writer` | This actor may not make this Status move. |
| `status-no-change` | The ticket has this Status already. |
| `status-changed-meanwhile` | Somebody moved the ticket after this command read it; nothing was written. |
| `status-gate-missing` | A move to Ready needs a Gate. |
| `status-answer-missing` | A Blocked ticket is taken only after an answer to the block. |
| `status-evidence-missing` | A delivery needs complete evidence: PR, head SHA and CI. |
| `status-done-final` | The ticket is Done; only the owner moves it. |
| `status-dropped-final` | The ticket is Dropped; only the owner moves it. |
| `status-verify-required` | Review to Done needs a passed verify. |
| `status-verify-failed-required` | Review to In Progress needs a failed verify. |
| `status-verify-failed-agent-busy` | After a failed verify, a busy agent gets the ticket back in Ready. |
| `status-verify-requeue-agent-idle` | After a failed verify, an idle agent gets the ticket back In Progress. |
| `status-agent-not-active-dev` | The agent is not an active dev agent. |
| `status-agent-not-alive` | The agent is late or stale; it would not see the ticket. |
| `status-reassign-reason` | `reassign` needs a reason. |
| `status-reassign-not-justified` | The agent is neither dead nor stopped; give a reason or wait. |
| `move-bad-status` | The Status of `/crew move` is not one that the owner may set. |
| `move-done-needs-close` | A move to Done goes through `close`. |
| `move-dropped-needs-close` | A move to Dropped goes through `close`. |
| `move-needs-assignee` | The move needs an agent, and the ticket has no signed assignment. |
| `unblock-coordinator-only` | `unblock` is a coordinator action. |
| `unblock-needs-blocked` | `unblock` needs a Blocked ticket. |
| `unblock-no-assignee` | `unblock` needs a ticket with an assignee. |
| `unblock-not-effective` | The answer would not satisfy `take`; nothing was written. |

## Tickets: take, block and ask

| Id | Meaning |
|---|---|
| `take-needs-role` | An agent without a role cannot take a ticket. |
| `coordinator-cannot-take` | The coordinator does not take tickets. |
| `take-agent-busy` | The agent has another ticket In Progress. |
| `interrupt-needs-reason` | `--interrupt` needs `--why`. |
| `interrupt-needs-ready-ticket` | `--interrupt` needs a Ready ticket, or an answered Blocked ticket, of this agent. |
| `interrupt-not-ahead` | `--interrupt` needs a ticket ahead of the ticket in progress (a lower Order). |
| `interrupt-nothing-to-park` | The agent has no ticket In Progress to park. |
| `interrupt-park-ambiguous` | The agent has more than one ticket In Progress; `--park` names the one to park. |
| `interrupt-park-not-in-progress` | The ticket of `--park` is not a ticket of this agent that is In Progress. |
| `adopt-pr-needs-ready-ticket` | `--continue-pr` goes with the first take of a Ready ticket. |
| `adopt-pr-bad-branch` | The head branch of the PR is not the crew branch of the ticket. |
| `adopt-pr-not-linked` | The PR does not link the ticket. |
| `adopt-pr-not-open` | Only an open PR is adopted. |
| `adopt-pr-not-recorded` | The reassign notice of the coordinator does not name this PR. |
| `adopt-pr-other-repo` | The PR is in another repo than the ticket. |
| `block-needs-reason` | `block` needs a reason. |
| `ask-needs-text` | `ask` needs the question. |
| `ask-not-assignee` | Only the assignee or the coordinator asks the owner about a ticket. |
| `mine-not-your-ticket` | `mine <n>` on a ticket that the signed log does not assign to you. |

## Tickets: done, verify, merge and close

| Id | Meaning |
|---|---|
| `evidence-pr-url` | The text is not a PR URL. |
| `evidence-pr-missing` | The PR does not exist. |
| `evidence-pr-foreign-repo` | The PR is not in a repo of the initiative. |
| `evidence-pr-not-linked` | The PR does not link the ticket. |
| `evidence-pr-closed` | The PR is closed without a merge. |
| `evidence-branch-name` | The branch name is not the crew branch of the ticket. |
| `evidence-branch-protected` | The PR head is a protected branch. |
| `evidence-head-moved` | The PR head moved while the evidence was collected. |
| `evidence-ci-missing` | No CI run exists for the head, and the ticket does not declare `CI: none`. |
| `evidence-ci-not-green` | CI of the head is not green. |
| `evidence-ci-unreadable` | CI cannot be read, and the ticket does not declare `CI: none`. |
| `evidence-run-ref` | The text is not a run id or a run URL. |
| `evidence-run-foreign-repo` | The run is not in a repo of the initiative. |
| `evidence-run-wrong-sha` | The named run ran on another commit than the PR head. |
| `pr-already-merged` | The PR is merged already; a remaining change needs a new PR. |
| `local-gate-missing` | `--local-run` needs the text of the local gate. |
| `local-gate-invalid` | The text of the local gate does not have the expected shape. |
| `local-gate-not-enabled` | `--local-run` needs `ci_mode: local` in the authority file. |
| `local-gate-unexpected` | `--local-run` is for a local-gate delivery only. |
| `owner-route-closing-keyword` | An owner-route PR has a closing keyword for the ticket; it says `Refs` until the owner applied the patch. |
| `workflow-needs-owner` | The change touches a workflow file: the owner merges it (the owner route). |
| `protected-path-owner-merge` | The PR changes an authority file: only the owner merges it. |
| `verify-needs-review` | `verify` needs a ticket in Review. |
| `verify-no-evidence` | The ticket has no delivery to verify. |
| `verify-fail-needs-note` | `verify --fail` needs a note for the dev. |
| `verify-owner-question-open` | The ticket has an open question to the owner. |
| `verify-owner-route` | The delivery is on the owner route: the owner merges it, then the coordinator closes the ticket. |
| `verify-coordinator-only` | Only the coordinator verifies and ticks. |
| `tick-headless-only` | `tick` is for a headless config dir. |
| `merge-needs-review` | `merge` needs a ticket in Review. |
| `merge-verify-required` | `merge` needs a passed verify of the latest delivery. |
| `merge-pr-not-open` | The PR is not open; there is nothing to merge. |
| `merge-method-unknown` | The merge method is not squash, merge or rebase. |
| `merge-head-moved` | GitHub refused the merge: the PR head moved after the verify. |
| `merge-not-allowed` | GitHub refused the merge. |
| `merge-done-ticket-open` | The PR is merged, but a later step failed; `close --verified` finishes the ticket. |
| `pr-not-mergeable` | GitHub does not let the PR be merged now. |
| `close-needs-reason` | The close of this ticket needs a reason. |
| `close-verified-not-merged` | `close --verified` needs a PR that is merged. |
| `close-verified-head-moved` | The PR is not merged at the verified head. |

## Authority file

| Id | Meaning |
|---|---|
| `authority-bad-front-matter` | The front matter of the authority file cannot be read. |
| `authority-owner-mismatch` | The authority file names another owner than the board config. |
| `authority-no-ticket-creation` | The authority file does not let the coordinator create tickets. |
| `authority-split-not-allowed` | The authority file does not let the coordinator split tickets. |
| `authority-no-prs` | The authority file does not let dev agents open PRs. |
| `authority-merge-not-allowed` | The authority file does not let the coordinator merge. |
| `authority-needs-owner` | The authority file reserves the merge for the owner. |

## Profiles

| Id | Meaning |
|---|---|
| `profile-tag-invalid` | The text is not a valid tag. |
| `profile-tag-conflict` | The tag is in `--tag` and in `--untag`. |
| `profile-tier-invalid` | The text is not a valid tier. |
| `profile-about-too-long` | The background text is longer than the limit. |
| `capabilities-too-long` | The profile has more declared tags than fit. |

## Setup, install and update

| Id | Meaning |
|---|---|
| `setup-incomplete` | A setup check of `doctor --setup` failed. |
| `repo-empty-no-pr` | The repo has no commit, so no pull request can be opened. |
| `report-status-coordinator-only` | `report-status` is a coordinator command. |
| `report-status-readback` | The status update was created, but it does not read back as written. |
| `second-wake-job` | Another wake job exists for this agent; `--replace` replaces it. |
| `skill-source` | The skill source has no `SKILL.md`. |
| `update-source` | The source of the update cannot be read. |
| `update-release` | The release does not exist or cannot be read. |
| `update-target` | No release exists for this platform. |
| `update-checksum` | The checksum of the download does not match `SHA256SUMS`. |
| `update-tarball` | The release archive cannot be unpacked. |
| `update-version` | The new binary does not print the expected version. |
