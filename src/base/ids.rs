//! The catalog of refusal ids: every rule id that a [`Refusal`](super::Refusal) can name.
//!
//! A refusal names its rule with a [`RuleId`]. Only this module makes one, so every id that the
//! CLI can print is in [`ALL`], and a misspelt id does not compile. The text of an id never
//! changes: scripts, the skill and the docs name it. Each id has a one-line meaning; the page
//! `docs/reference/refusals.md` is generated from this catalog (test `refusal_reference`).
//!
//! The ids are grouped by area. A new id goes into its group and into [`ALL`] (the macro does
//! both).

use std::fmt;

/// A stable rule id (kebab case) with its meaning. Made only by the catalog below.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RuleId {
    id: &'static str,
    meaning: &'static str,
    area: &'static str,
}

impl RuleId {
    /// The id as the CLI prints it, for example `field-wrong-writer`.
    pub const fn as_str(self) -> &'static str {
        self.id
    }

    /// One line on what the refusal means.
    pub const fn meaning(self) -> &'static str {
        self.meaning
    }

    /// The area of the catalog that the id belongs to.
    pub const fn area(self) -> &'static str {
        self.area
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id)
    }
}

impl fmt::Debug for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.id)
    }
}

impl PartialEq<str> for RuleId {
    fn eq(&self, other: &str) -> bool {
        self.id == other
    }
}

impl PartialEq<&str> for RuleId {
    fn eq(&self, other: &&str) -> bool {
        self.id == *other
    }
}

impl PartialEq<RuleId> for &str {
    fn eq(&self, other: &RuleId) -> bool {
        *self == other.id
    }
}

/// Defines the ids of one area and lists them in `ALL`.
macro_rules! catalog {
    ($( $area:literal { $( $name:ident = $id:literal : $meaning:literal ; )* } )*) => {
        $( $(
            #[doc = $meaning]
            pub const $name: RuleId = RuleId { id: $id, meaning: $meaning, area: $area };
        )* )*

        /// Every id of the catalog, area by area.
        pub const ALL: &[RuleId] = &[ $( $( $name, )* )* ];
    };
}

catalog! {
    "Board fields and options" {
        FIELD_NOT_ON_ITEM = "field-not-on-item" :
            "The field is not a field of this kind of item (ticket or agent item).";
        FIELD_TYPE_MISMATCH = "field-type-mismatch" :
            "A board field exists with another type than crewboard needs.";
        FIELD_WRONG_WRITER = "field-wrong-writer" :
            "The field-writer table does not let this actor write this field.";
        USE_TRANSITION = "use-transition" :
            "Status and State are written only through their transition tables, never directly.";
        ROLE_NEEDS_OWNER_APPROVAL = "role-needs-owner-approval" :
            "The Role of an agent is written only after an owner approval comment.";
        KEY_FROZEN = "key-frozen" :
            "The Key of an agent is frozen after its approval.";
        TEXT_FIELD_TOO_LONG = "text-field-too-long" :
            "A text field value is longer than 1024 bytes (UTF-8).";
        OPTION_MISSING = "option-missing" :
            "The single-select field has no option with this name.";
        OPTION_DUPLICATE_NAME = "option-duplicate-name" :
            "An option with this name exists already.";
        OPTION_UNKNOWN_ID = "option-unknown-id" :
            "No option has this id.";
        OPTION_LAST = "option-last" :
            "A single-select field keeps at least one option.";
        OPTION_FIELD_FULL = "option-field-full" :
            "The field would have more options than GitHub allows (100).";
        OPTION_ID_LOST = "option-id-lost" :
            "An option id is missing from the planned list or after the write; the write would empty that value on items.";
        OPTION_NAME_CHANGED = "option-name-changed" :
            "The write renamed an option that it did not plan to rename.";
        OPTION_NOT_REMOVED = "option-not-removed" :
            "An option that the write removes is still there after it.";
        OPTION_LOST_CONCURRENTLY = "option-lost-concurrently" :
            "Another writer changed the options at the same time; run the command again.";
        AGENT_FIELD_FULL = "agent-field-full" :
            "The Agent field is full and too few options are free for the new agent.";
        DROPPED_OPTION_MISSING = "dropped-option-missing" :
            "The Status field of the board has no `Dropped` option; the owner runs `init` again.";
        PRUNE_COORDINATOR_ONLY = "prune-coordinator-only" :
            "`prune` is a coordinator command.";
        PRUNE_AGENT_NOT_LEFT = "prune-agent-not-left" :
            "`prune` removes the option of an agent that has left only.";
        PRUNE_IN_USE = "prune-in-use" :
            "A ticket still uses the option.";
        PRUNE_CHANGED_TICKET_VALUES = "prune-changed-ticket-values" :
            "The Agent value of a ticket changed during the prune.";
    }
    "Agents: join, approval and identity" {
        NAME_TAKEN = "name-taken" :
            "The agent name is registered with another key.";
        ONE_AGENT_PER_CONFIG = "one-agent-per-config" :
            "This config dir holds another agent of the initiative already.";
        NO_JOIN_REQUEST = "no-join-request" :
            "The agent has no valid join request in the registry issue.";
        BAD_KEY = "bad-key" :
            "A key cannot be read.";
        KEY_MISMATCH = "key-mismatch" :
            "The Key field differs from the key of the signed join request.";
        ALREADY_APPROVED = "already-approved" :
            "The agent is approved already; the owner changes the role with `/crew role`.";
        AGENT_REJECTED = "agent-rejected" :
            "The owner rejected the agent.";
        APPROVE_AMBIGUOUS_JOIN = "approve-ambiguous-join" :
            "The agent has more than one undecided join request with different keys.";
        APPROVE_OWNER_ACCOUNT = "approve-owner-account" :
            "The join came from the owner account, which an agent never uses on a crew board.";
        AGENT_NOT_APPROVED = "agent-not-approved" :
            "The agent is not approved.";
        AGENT_APPROVAL_PENDING = "agent-approval-pending" :
            "The approval is in the registry log, but the item State is still `requested`; run the command again in a moment.";
        SENDER_NOT_APPROVED = "sender-not-approved" :
            "This agent is not approved, so it cannot sign messages.";
        SINGLE_COORDINATOR = "single-coordinator" :
            "Another agent is the coordinator; a crew has one.";
        CREW_LOGIN_IS_OWNER = "crew-login-is-owner" :
            "The agent account of `init` is the owner account.";
        AGENT_GH_NOT_CREW = "agent-gh-not-crew" :
            "The registry binds this agent to the owner account.";
        AGENT_TOKEN_NOT_CREW = "agent-token-not-crew" :
            "The token belongs to another account than the one the registry binds to the agent.";
        AGENT_USES_OWNER_TOKEN = "agent-uses-owner-token" :
            "The token is the owner's; an agent loads its own token.";
        OWNER_TOKEN_IN_AGENT_DIR = "owner-token-in-agent-dir" :
            "An agent dir has no token of its own, so crewboard would use the owner's login.";
        OWNER_COMMAND_NOT_OWNER = "owner-command-not-owner" :
            "An owner command was run with a token that is not the owner's.";
        CREW_TOKEN_LOGIN = "crew-token-login" :
            "The token belongs to another account than the one that was named.";
        CREW_TOKEN_SCOPES = "crew-token-scopes" :
            "The token lacks a scope that an agent needs.";
        INIT_IDENTITY_MODE_CHANGE = "init-identity-mode-change" :
            "`init` would change the identity mode of an existing board.";
        BOARD_IDENTITY_CHANGED = "board-identity-changed" :
            "The initiative file in the repo names another board; an agent never follows a moved board.";
        UNKNOWN_RUNTIME = "unknown-runtime" :
            "The runtime is not one that crewboard knows.";
        UNIX_ONLY = "unix-only" :
            "The command needs a Unix host (macOS or Linux).";
    }
    "Agents: lifecycle" {
        LIFECYCLE_UNKNOWN_TRANSITION = "lifecycle-unknown-transition" :
            "The State change is not in the lifecycle table.";
        LIFECYCLE_WRONG_ACTOR = "lifecycle-wrong-actor" :
            "This actor may not make this State change.";
        AGENT_PAUSED = "agent-paused" :
            "The agent is paused: it takes nothing new until `resume`.";
        AGENT_NOT_ACTIVE = "agent-not-active" :
            "The agent is not `active`; `crewboard start` sets it.";
        AGENT_LEFT = "agent-left" :
            "The registry log ended the agent; its loop stops.";
        RESUME_OWNER_PAUSED = "resume-owner-paused" :
            "The owner paused the agent; only the owner resumes it.";
        HEARTBEAT_NOT_RUNNING = "heartbeat-not-running" :
            "Only a running agent writes heartbeats.";
        STOP_FORCE_SELF = "stop-force-self" :
            "An agent stops itself with a plain `stop`; `--force` is for another agent.";
        STOP_FORCE_NEEDS_COORDINATOR = "stop-force-needs-coordinator" :
            "`stop --force` is for the coordinator or the owner.";
        STOP_FORCE_NOT_DEV = "stop-force-not-dev" :
            "The coordinator is stopped only after the owner hands the role over.";
        STOP_FORCE_ALREADY_LEFT = "stop-force-already-left" :
            "The agent is stopped already.";
        STOP_FORCE_NOT_RUNNING = "stop-force-not-running" :
            "The agent is not an approved, running agent.";
        TOOLCHAIN_UNREACHABLE = "toolchain-unreachable" :
            "The agent cannot run the build tools of the repo (PATH or a sandbox).";
        SELFTEST_NEEDS_APPROVAL = "selftest-needs-approval" :
            "`selftest` needs an approved agent.";
        SELFTEST_FAILED = "selftest-failed" :
            "The selftest message did not come back as expected.";
    }
    "Messages and signatures" {
        MALFORMED_MESSAGE = "malformed-message" :
            "A signed message does not have the protocol shape.";
        VISIBLE_LINE_MISMATCH = "visible-line-mismatch" :
            "The visible sender line of a message does not match its header.";
        BAD_SIGNATURE = "bad-signature" :
            "The signature of a message does not verify.";
        BODY_TOO_LONG = "body-too-long" :
            "A message body is longer than the limit.";
        SECRET_IN_BODY = "secret-in-body" :
            "The text holds something that looks like a secret; nothing was posted.";
        TEXT_READS_AS_ACT = "text-reads-as-act" :
            "The text starts like a protocol act; a reader would take it for one.";
        REPO_NOT_IN_INITIATIVE = "repo-not-in-initiative" :
            "The repo is not a repo of the initiative.";
        UNREAD_MESSAGE_ON_TICKET = "unread-message-on-ticket" :
            "The ticket has a message to this agent that it has not read.";
        UNREAD_EVENTS = "unread-events" :
            "There are events that this agent has not read.";
        HEADLESS_SCOPE = "headless-scope" :
            "A headless coordinator does not do this; the interactive coordinator does.";
        OWNER_COMMAND_MALFORMED = "owner-command-malformed" :
            "An owner command (`/crew ...`) cannot be read.";
        OWNER_COMMAND_BAD_ROLE = "owner-command-bad-role" :
            "The role in an owner command is not `coordinator` or `dev`.";
    }
    "Tickets: create, assign and queue" {
        TICKET_TITLE_MISSING = "ticket-title-missing" :
            "`ticket new` needs a title.";
        TICKET_TITLE_TOO_LONG = "ticket-title-too-long" :
            "The title is longer than 256 characters.";
        TICKET_GATE_CONFLICT = "ticket-gate-conflict" :
            "`--gate-ci` and a `Gate:` line in the body name two Gates.";
        TICKET_IS_PR = "ticket-is-pr" :
            "The number is a pull request, not a ticket.";
        TICKET_NOT_WORK = "ticket-not-work" :
            "The issue has no `crew:work` label.";
        TICKET_NOT_ON_BOARD = "ticket-not-on-board" :
            "The ticket has no signed info comment with its item id; the coordinator assigns it first.";
        TICKET_CLOSED = "ticket-closed" :
            "The issue is closed.";
        TICKET_STATUS_UNSET = "ticket-status-unset" :
            "The ticket has no Status on the board.";
        TICKET_STATUS_UNKNOWN = "ticket-status-unknown" :
            "The ticket has a Status that crewboard does not know.";
        TICKET_AGENT_NOT_SIGNED = "ticket-agent-not-signed" :
            "The board Agent names someone else than the latest signed assignment.";
        TICKET_ALREADY_ASSIGNED = "ticket-already-assigned" :
            "The ticket has an agent already.";
        ASSIGN_NEEDS_TO = "assign-needs-to" :
            "`assign` needs `--to`, except to set the queue position of a queued ticket.";
        ORDER_INVALID = "order-invalid" :
            "The order is not a whole number from 0 to 1000000.";
        REORDER_NOT_QUEUED = "reorder-not-queued" :
            "Only a Ready ticket that has an agent has a queue position.";
        REBALANCE_NOT_READY = "rebalance-not-ready" :
            "Only a Ready ticket moves between the queues of agents.";
        REBALANCE_NO_AGENT = "rebalance-no-agent" :
            "The ticket has no agent to move it from.";
        REBALANCE_SAME_AGENT = "rebalance-same-agent" :
            "The ticket is in the queue of this agent already.";
        CAPABILITY_MISMATCH = "capability-mismatch" :
            "The agent lacks a capability that the ticket needs.";
        CAPABILITY_OVERRIDE_NEEDS_REASON = "capability-override-needs-reason" :
            "`--override-capabilities` needs `--why`.";
        NEEDS_INVALID = "needs-invalid" :
            "A requirement of the ticket is not valid.";
        SPLIT_NOT_IN_PROGRESS = "split-not-in-progress" :
            "Only an In Progress or Blocked ticket is split.";
        SPLIT_NO_AGENT = "split-no-agent" :
            "The ticket has no agent to queue the new ticket for.";
        ADOPT_PR_PARENT_NOT_DONE = "adopt-pr-parent-not-done" :
            "The PR to adopt belongs to a ticket that is not ended.";
    }
    "Tickets: status moves" {
        STATUS_UNKNOWN_TRANSITION = "status-unknown-transition" :
            "The Status move is not in the status table.";
        STATUS_WRONG_WRITER = "status-wrong-writer" :
            "This actor may not make this Status move.";
        STATUS_NO_CHANGE = "status-no-change" :
            "The ticket has this Status already.";
        STATUS_CHANGED_MEANWHILE = "status-changed-meanwhile" :
            "Somebody moved the ticket after this command read it; nothing was written.";
        STATUS_GATE_MISSING = "status-gate-missing" :
            "A move to Ready needs a Gate.";
        STATUS_ANSWER_MISSING = "status-answer-missing" :
            "A Blocked ticket is taken only after an answer to the block.";
        STATUS_EVIDENCE_MISSING = "status-evidence-missing" :
            "A delivery needs complete evidence: PR, head SHA and CI.";
        STATUS_DONE_FINAL = "status-done-final" :
            "The ticket is Done; only the owner moves it.";
        STATUS_DROPPED_FINAL = "status-dropped-final" :
            "The ticket is Dropped; only the owner moves it.";
        STATUS_VERIFY_REQUIRED = "status-verify-required" :
            "Review to Done needs a passed verify.";
        STATUS_VERIFY_FAILED_REQUIRED = "status-verify-failed-required" :
            "Review to In Progress needs a failed verify.";
        STATUS_VERIFY_FAILED_AGENT_BUSY = "status-verify-failed-agent-busy" :
            "After a failed verify, a busy agent gets the ticket back in Ready.";
        STATUS_VERIFY_REQUEUE_AGENT_IDLE = "status-verify-requeue-agent-idle" :
            "After a failed verify, an idle agent gets the ticket back In Progress.";
        STATUS_AGENT_NOT_ACTIVE_DEV = "status-agent-not-active-dev" :
            "The agent is not an active dev agent.";
        STATUS_AGENT_NOT_ALIVE = "status-agent-not-alive" :
            "The agent is late or stale; it would not see the ticket.";
        STATUS_REASSIGN_REASON = "status-reassign-reason" :
            "`reassign` needs a reason.";
        STATUS_REASSIGN_NOT_JUSTIFIED = "status-reassign-not-justified" :
            "The agent is neither dead nor stopped; give a reason or wait.";
        MOVE_BAD_STATUS = "move-bad-status" :
            "The Status of `/crew move` is not one that the owner may set.";
        MOVE_DONE_NEEDS_CLOSE = "move-done-needs-close" :
            "A move to Done goes through `close`.";
        MOVE_DROPPED_NEEDS_CLOSE = "move-dropped-needs-close" :
            "A move to Dropped goes through `close`.";
        MOVE_NEEDS_ASSIGNEE = "move-needs-assignee" :
            "The move needs an agent, and the ticket has no signed assignment.";
        UNBLOCK_COORDINATOR_ONLY = "unblock-coordinator-only" :
            "`unblock` is a coordinator action.";
        UNBLOCK_NEEDS_BLOCKED = "unblock-needs-blocked" :
            "`unblock` needs a Blocked ticket.";
        UNBLOCK_NO_ASSIGNEE = "unblock-no-assignee" :
            "`unblock` needs a ticket with an assignee.";
        UNBLOCK_NOT_EFFECTIVE = "unblock-not-effective" :
            "The answer would not satisfy `take`; nothing was written.";
    }
    "Tickets: take, block and ask" {
        TAKE_NEEDS_ROLE = "take-needs-role" :
            "An agent without a role cannot take a ticket.";
        COORDINATOR_CANNOT_TAKE = "coordinator-cannot-take" :
            "The coordinator does not take tickets.";
        TAKE_AGENT_BUSY = "take-agent-busy" :
            "The agent has another ticket In Progress.";
        INTERRUPT_NEEDS_REASON = "interrupt-needs-reason" :
            "`--interrupt` needs `--why`.";
        INTERRUPT_NEEDS_READY_TICKET = "interrupt-needs-ready-ticket" :
            "`--interrupt` needs a Ready ticket, or an answered Blocked ticket, of this agent.";
        INTERRUPT_NOT_AHEAD = "interrupt-not-ahead" :
            "`--interrupt` needs a ticket ahead of the ticket in progress (a lower Order).";
        INTERRUPT_NOTHING_TO_PARK = "interrupt-nothing-to-park" :
            "The agent has no ticket In Progress to park.";
        INTERRUPT_PARK_AMBIGUOUS = "interrupt-park-ambiguous" :
            "The agent has more than one ticket In Progress; `--park` names the one to park.";
        INTERRUPT_PARK_NOT_IN_PROGRESS = "interrupt-park-not-in-progress" :
            "The ticket of `--park` is not a ticket of this agent that is In Progress.";
        ADOPT_PR_NEEDS_READY_TICKET = "adopt-pr-needs-ready-ticket" :
            "`--continue-pr` goes with the first take of a Ready ticket.";
        ADOPT_PR_BAD_BRANCH = "adopt-pr-bad-branch" :
            "The head branch of the PR is not the crew branch of the ticket.";
        ADOPT_PR_NOT_LINKED = "adopt-pr-not-linked" :
            "The PR does not link the ticket.";
        ADOPT_PR_NOT_OPEN = "adopt-pr-not-open" :
            "Only an open PR is adopted.";
        ADOPT_PR_NOT_RECORDED = "adopt-pr-not-recorded" :
            "The reassign notice of the coordinator does not name this PR.";
        ADOPT_PR_OTHER_REPO = "adopt-pr-other-repo" :
            "The PR is in another repo than the ticket.";
        BLOCK_NEEDS_REASON = "block-needs-reason" :
            "`block` needs a reason.";
        ASK_NEEDS_TEXT = "ask-needs-text" :
            "`ask` needs the question.";
        ASK_NOT_ASSIGNEE = "ask-not-assignee" :
            "Only the assignee or the coordinator asks the owner about a ticket.";
        MINE_NOT_YOUR_TICKET = "mine-not-your-ticket" :
            "`mine <n>` on a ticket that the signed log does not assign to you.";
    }
    "Tickets: done, verify, merge and close" {
        EVIDENCE_PR_URL = "evidence-pr-url" :
            "The text is not a PR URL.";
        EVIDENCE_PR_MISSING = "evidence-pr-missing" :
            "The PR does not exist.";
        EVIDENCE_PR_FOREIGN_REPO = "evidence-pr-foreign-repo" :
            "The PR is not in a repo of the initiative.";
        EVIDENCE_PR_NOT_LINKED = "evidence-pr-not-linked" :
            "The PR does not link the ticket.";
        EVIDENCE_PR_CLOSED = "evidence-pr-closed" :
            "The PR is closed without a merge.";
        EVIDENCE_BRANCH_NAME = "evidence-branch-name" :
            "The branch name is not the crew branch of the ticket.";
        EVIDENCE_BRANCH_PROTECTED = "evidence-branch-protected" :
            "The PR head is a protected branch.";
        EVIDENCE_HEAD_MOVED = "evidence-head-moved" :
            "The PR head moved while the evidence was collected.";
        EVIDENCE_CI_MISSING = "evidence-ci-missing" :
            "No CI run exists for the head, and the ticket does not declare `CI: none`.";
        EVIDENCE_CI_NOT_GREEN = "evidence-ci-not-green" :
            "CI of the head is not green.";
        EVIDENCE_CI_UNREADABLE = "evidence-ci-unreadable" :
            "CI cannot be read, and the ticket does not declare `CI: none`.";
        EVIDENCE_RUN_REF = "evidence-run-ref" :
            "The text is not a run id or a run URL.";
        EVIDENCE_RUN_FOREIGN_REPO = "evidence-run-foreign-repo" :
            "The run is not in a repo of the initiative.";
        EVIDENCE_RUN_WRONG_SHA = "evidence-run-wrong-sha" :
            "The named run ran on another commit than the PR head.";
        PR_ALREADY_MERGED = "pr-already-merged" :
            "The PR is merged already; a remaining change needs a new PR.";
        LOCAL_GATE_MISSING = "local-gate-missing" :
            "`--local-run` needs the text of the local gate.";
        LOCAL_GATE_INVALID = "local-gate-invalid" :
            "The text of the local gate does not have the expected shape.";
        LOCAL_GATE_NOT_ENABLED = "local-gate-not-enabled" :
            "`--local-run` needs `ci_mode: local` in the authority file.";
        LOCAL_GATE_UNEXPECTED = "local-gate-unexpected" :
            "`--local-run` is for a local-gate delivery only.";
        OWNER_ROUTE_CLOSING_KEYWORD = "owner-route-closing-keyword" :
            "An owner-route PR has a closing keyword for the ticket; it says `Refs` until the owner applied the patch.";
        WORKFLOW_NEEDS_OWNER = "workflow-needs-owner" :
            "The change touches a workflow file: the owner merges it (the owner route).";
        PROTECTED_PATH_OWNER_MERGE = "protected-path-owner-merge" :
            "The PR changes an authority file: only the owner merges it.";
        VERIFY_NEEDS_REVIEW = "verify-needs-review" :
            "`verify` needs a ticket in Review.";
        VERIFY_NO_EVIDENCE = "verify-no-evidence" :
            "The ticket has no delivery to verify.";
        VERIFY_FAIL_NEEDS_NOTE = "verify-fail-needs-note" :
            "`verify --fail` needs a note for the dev.";
        VERIFY_OWNER_QUESTION_OPEN = "verify-owner-question-open" :
            "The ticket has an open question to the owner.";
        VERIFY_OWNER_ROUTE = "verify-owner-route" :
            "The delivery is on the owner route: the owner merges it, then the coordinator closes the ticket.";
        VERIFY_COORDINATOR_ONLY = "verify-coordinator-only" :
            "Only the coordinator verifies and ticks.";
        TICK_HEADLESS_ONLY = "tick-headless-only" :
            "`tick` is for a headless config dir.";
        MERGE_NEEDS_REVIEW = "merge-needs-review" :
            "`merge` needs a ticket in Review.";
        MERGE_VERIFY_REQUIRED = "merge-verify-required" :
            "`merge` needs a passed verify of the latest delivery.";
        MERGE_PR_NOT_OPEN = "merge-pr-not-open" :
            "The PR is not open; there is nothing to merge.";
        MERGE_METHOD_UNKNOWN = "merge-method-unknown" :
            "The merge method is not squash, merge or rebase.";
        MERGE_HEAD_MOVED = "merge-head-moved" :
            "GitHub refused the merge: the PR head moved after the verify.";
        MERGE_NOT_ALLOWED = "merge-not-allowed" :
            "GitHub refused the merge.";
        MERGE_DONE_TICKET_OPEN = "merge-done-ticket-open" :
            "The PR is merged, but a later step failed; `close --verified` finishes the ticket.";
        PR_NOT_MERGEABLE = "pr-not-mergeable" :
            "GitHub does not let the PR be merged now.";
        CLOSE_NEEDS_REASON = "close-needs-reason" :
            "The close of this ticket needs a reason.";
        CLOSE_VERIFIED_NOT_MERGED = "close-verified-not-merged" :
            "`close --verified` needs a PR that is merged.";
        CLOSE_VERIFIED_HEAD_MOVED = "close-verified-head-moved" :
            "The PR is not merged at the verified head.";
    }
    "Authority file" {
        AUTHORITY_BAD_FRONT_MATTER = "authority-bad-front-matter" :
            "The front matter of the authority file cannot be read.";
        AUTHORITY_OWNER_MISMATCH = "authority-owner-mismatch" :
            "The authority file names another owner than the board config.";
        AUTHORITY_NO_TICKET_CREATION = "authority-no-ticket-creation" :
            "The authority file does not let the coordinator create tickets.";
        AUTHORITY_SPLIT_NOT_ALLOWED = "authority-split-not-allowed" :
            "The authority file does not let the coordinator split tickets.";
        AUTHORITY_NO_PRS = "authority-no-prs" :
            "The authority file does not let dev agents open PRs.";
        AUTHORITY_MERGE_NOT_ALLOWED = "authority-merge-not-allowed" :
            "The authority file does not let the coordinator merge.";
        AUTHORITY_NEEDS_OWNER = "authority-needs-owner" :
            "The authority file reserves the merge for the owner.";
    }
    "Profiles" {
        PROFILE_TAG_INVALID = "profile-tag-invalid" :
            "The text is not a valid tag.";
        PROFILE_TAG_CONFLICT = "profile-tag-conflict" :
            "The tag is in `--tag` and in `--untag`.";
        PROFILE_TIER_INVALID = "profile-tier-invalid" :
            "The text is not a valid tier.";
        PROFILE_ABOUT_TOO_LONG = "profile-about-too-long" :
            "The background text is longer than the limit.";
        CAPABILITIES_TOO_LONG = "capabilities-too-long" :
            "The profile has more declared tags than fit.";
    }
    "Setup, install and update" {
        SETUP_INCOMPLETE = "setup-incomplete" :
            "A setup check of `doctor --setup` failed.";
        REPO_EMPTY_NO_PR = "repo-empty-no-pr" :
            "The repo has no commit, so no pull request can be opened.";
        REPORT_STATUS_COORDINATOR_ONLY = "report-status-coordinator-only" :
            "`report-status` is a coordinator command.";
        REPORT_STATUS_READBACK = "report-status-readback" :
            "The status update was created, but it does not read back as written.";
        SECOND_WAKE_JOB = "second-wake-job" :
            "Another wake job exists for this agent; `--replace` replaces it.";
        SKILL_SOURCE = "skill-source" :
            "The skill source has no `SKILL.md`.";
        UPDATE_SOURCE = "update-source" :
            "The source of the update cannot be read.";
        UPDATE_RELEASE = "update-release" :
            "The release does not exist or cannot be read.";
        UPDATE_TARGET = "update-target" :
            "No release exists for this platform.";
        UPDATE_CHECKSUM = "update-checksum" :
            "The checksum of the download does not match `SHA256SUMS`.";
        UPDATE_TARBALL = "update-tarball" :
            "The release archive cannot be unpacked.";
        UPDATE_VERSION = "update-version" :
            "The new binary does not print the expected version.";
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_id_is_kebab_case_and_once_in_the_catalog() {
        let mut seen = BTreeSet::new();
        for r in ALL {
            let id = r.as_str();
            assert!(
                id.split('-').count() >= 2
                    && id.split('-').all(|w| !w.is_empty()
                        && w.bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())),
                "{id} is not kebab case"
            );
            assert!(seen.insert(id), "{id} is twice in the catalog");
            assert!(
                r.meaning().ends_with('.'),
                "{id}: the meaning is one sentence"
            );
            assert!(!r.area().is_empty());
        }
        assert!(ALL.len() > 150, "{} ids", ALL.len());
    }

    #[test]
    fn ids_keep_their_wire_text_and_compare_with_text() {
        // Scripts and the spec name these ids: the text must not change by accident.
        assert_eq!(STATUS_GATE_MISSING.as_str(), "status-gate-missing");
        assert_eq!(STATUS_ANSWER_MISSING.as_str(), "status-answer-missing");
        assert_eq!(CLOSE_NEEDS_REASON.as_str(), "close-needs-reason");
        assert_eq!(TICKET_ALREADY_ASSIGNED.as_str(), "ticket-already-assigned");
        assert_eq!(FIELD_WRONG_WRITER, "field-wrong-writer");
        assert_ne!(FIELD_WRONG_WRITER, "field-wrong-writers");
        assert_eq!(format!("{FIELD_WRONG_WRITER}"), "field-wrong-writer");
        assert_eq!(format!("{FIELD_WRONG_WRITER:?}"), "\"field-wrong-writer\"");
    }
}
