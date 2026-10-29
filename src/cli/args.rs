//! The clap types of the command tree: `Cli`, `Command` and the argument structs and enums of the
//! commands. They hold the `--help` texts and no logic beyond `Command::name` and
//! `family`.

// The doc comments of the args below are the `--help` texts that clap prints as they are, so
// placeholders like `<runtime>` and plain URLs stay plain. Rustdoc would read them as HTML.
#![allow(rustdoc::invalid_html_tags, rustdoc::bare_urls)]

use crate::exit::Family;
use crate::ops;
use crate::protocol::names::{Kind, To};
use crate::rules::Role;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

fn parse_to(s: &str) -> Result<To, String> {
    To::parse(s).map_err(|e| e.to_string())
}

fn parse_kind(s: &str) -> Result<Kind, String> {
    Kind::parse(s).map_err(|e| e.to_string())
}

/// Help text of `say --kind`: the list comes from the enum.
fn kind_help() -> &'static str {
    static HELP: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| format!("Kind of the message: one of {}.", Kind::list()));
    HELP.as_str()
}

#[derive(Debug, Parser)]
#[command(
    name = "crewboard",
    version,
    about = "AI agents work as a crew through a GitHub Project board"
)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        help = "Directory for the local config, keys and cache",
        long_help = "Directory for the local config, keys and cache (default ~/.config/crewboard, or \
                     $CREWBOARD_CONFIG_DIR)."
    )]
    pub config_dir: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        help = "Env file with the GitHub token (mode 600)",
        long_help = "Load this env file (`KEY=VALUE` lines; it must define GH_TOKEN or GITHUB_TOKEN and \
                     have mode 600). Its token wins over the token variables of the caller. Without \
                     it, the `agent.env` of the config dir is loaded when there is one. An owner \
                     command (init, approve, audit, move, and pause, resume, stop or agent profile \
                     set with --agent) outside an agent dir reads only this file: without it, it runs \
                     with your own gh login and ignores GH_TOKEN and GITHUB_TOKEN."
    )]
    pub env_file: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

/// Which initiative a command works on.
#[derive(Debug, Args, Clone, Default)]
pub struct Sel {
    /// Initiative name (default: the only one in the config dir).
    #[arg(long)]
    pub initiative: Option<String>,
    /// Read the board config (.crewboard.toml shape) from this file.
    #[arg(long, hide = true)]
    pub initiative_file: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    // Setting up a crew and its agents: init, join, approve, start, lifecycle, profiles, audit.
    #[command(flatten)]
    Crew(CrewCmd),
    // A dev agent working on its tickets.
    #[command(flatten)]
    Dev(DevCmd),
    // Planning the queue.
    #[command(flatten)]
    Plan(PlanCmd),
    // Verifying and ending tickets, and reporting.
    #[command(flatten)]
    Review(ReviewCmd),
    // Reading events and the board.
    #[command(flatten)]
    Watch(WatchCmd),
    // The host: doctor, lock, gate, skill, update, wake, hook and agent setup.
    #[command(flatten)]
    Host(HostCmd),
}

/// The commands of the group `crew`: setting up a crew and its agents: init, join, approve, start, lifecycle, profiles, audit.
#[derive(Debug, Subcommand)]
pub enum CrewCmd {
    /// Create the board, labels and authority file.
    ///
    /// Owner: create the board, fields, labels, registry issue, AUTHORITY.md and .crewboard.toml.
    /// It also deletes the four built-in project workflows that write Status (one way).
    #[command(display_order = 0)]
    Init {
        /// Name of the initiative, for example `my-crew`. It names the config file and the env file.
        #[arg(long)]
        initiative: String,
        /// Repository `owner/repo`. The first one is the main repo. Repeatable.
        #[arg(long = "repo", required = true)]
        repos: Vec<String>,
        /// Adopt this existing project number (create what is missing) instead of creating one.
        #[arg(long)]
        project: Option<u64>,
        /// Project title when creating one (default: the initiative name).
        #[arg(long)]
        title: Option<String>,
        /// GitHub login of the owner (default: the login of your own gh, or of --env-file; it
        /// must match).
        #[arg(long)]
        owner_login: Option<String>,
        /// GitHub login of the shared crew account that your agents act as (for example
        /// `crew-bot`). Written to .crewboard.toml as `crew_login`; added to the board
        /// with the Write role. Agent commands then refuse the owner's token. A contributor's
        /// agent may act as its own account instead; `approve` binds each agent to the account
        /// that wrote its join.
        #[arg(long)]
        crew_login: Option<String>,
        /// A one-person trial without a crew account: the agents use the owner's token and
        /// nothing separates owner from agent. Written to .crewboard.toml as
        /// `insecure_one_account`. Without `--crew-login` and without this flag, each agent acts
        /// as its own GitHub account.
        #[arg(long, conflicts_with = "crew_login")]
        insecure_one_account: bool,
        /// Do not open the pull request with AUTHORITY.md and .crewboard.toml.
        #[arg(long)]
        no_pr: bool,
        /// Do not create the branch rule. By default init adds a ruleset to the default branch of
        /// the main repo: a pull request is required, no force push, no deletion, 0 approvals,
        /// and repo admins can bypass it through a pull request. An existing rule is left alone.
        /// A repo where the token is not admin gets one line with the reason; init does not fail.
        #[arg(long)]
        no_branch_rule: bool,
        /// When the main repo has no workflow, add a minimal CI workflow
        /// (`.github/workflows/ci.yml`) to the same pull request, so that one merge brings CI. The
        /// test command comes from the root files (package.json, go.mod, pyproject.toml,
        /// requirements.txt, pom.xml, gradlew, Cargo.toml); a repo that none fits gets a step that fails until you edit it. Needs the
        /// `workflow` scope on the owner token (`gh auth refresh -h github.com -s workflow`).
        #[arg(long, conflicts_with = "no_pr")]
        with_ci: bool,
    },
    /// Ask to join the crew.
    ///
    /// Agent: create a key if none exists, the agent item and the join request.
    #[command(display_order = 1)]
    Join {
        /// The agent name, <runtime>@<host>, for example claude@host-a.
        #[arg(long)]
        agent: String,
        /// claude, codex or hermes, as `agent add --runtime` takes it. With `--model`, join writes
        /// the runtime label `claude-code/<model>` for claude, else `codex/<model>` or
        /// `hermes/<model>`; without it, the label is the first word alone (`claude-code`). The
        /// old form `<runtime>/<model>` is refused.
        #[arg(long, value_parser = ops::agent::parse_runtime)]
        runtime: ops::agent::AgentRuntime,
        /// The model, for example sonnet: the second part of the runtime label.
        #[arg(long, value_parser = ops::agent::parse_model)]
        model: Option<String>,
        /// Host name for the agent item (default: the host part of the agent name).
        #[arg(long)]
        host: Option<String>,
        /// Read .crewboard.toml from the default branch of this repo (`owner/repo`).
        #[arg(long)]
        repo: Option<String>,
        /// A tool whose version is detected (repeatable). Replaces the list of the agent
        /// config; without it the config's list, or the default list, is used.
        #[arg(long = "tool")]
        tools: Vec<String>,
        /// The agent's consent to a one-person trial board (`insecure_one_account` in
        /// .crewboard.toml). It is kept in the new agent config. It never makes a crew board a
        /// trial.
        #[arg(long)]
        insecure_one_account: bool,
        #[command(flatten)]
        sel: Sel,
    },
    /// Approve an agent that joined.
    ///
    /// Owner: approve a joined agent (posts `/crew approve`, sets State and Role, adds the Agent option).
    #[command(display_order = 2)]
    Approve {
        /// Names of joined agents, for example hermes@host-b codex@host-c. Each one is judged
        /// on its own; a refusal of one does not stop the others. The exit code is that of the
        /// first failure, or 0 when all passed.
        #[arg(required = true, num_args = 1..)]
        names: Vec<String>,
        /// Role of every named agent: dev (default) or coordinator.
        #[arg(long, default_value = "dev", value_parser = Role::parse_named)]
        role: Role,
        #[command(flatten)]
        sel: Sel,
    },
    /// Check approval and setup, set the agent active.
    ///
    /// Agent: check approval and AUTHORITY.md, run selftest, set active, print the role brief.
    #[command(display_order = 3)]
    Start {
        /// The agent's consent to a one-person trial board (`insecure_one_account` in
        /// .crewboard.toml). It is kept in the agent config. It never makes a crew board a
        /// trial.
        #[arg(long)]
        insecure_one_account: bool,
        #[command(flatten)]
        sel: Sel,
    },
    /// Remove Agent options of agents that left.
    ///
    /// Coordinator: remove Agent options of agents in state `left` that no ticket uses.
    #[command(display_order = 22)]
    Prune {
        #[command(flatten)]
        sel: Sel,
    },
    /// Finish the current step and take nothing new.
    ///
    /// Agent: finish the current step, take nothing new. Owner: `--agent <name>` posts
    /// `/crew pause <name>`.
    #[command(display_order = 23)]
    Pause {
        /// Owner only: the agent to pause (posts the owner command in the registry issue).
        #[arg(long)]
        agent: Option<String>,
        #[command(flatten)]
        sel: Sel,
    },
    /// Continue after a restart or a pause.
    ///
    /// Agent: after any restart. Lifts a pause (not an owner pause), runs the checks of `start`,
    /// then prints the to-do block of the role and the command that re-arms the watch loop.
    /// Owner: `--agent <name>` posts `/crew resume <name>`.
    #[command(display_order = 24)]
    Resume {
        /// Owner only: the agent to resume (posts the owner command in the registry issue).
        #[arg(long)]
        agent: Option<String>,
        #[command(flatten)]
        sel: Sel,
    },
    /// Drain open tickets, then leave the crew.
    ///
    /// Agent: drain (open tickets go back to Backlog through the coordinator), then leave.
    /// Owner: `--agent <name>` posts `/crew stop <name>`. With `--force` the owner or the
    /// coordinator ends an agent that cannot drain: its open tickets go back to Backlog and its
    /// State becomes left. The owner's forced stop also posts `/crew stop <name>`, because the
    /// registry log, not the State, stops an agent (the key stays valid; `/crew reject` revokes
    /// it).
    #[command(display_order = 25)]
    Stop {
        /// Owner or coordinator: the agent to stop (the owner posts the owner command in the
        /// registry issue).
        #[arg(long)]
        agent: Option<String>,
        /// With --agent: stop an agent that cannot drain, at once.
        #[arg(long, requires = "agent")]
        force: bool,
        #[command(flatten)]
        sel: Sel,
    },
    /// Send one heartbeat.
    ///
    /// Agent: one heartbeat.
    #[command(hide = true, display_order = 30)]
    Heartbeat {
        #[command(flatten)]
        sel: Sel,
    },
    /// Find unsigned, forged or edited messages.
    ///
    /// Owner: list headerless owner comments, bad signatures, edited messages, wrong writers.
    #[command(display_order = 34)]
    Audit {
        /// Print the findings as JSON.
        #[arg(long)]
        json: bool,
        /// Only findings of comments created since this time (`2026-10-16T08:00:00Z`) or
        /// this long ago (`90m`, `2h`).
        #[arg(long)]
        since: Option<String>,
        #[command(flatten)]
        sel: Sel,
    },
    /// Move a ticket by hand, in the signed log.
    ///
    /// Owner: post `/crew move <status>` on the ticket, then write the board Status, so `status`
    /// and `audit` see the move as the owner's and not as a board edit. Backlog keeps the Agent.
    /// Ready, In Progress, Review and Blocked need a signed assignment; a board Agent that differs
    /// is set to that assignee. A closed issue is reopened. Done is refused
    /// (`move-done-needs-close`): merge the PR or close the issue, and the coordinator closes the
    /// ticket. Runs with the owner's token (like approve and audit).
    #[command(display_order = 35)]
    Move {
        /// The ticket: an issue number, or `<owner>/<repo>#<n>`.
        issue: String,
        /// The new status: Backlog, Ready, "In Progress" (or in-progress), Review or Blocked.
        status: String,
        /// Why you move it. It goes on the lines after the command in the comment.
        #[arg(long)]
        why: Option<String>,
        #[command(flatten)]
        sel: Sel,
    },
}

impl CrewCmd {
    /// The command name as typed.
    pub fn name(&self) -> &'static str {
        match self {
            CrewCmd::Init { .. } => "init",
            CrewCmd::Join { .. } => "join",
            CrewCmd::Approve { .. } => "approve",
            CrewCmd::Start { .. } => "start",
            CrewCmd::Prune { .. } => "prune",
            CrewCmd::Pause { .. } => "pause",
            CrewCmd::Resume { .. } => "resume",
            CrewCmd::Stop { .. } => "stop",
            CrewCmd::Heartbeat { .. } => "heartbeat",
            CrewCmd::Audit { .. } => "audit",
            CrewCmd::Move { .. } => "move",
        }
    }
}

/// The commands of the group `dev`: a dev agent working on its tickets.
#[derive(Debug, Subcommand)]
pub enum DevCmd {
    /// Post a signed message.
    ///
    /// Agent: post a signed message.
    #[command(display_order = 7)]
    Say {
        /// The ticket: an issue number, or `<owner>/<repo>#<n>`.
        issue: String,
        /// Who should read it: an agent name, `coordinator`, `owner` or `all`.
        #[arg(long, value_parser = parse_to)]
        to: To,
        #[arg(long, value_parser = parse_kind, help = kind_help())]
        kind: Kind,
        /// The message that this one answers: `#<issue>/c-<comment id>` or `none`.
        #[arg(long)]
        re: Option<String>,
        /// File with the body (`-` reads standard input).
        #[arg(long)]
        body_file: Option<String>,
        /// The body as text.
        #[arg(long)]
        body: Option<String>,
        /// Post although a message for you is unread on this ticket
        /// (spec/events#unread-messages-on-a-ticket). Use it only after you have read and
        /// handled the message; never to skip it.
        #[arg(long = "override-unread")]
        override_unread: bool,
        #[command(flatten)]
        sel: Sel,
    },
    /// Take a Ready ticket and start work.
    ///
    /// Dev: ack and move Ready -> In Progress (also resumes a Blocked ticket after an answer).
    #[command(display_order = 8)]
    Take {
        /// The ticket: an issue number, or `<owner>/<repo>#<n>`.
        issue: String,
        /// Park your In Progress ticket (it becomes Blocked: "parked for #N") and take this
        /// one, which must be strictly ahead of it in your queue (a lower `Order`). The ticket is Ready, or
        /// your own Blocked ticket with its answer. Needs `--why`.
        #[arg(long, requires = "why")]
        interrupt: bool,
        /// Why the current ticket is parked (with `--interrupt`).
        #[arg(long, requires = "interrupt")]
        why: Option<String>,
        /// Which In Progress ticket to park, when you have more than one (`<n>` or
        /// `<owner>/<repo>#<n>`). Needed only then.
        #[arg(long, requires = "interrupt")]
        park: Option<String>,
        /// Continue on the open PR that an earlier agent of this ticket left (the URL that the
        /// reassign notice and `assign` name). It must be open, in the repo of the ticket, on a
        /// crew branch of this ticket, and name the ticket. See
        /// spec/tickets#work-that-an-earlier-agent-left.
        #[arg(
            long = "continue-pr",
            conflicts_with = "interrupt",
            value_name = "PR_URL"
        )]
        continue_pr: Option<String>,
        #[command(flatten)]
        sel: Sel,
    },
    /// Report a finished ticket and move it to Review.
    ///
    /// Dev: collect SHA and CI run, check them, post done, move to Review. Merge the base
    /// branch into yours before `done`. When the PR head is behind its base, `done` prints one
    /// warning line and goes on.
    #[command(display_order = 9)]
    Done {
        /// The ticket: an issue number, or `<owner>/<repo>#<n>`.
        issue: String,
        /// URL of the pull request, for example https://github.com/<owner>/<repo>/pull/<n>.
        #[arg(long)]
        pr: String,
        /// The commit you worked on; refused if the PR head is another commit now.
        #[arg(long)]
        sha: Option<String>,
        /// Deliver although events or messages for you are unread (messages, owner comments,
        /// and messages on this ticket). Without it, `done` refuses with `unread-events` or
        /// `unread-message-on-ticket`: run `watch --timeout 1m` first. Use it only after you
        /// have read and handled them; never to skip them.
        #[arg(long = "override-unread")]
        override_unread: bool,
        /// A CI run that is part of the evidence (spec/ci#which-ci-runs-count), for a Gate that
        /// needs a by-hand run: a run id, or a run URL. Repeat for more runs. It must be a run of
        /// the head SHA. It never hides a red push or pull_request run.
        #[arg(long = "run")]
        runs: Vec<String>,
        /// Only while AUTHORITY.md has `ci_mode: local` (the owner switched CI off): what the
        /// local run did, as one line of at most 400 bytes: "<host>: <command> rc=0, log <path>". Required then; the
        /// CI of the head is not judged. Refused with `local-gate-not-enabled` in `ci_mode: ci`.
        #[arg(long = "local-run")]
        local_run: Option<String>,
        #[command(flatten)]
        sel: Sel,
    },
    /// Move your ticket to Blocked.
    ///
    /// Dev: move to Blocked.
    #[command(display_order = 10)]
    Block {
        /// The ticket: an issue number, or `<owner>/<repo>#<n>`.
        issue: String,
        /// Why the ticket is blocked.
        #[arg(long)]
        why: String,
        /// Post although a message for you is unread on this ticket
        /// (spec/events#unread-messages-on-a-ticket). Use it only after you have read and
        /// handled the message; never to skip it.
        #[arg(long = "override-unread")]
        override_unread: bool,
        #[command(flatten)]
        sel: Sel,
    },
    /// Ask the owner a question.
    ///
    /// Any: post a question to the owner and set the label needs-owner (an In Progress ticket of the assignee becomes Blocked).
    #[command(display_order = 12)]
    Ask {
        /// The ticket: an issue number, or `<owner>/<repo>#<n>`.
        issue: String,
        /// File with the question (`-` reads standard input).
        #[arg(long)]
        body_file: Option<String>,
        /// The question as text.
        #[arg(long)]
        body: Option<String>,
        /// Post although a message for you is unread on this ticket
        /// (spec/events#unread-messages-on-a-ticket). Use it only after you have read and
        /// handled the message; never to skip it.
        #[arg(long = "override-unread")]
        override_unread: bool,
        #[command(flatten)]
        sel: Sel,
    },
}

impl DevCmd {
    /// The command name as typed.
    pub fn name(&self) -> &'static str {
        match self {
            DevCmd::Say { .. } => "say",
            DevCmd::Take { .. } => "take",
            DevCmd::Done { .. } => "done",
            DevCmd::Block { .. } => "block",
            DevCmd::Ask { .. } => "ask",
        }
    }
}

/// The commands of the group `plan`: planning the queue.
#[derive(Debug, Subcommand)]
pub enum PlanCmd {
    /// Answer a block so the agent can resume.
    ///
    /// Coordinator: post the answer that lets the assignee resume a Blocked ticket with `take`. A ticket that waits for a question to the owner gets an answer that withdraws the question.
    #[command(display_order = 11)]
    Unblock {
        /// The ticket: an issue number, or `<owner>/<repo>#<n>`.
        issue: String,
        /// File with the answer (`-` reads standard input). Default: "resume allowed: run take <n>".
        #[arg(long)]
        body_file: Option<String>,
        /// The answer as text.
        #[arg(long)]
        body: Option<String>,
        /// Post although a message for you is unread on this ticket
        /// (spec/events#unread-messages-on-a-ticket). Use it only after you have read and
        /// handled the message; never to skip it.
        #[arg(long = "override-unread")]
        override_unread: bool,
        #[command(flatten)]
        sel: Sel,
    },
    /// Give a ticket to an agent, or move a queued ticket.
    ///
    /// Coordinator: check the gate and the agent state, move to Ready. On a ticket that is
    /// queued already, `--order <n>` (with or without `--to` its agent) sets its position in the
    /// queue, and `--to <other agent>` moves it to the queue of that agent.
    #[command(display_order = 13)]
    Assign {
        /// The ticket: an issue number, or `<owner>/<repo>#<n>`.
        issue: String,
        /// Name of the dev agent that gets the ticket, for example claude@host-a. Only
        /// `assign <n> --order <number>` may leave it out (the queued ticket keeps its agent).
        #[arg(long)]
        to: Option<String>,
        /// Assign although the agent lacks a capability that the ticket needs (`Needs:` line or
        /// `needs:` label). Needs --why; both are recorded in the signed assignment message.
        #[arg(long, requires = "to")]
        override_capabilities: bool,
        /// Why the override is right (only with --override-capabilities).
        #[arg(long, requires = "override_capabilities")]
        why: Option<String>,
        /// Position in the queue of the agent: a whole number, lowest first (board field
        /// `Order`, created on first use). The next ticket of an agent is its lowest `Order`
        /// among Ready. Default: the end of the queue. On a queued ticket this
        /// moves it within the queue (or, with another `--to`, sets the position in the new
        /// queue; the default there is the end of it).
        #[arg(long, allow_hyphen_values = true)]
        order: Option<String>,
        #[command(flatten)]
        sel: Sel,
    },
    /// Create, adopt or split a ticket.
    ///
    /// Coordinator: create a ticket (`new`), put an existing issue on the board (`adopt`), or move the
    /// open part of a ticket to a new one (`split`).
    #[command(display_order = 16)]
    Ticket {
        #[command(subcommand)]
        action: TicketAction,
    },
    /// Take a ticket from its agent.
    ///
    /// Coordinator: with --to the ticket goes to Ready for that agent, without it to Backlog (a
    /// ticket in Ready always has an agent). A reason is needed: --reason dead, stopping, late or
    /// blocked-too-long (with --why for the text), or --derived.
    #[command(display_order = 20)]
    Reassign {
        /// The ticket: an issue number, or `<owner>/<repo>#<n>`.
        issue: String,
        /// Why: dead, stopping, late or blocked-too-long. Needed unless --derived.
        #[arg(
            long,
            required_unless_present = "derived",
            conflicts_with = "derived",
            value_parser = clap::builder::PossibleValuesParser::new(crate::rules::reassign::Reason::WORDS)
        )]
        reason: Option<String>,
        /// The free text that goes with the reason (what you saw).
        #[arg(long, requires = "reason")]
        why: Option<String>,
        /// Derive the reason from the board: the agent of the ticket is dead (no heartbeat
        /// for 5 intervals) or draining or left. Refused if it is none of these.
        #[arg(long)]
        derived: bool,
        /// The new agent: the ticket goes to Ready for it. Without --to the Agent field is
        /// cleared and the ticket goes to Backlog (`assign` sets an agent later).
        #[arg(long)]
        to: Option<String>,
        /// With --to: the new agent lacks a capability that the ticket needs; the --reason
        /// of the reassign is the reason of the override and is recorded.
        #[arg(long, requires = "to")]
        override_capabilities: bool,
        #[command(flatten)]
        sel: Sel,
    },
}

impl PlanCmd {
    /// The command name as typed.
    pub fn name(&self) -> &'static str {
        match self {
            PlanCmd::Unblock { .. } => "unblock",
            PlanCmd::Assign { .. } => "assign",
            PlanCmd::Ticket { .. } => "ticket",
            PlanCmd::Reassign { .. } => "reassign",
        }
    }
}

/// The commands of the group `review`: verifying and ending tickets, and reporting.
#[derive(Debug, Subcommand)]
pub enum ReviewCmd {
    /// Check evidence and CI of a ticket in Review.
    ///
    /// Coordinator: check evidence and CI of a ticket in Review.
    #[command(display_order = 17)]
    Verify {
        /// The ticket: an issue number, or `<owner>/<repo>#<n>`.
        issue: String,
        /// What you found when you checked the Gate yourself (recorded in the result).
        #[arg(long)]
        body: Option<String>,
        /// Your own Gate check failed although the evidence holds: record a failed verify with
        /// the body as the failure (needs --body; not for a headless coordinator).
        #[arg(long)]
        fail: bool,
        /// A CI run that is part of the evidence (spec/ci#which-ci-runs-count); see `done --run`.
        #[arg(long = "run")]
        runs: Vec<String>,
        /// What you checked for a local-run done (`ci_mode: local`): host, command, rc, log.
        /// Required then (not with --fail). Refused with `local-gate-not-enabled` in
        /// `ci_mode: ci`; not for a headless coordinator.
        #[arg(long = "local-run")]
        local_run: Option<String>,
        #[command(flatten)]
        sel: Sel,
    },
    /// Merge the PR of a verified ticket.
    ///
    /// Coordinator: merge the PR of a verified ticket at the verified head SHA, delete its
    /// branch, close the ticket. Only if AUTHORITY.md lets the coordinator merge.
    #[command(display_order = 18)]
    Merge {
        /// The ticket: an issue number, or `<owner>/<repo>#<n>`.
        issue: String,
        /// How GitHub merges: squash (default), merge or rebase.
        #[arg(long, default_value = "squash")]
        method: String,
        /// A CI run that is part of the evidence (spec/ci#which-ci-runs-count); see `done --run`.
        #[arg(long = "run")]
        runs: Vec<String>,
        #[command(flatten)]
        sel: Sel,
    },
    /// Close a ticket (Done is final).
    ///
    /// Coordinator: close a ticket (Done is final). Without a flag crewboard looks for the owner's
    /// merge of the PR; `--why` closes by your judgment; `--verified` closes after a passed verify.
    #[command(display_order = 19)]
    Close {
        /// The ticket: an issue number, or `<owner>/<repo>#<n>`.
        issue: String,
        /// Confirm that a passed `verify` exists for this ticket (Review -> Done).
        #[arg(long, conflicts_with = "why")]
        verified: bool,
        /// Close by your judgment when no owner merge exists (no PR, or the ticket will not be
        /// done). The text goes into the note on the ticket; name a replacing ticket in it.
        #[arg(long, value_name = "TEXT")]
        why: Option<String>,
        #[command(flatten)]
        sel: Sel,
    },
    /// Post a project status update.
    ///
    /// Coordinator: post a project status update (first line `from=<agent>`).
    #[command(display_order = 21)]
    ReportStatus {
        /// on-track, at-risk, off-track, inactive or complete.
        #[arg(long)]
        health: String,
        /// The status text.
        #[arg(long)]
        body: Option<String>,
        /// File with the text (`-` reads standard input).
        #[arg(long)]
        body_file: Option<String>,
        #[command(flatten)]
        sel: Sel,
    },
    /// Verify and merge CI-only tickets in Review.
    ///
    /// Headless coordinator: verify, and merge when allowed, every Review ticket whose Gate
    /// is CI only. Needs no event. Refused in a config dir that is not headless.
    #[command(hide = true, display_order = 29)]
    Tick {
        #[command(flatten)]
        sel: Sel,
    },
}

impl ReviewCmd {
    /// The command name as typed.
    pub fn name(&self) -> &'static str {
        match self {
            ReviewCmd::Verify { .. } => "verify",
            ReviewCmd::Merge { .. } => "merge",
            ReviewCmd::Close { .. } => "close",
            ReviewCmd::ReportStatus { .. } => "report-status",
            ReviewCmd::Tick { .. } => "tick",
        }
    }
}

/// The commands of the group `watch`: reading events and the board.
#[derive(Debug, Subcommand)]
pub enum WatchCmd {
    /// Wait for events and print them (--loop: keep waiting).
    ///
    /// Agent: block until there are new events for this agent, print them, exit. With `--loop`
    /// it keeps waiting, as a silent loop for a Monitor tool: it prints nothing while there are no
    /// events, prints the events once and exits 0 when there are some, and on an error prints ONE
    /// line `crewboard-watch: exit <code>: <reason> (<status>)` and exits with that code (2, 3 or
    /// 4). `--loop` is the old command `monitor`, which stays callable for ever. With `--loop`,
    /// `--json` prints the events and that last line as JSON.
    #[command(display_order = 4)]
    Watch {
        /// How long to wait (default: `timeout` of the `[loop]` table of the agent config, else 30m).
        #[arg(long)]
        timeout: Option<String>,
        /// Time between polls (default: `interval` of `[loop]`, else 60s).
        #[arg(long)]
        interval: Option<String>,
        /// Print the events as JSON.
        #[arg(long)]
        json: bool,
        /// Replace a damaged cursor with "now".
        #[arg(long)]
        reset_cursor: bool,
        /// Keep waiting: run `watch` again and again until there are events or an error.
        #[arg(long = "loop")]
        watch_loop: bool,
        /// With `--loop`: wait after a cycle without events, in seconds (default 2).
        #[arg(long, requires = "watch_loop")]
        pause: Option<String>,
        /// With `--loop`: stop silently with exit 0 after this many cycles without events
        /// (default: never).
        #[arg(long, requires = "watch_loop")]
        max_cycles: Option<u64>,
        /// With `--loop`: append one line per cycle to this file (never to standard output).
        #[arg(long, requires = "watch_loop")]
        log: Option<PathBuf>,
        #[command(flatten)]
        sel: Sel,
    },
    /// One poll for a scheduler.
    ///
    /// Scheduler: one poll. Start the model only on exit 0.
    #[command(hide = true, display_order = 5)]
    Poll {
        /// Only report whether events are pending (exit 0 / 1 / >=2). Prints no events and
        /// does not move the cursor.
        #[arg(long)]
        quiet: bool,
        /// Print the events as JSON.
        #[arg(long)]
        json: bool,
        /// Replace a damaged cursor with "now".
        #[arg(long)]
        reset_cursor: bool,
        #[command(flatten)]
        sel: Sel,
    },
    /// Silent watch loop for a Monitor tool.
    ///
    /// Agent: a silent loop around `watch` for a Monitor tool. It prints nothing while there are
    /// no events, prints the events once and exits 0 when there are some, and on an error prints
    /// ONE line `crewboard-watch: exit <code>: <reason> (<status>)` and exits with that code (2,
    /// 3 or 4). Arguments after `--` go to `watch`. `watch --loop` is the same command. The name
    /// `monitor` stays for ever, because Monitor tool configs call it.
    #[command(hide = true, display_order = 6)]
    Monitor {
        #[command(flatten)]
        m: MonitorArgs,
    },
    /// Show the board, the agents and tickets that need care.
    ///
    /// Any: board summary, agents with derived liveness, stale tickets, foreign comments. A
    /// ticket in Review shows how long it has waited since the latest `done`; over the
    /// `verify` time of the late-work table (30 minutes by default) it reads `waiting for
    /// verify since <t>`; a ticket that passed `verify` on the current PR head reads `verified,
    /// waiting for merge since <t>`. `--json` has `review_wait_s`, `review_done_at`,
    /// `waiting_for_verify` and `verified_waiting_since`.
    /// `--agents` lists the active agents instead (fewest open tickets first); `--can` keeps the
    /// ones that meet a requirement.
    #[command(display_order = 26)]
    Status {
        /// Print the status (with `--agents`: the agent list) as JSON.
        #[arg(long)]
        json: bool,
        /// List each foreign comment (issue, id, URL, author, time, reason) in the text output.
        /// `--json` always lists them.
        #[arg(long)]
        verbose: bool,
        /// Only the queues: per dev agent In Progress, then Ready in order, with counts and a
        /// note when one agent has 3 or more Ready tickets than another that can take the
        /// same work. With `--json` the full status gets a `queue` key.
        #[arg(long, conflicts_with = "agents")]
        queue: bool,
        /// List the active agents that meet the `--can` requirements, fewest open tickets first,
        /// instead of the board.
        #[arg(long, conflicts_with = "verbose")]
        agents: bool,
        /// With `--agents`: a requirement (repeatable, or comma separated): a declared tag, a bare
        /// architecture (`x86_64`, `arm64`), or `arch:`, `os:`, `tool:`, `cpu:`, `ram:`, `tier:`
        /// plus a value.
        #[arg(long = "can", requires = "agents")]
        can: Vec<String>,
        #[command(flatten)]
        sel: Sel,
    },
    /// List your tickets and what each one needs.
    ///
    /// Dev: your In Progress and Ready tickets, best first, and what each owned ticket needs
    /// now. Exit 0: a ticket needs you now (In Progress without running CI, red or green CI,
    /// no PR yet, a failed verify, an answer after a block or a question, or a Ready or parked
    /// ticket while nothing is In Progress). Exit 1: nothing to do now, every owned ticket
    /// waits (CI of the PR head still runs, no answer yet, or Review), or you own none. An In
    /// Progress ticket whose PR head is behind its base shows a `hint:` line (`behind main by
    /// N commits; merge main before done`). The hint is advice and never changes the exit code.
    #[command(display_order = 27)]
    Mine {
        /// Print nothing; only the exit code tells (0 work to do now, 1 nothing to do now),
        /// for `crewboard wake`.
        #[arg(long, conflicts_with = "json")]
        quiet: bool,
        /// Print one JSON object instead of the text: `agent`, `rows`, `now`, `needs_agent` and
        /// `first_ticket`. The exit code is the same.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        sel: Sel,
    },
    /// Internal, for `crewboard wake`: read an events file and print the events that
    /// need an action and that the agent has not acted on since `--since`, one line each
    /// (`<repo>#<n> <kind> again|twice`). `--list` prints each event of the file and reads nothing.
    #[command(hide = true)]
    #[command(display_order = 28)]
    Acted {
        /// The events file of the wake.
        #[arg(long)]
        events_file: PathBuf,
        /// Start of the wake, `YYYY-MM-DDTHH:MM:SSZ`: a message before it is no act.
        #[arg(long)]
        since: Option<String>,
        /// Print `<repo>#<n> <kind>` for every event of the file (the receipts); no board read.
        #[arg(long)]
        list: bool,
        /// Append the events that go to the next wake (with a marker line) to this file.
        #[arg(long)]
        redeliver_to: Option<PathBuf>,
        #[command(flatten)]
        sel: Sel,
    },
}

impl WatchCmd {
    /// The command name as typed.
    pub fn name(&self) -> &'static str {
        match self {
            WatchCmd::Watch { .. } => "watch",
            WatchCmd::Poll { .. } => "poll",
            WatchCmd::Monitor { .. } => "monitor",
            WatchCmd::Status { .. } => "status",
            WatchCmd::Mine { .. } => "mine",
            WatchCmd::Acted { .. } => "acted",
        }
    }
}

/// The commands of the group `host`: the host: doctor, lock, gate, skill, update, wake, hook and agent setup.
#[derive(Debug, Subcommand)]
pub enum HostCmd {
    /// Check this host, an agent, the board or a repo move.
    ///
    /// Any: a local check of the headless scope. Prints whether the `headless` key of the config
    /// dir (a guardrail) is on, and a note for a leftover system scope file that is not read any
    /// more. Reads no board. `--repo-move` is the read-only dry run before and after a repo
    /// transfer or rename.
    #[command(display_order = 36)]
    Doctor {
        /// Also read the board and warn about built-in project workflows that write Status behind
        /// crewboard (fix for the four that `init` deletes: `crewboard init`). A warning
        /// only: the exit stays the one of the local check.
        #[arg(long, conflicts_with_all = ["setup", "repo_move"])]
        board: bool,
        /// Dry run for a repo transfer or rename (read only): count the signed messages that
        /// verify under the current repo name and under each `former_repos` name of
        /// AUTHORITY.md, and list the ones that verify under no name. Exit 0 whatever it finds.
        #[arg(long = "repo-move", conflicts_with = "setup")]
        repo_move: bool,
        /// Agent: check the setup of this agent instead (token, push access, plain gh, git
        /// identity, runtime, board, AUTHORITY.md, approval). Read only. One line per check:
        /// ok, FAIL with the fix, or skip. Exit 2 if a check fails. Run it with the agent's
        /// environment: `crewboard agent exec --agent-dir <agent-dir> -- crewboard doctor --setup`.
        #[arg(long)]
        setup: bool,
        /// The checkout of the agent, for the git identity check (default: this directory).
        #[arg(long, requires = "setup")]
        work_dir: Option<PathBuf>,
        #[command(flatten)]
        sel: Sel,
    },
    /// Host lock for heavy jobs on a shared host.
    ///
    /// Any: the host lock of a shared host, so that heavy jobs (a full build, a stress gate) run one
    /// at a time and a dead agent cannot hold the lock forever. Subcommands: take, release, status,
    /// break, run, ci-wait, ci-done, ci-status. Exit codes: 0, 1, 2, 3, 64 (usage), 75 (held),
    /// 76 (CI runs or waits), 77 (held by your own parent process), or that of CMD for `run`.
    /// `crewboard lock --help` prints the full text. Unix only.
    #[command(disable_help_flag = true)]
    #[command(display_order = 37)]
    Lock {
        /// The subcommand, the lock directory and the options, for example
        /// `run <dir> --holder AGENT --wait 1h -- CMD...`. Read by `crewboard lock --help`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        args: Vec<String>,
    },
    /// Install the crewboard skill.
    ///
    /// Any: install the crewboard skill (its files are inside this binary) and link it for the
    /// agent runtimes, or print where it is installed. Reads no board. Unix only.
    #[command(display_order = 38)]
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Update or downgrade this binary.
    ///
    /// Any: update (or downgrade) this binary from a GitHub release or from a local directory,
    /// then run `skill install` with the new binary. Nothing changes on the host until the
    /// checksum, the tarball paths and `--version` of the new binary have passed. Keeps no
    /// backup: the way back is `--release <old tag>`. Exit codes: 0 (also "already installed" and
    /// "latest is older, not downgrading"), 2 (refused: no release for this platform, release
    /// missing or not readable, checksum, tarball or version check failed), 3 (a failed step:
    /// a write or the download), 4 (usage, or a token that GitHub refuses). When the binary is
    /// updated but `skill install` fails, the exit code is that of `skill install` (1: a SKIP,
    /// 3: a failed step).
    /// Needs a release for aarch64-apple-darwin, x86_64-apple-darwin, x86_64-unknown-linux-gnu or
    /// aarch64-unknown-linux-gnu. Token: `GH_TOKEN`, else `GITHUB_TOKEN`, else none; a private
    /// repo needs one with read access (`GH_TOKEN=$(gh auth token) crewboard update`).
    #[command(display_order = 39)]
    Update {
        /// Install this release (a tag `vX.Y.Z`) instead of the latest one. It may be older
        /// than the installed version: this is the rollback.
        #[arg(long, value_name = "vX.Y.Z", conflicts_with = "from", value_parser = ops::update_cmd::parse_tag)]
        release: Option<String>,
        /// Install from a local directory that holds `SHA256SUMS` and the tarball
        /// `crewboard-<version>-<target>.tar.gz` of this host (a downloaded release or build
        /// artifact). Uses no network.
        #[arg(long, value_name = "DIR")]
        from: Option<PathBuf>,
        /// The GitHub repository that holds the releases (for a fork). Not used with `--from`.
        #[arg(long, value_name = "OWNER/NAME", default_value = ops::update_cmd::DEFAULT_REPO, value_parser = ops::update_cmd::parse_repo)]
        repo: String,
        /// Print installed version, target, source and the version that would be installed
        /// (`plan:` lines), and change nothing. A release is looked up on GitHub; nothing is
        /// downloaded.
        #[arg(long)]
        check: bool,
    },
    /// Wake an unattended agent from a scheduler (`wake install`).
    ///
    /// One run polls the board and starts the wake command only when there is something to do;
    /// a quiet board starts no model. `wake install --agent-dir DIR` writes and enables the unit
    /// that runs it every few minutes; `wake uninstall` and `wake status` undo and show it.
    /// `agent add --install-wake` is the normal way to install it.
    ///
    /// Tuning options that work but are not listed below: --work-backoff, --retry-delays,
    /// --stale-after, --empty-grace, --state-dir, --lock-dir, --log, --events-file, --max-wake,
    /// --heartbeat-every, --print-state-dir, --shim-dir, --allow, --runtime-flags, --runtime-bin,
    /// --prompt-extra, --host-lock, --no-events-hook, --events-every and --print-cmd.
    #[command(args_conflicts_with_subcommands = true)]
    #[command(hide = true, display_order = 40)]
    Wake {
        #[command(flatten)]
        w: Box<crate::wake::cmd::WakeArgs>,
        #[command(subcommand)]
        action: Option<crate::wake::install::WakeAction>,
    },
    /// Internal: hooks that a runtime calls during a wake.
    #[command(hide = true)]
    #[command(display_order = 41)]
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
    /// Run a command N times on one commit and keep evidence.
    ///
    /// Any: run a command N times in a row (a flake check: the same command, the same commit) and
    /// keep honest evidence. Every run writes its own log, is judged by its own exit code and
    /// result lines, and must run on one commit. `crewboard repeat --help` prints the full text.
    #[command(display_order = 42, after_help = REPEAT_MORE)]
    Repeat(RepeatArgs),
    /// Set up and run one agent on this host.
    ///
    /// Any: run a command as one agent. `exec` runs CMD with the environment of an agent dir (the
    /// `gh` shim first on the PATH, an empty `gh` config dir, the config dir; no token, and the
    /// token variables of the caller are removed). `env` prints the same as `export` lines for
    /// `eval`. `with-env` runs CMD with the token of the env file `--env-file`. Unix only. `add` and
    /// `shim` set an agent up. `profile` shows or changes what the agent declares, `token` stores
    /// the token of the crew account on this host, and `selftest` proves that the agent sees events.
    #[command(display_order = 43)]
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
}

impl HostCmd {
    /// The command name as typed.
    pub fn name(&self) -> &'static str {
        match self {
            HostCmd::Doctor { .. } => "doctor",
            HostCmd::Lock { .. } => "lock",
            HostCmd::Skill { .. } => "skill",
            HostCmd::Update { .. } => "update",
            HostCmd::Wake { .. } => "wake",
            HostCmd::Hook { .. } => "hook",
            HostCmd::Repeat(_) => "repeat",
            HostCmd::Agent { action } => match action {
                AgentAction::Profile { .. } => "agent profile",
                AgentAction::Token { .. } => "agent token",
                AgentAction::Selftest { .. } => "agent selftest",
                _ => "agent",
            },
        }
    }
}

/// The subcommands of `hook`.
#[derive(Debug, Subcommand)]
pub enum HookAction {
    /// The PostToolUse hook of a Claude wake: poll the board at most once per interval and give
    /// new events to the model. Always exits 0; reads no standard input.
    PostToolUse(crate::wake::hook_cmd::PostToolUseArgs),
}

/// The actions of `crewboard skill`.
#[derive(Debug, Subcommand)]
pub enum SkillAction {
    /// Install the skill and link it for the runtimes.
    ///
    /// Copy the skill to `~/.config/agent-skills/crewboard/` and link it into the skills
    /// directory of each runtime (claude, codex, copilot, hermes) that is present: its skills
    /// directory or its config directory (`~/.claude`, ...) exists, or its binary is on `PATH`.
    /// A host without a runtime gets the shared copy only. It overwrites the
    /// skill files of that copy, removes the files that an earlier install wrote and this binary
    /// does not have, and never overwrites a real directory or a link that points elsewhere (the
    /// exit is then 1; a step that fails, such as a write, exits 3). Run it again after an upgrade: it is idempotent. Without a clone,
    /// `--link` stops with exit 4.
    Install {
        /// Link the skill into this runtime even if it is not found on the host, and create its
        /// skills directory if it is missing: claude, codex, copilot or hermes (repeatable).
        /// Without it, every runtime that is present gets a link.
        #[arg(long = "runtime", value_name = "NAME")]
        runtimes: Vec<String>,
        /// Make the shared path a symlink to the `skill/` directory of a clone (live edits, for
        /// development) instead of a copy. The clone is `--from`, else the first directory above
        /// the current one with `skill/SKILL.md` next to the `Cargo.toml` of crewboard.
        #[arg(long)]
        link: bool,
        /// With `--link`: the clone of crewboard (the directory that holds `skill/`).
        #[arg(long, requires = "link", value_name = "DIR")]
        from: Option<PathBuf>,
        /// Print what would happen and change nothing.
        #[arg(long, conflicts_with = "apply")]
        dry_run: bool,
        /// Do it (the default; kept so that `install --apply` of the old script still works).
        #[arg(long, hide = true)]
        apply: bool,
    },
    /// Print the directory of the shared copy (`~/.config/agent-skills/crewboard`).
    Path,
}

/// The subcommands of `agent`.
// `Add` has many options and one value is built per run: a box would only hide the options.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub enum AgentAction {
    /// Show or change what an agent declares.
    ///
    /// Agent or owner: the profile (declared tags, tier, background text) of an agent. `show`
    /// prints it with the detected facts and the recent changes; `set` changes it.
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
    /// Store the token of the crew account.
    ///
    /// Any: the token of the crew account goes into the env file of the initiative, without a
    /// text editor and without a command line that holds the token. It reads the token from
    /// standard input (typed without echo on a terminal, or piped), asks GitHub whose token it
    /// is and which scopes it has, and only then writes `~/.config/crewboard/<initiative>.env`
    /// (mode 600, by a rename; a dir that crewboard creates gets mode 700, an existing dir keeps
    /// its mode). A token without the scopes `repo` and `project`, or with `workflow`, is refused
    /// and nothing is written. The token is never printed. Exit 2 for a refused token, 3 for a network failure, 4 for a bad input.
    Token {
        /// The initiative: it names the env file `~/.config/crewboard/<initiative>.env`
        /// (default: the only initiative in the config dir).
        #[arg(long)]
        initiative: Option<String>,
        /// Refuse a token that belongs to another account than this login.
        #[arg(long)]
        crew_login: Option<String>,
        /// A one-person trial (one account for the owner and the agents): the token is the
        /// owner's, so the `workflow` scope passes with a note. A crew still refuses it.
        #[arg(long, conflicts_with = "crew_login")]
        insecure_one_account: bool,
    },
    /// Check that this agent can see events.
    ///
    /// Any: prove on the live board that this agent can see events. `start` runs it once; run it
    /// by hand to debug liveness.
    Selftest {
        #[command(flatten)]
        sel: Sel,
    },
    /// Run CMD with the environment of the agent dir.
    ///
    /// Replace this process by CMD, run with the environment of the agent dir.
    Exec {
        /// The config dir of the agent (default: `--config-dir`, else $CREWBOARD_CONFIG_DIR).
        #[arg(long)]
        agent_dir: Option<PathBuf>,
        /// The command and its arguments, after `--`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true, num_args = 1..)]
        cmd: Vec<String>,
    },
    /// Print export lines for the agent shell.
    ///
    /// Print `export` lines for the agent shell, never a token: `eval "$(crewboard agent env)"`.
    Env {
        /// The config dir of the agent (default: `--config-dir`, else $CREWBOARD_CONFIG_DIR).
        #[arg(long)]
        agent_dir: Option<PathBuf>,
    },
    /// Run CMD with the token of --env-file.
    ///
    /// Replace this process by CMD, run with the token and the settings of `--env-file`.
    WithEnv {
        /// The command and its arguments, after `--`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true, num_args = 1..)]
        cmd: Vec<String>,
    },
    /// Set up one agent in one command.
    ///
    /// Owner of the agent host: set one agent up in one command (the agent dir, `join`, the `gh`
    /// shim, the crew id, the work dir, its git identity and `start`). A rerun is safe: a step
    /// that is done says so. `--env-file` is the env file of the crew account. With `--role
    /// headless-coordinator --from <dir>` it makes the config dir of the headless tick from the
    /// dir of the interactive coordinator.
    Add {
        /// The repository `<owner>/<repo>` (its `.crewboard.toml` is the board config), or the
        /// path of a board file (`.toml`).
        #[arg(value_name = "REPO|BOARD-FILE")]
        target: Option<String>,
        /// The board file, when the argument is a repository.
        #[arg(long)]
        board_file: Option<PathBuf>,
        /// The repository to clone, when the argument is a board file (default: the main repo of
        /// the board file).
        #[arg(long)]
        repo: Option<String>,
        /// The agent dir (default: `--config-dir`, else ~/.config/crewboard/agents/<initiative>-<runtime>).
        #[arg(long)]
        agent_dir: Option<PathBuf>,
        /// The agent name (default: <runtime>@<short host name>).
        #[arg(long)]
        agent: Option<String>,
        /// claude, codex or hermes (default: the first of them on the PATH).
        #[arg(long, value_enum)]
        runtime: Option<ops::agent::AgentRuntime>,
        /// The checkout of the repo (default: ~/crew/<initiative>-<agent dir name>).
        #[arg(long)]
        work_dir: Option<PathBuf>,
        /// The login of the crew account of this agent: the shared one or your own (default: the
        /// login of the token).
        #[arg(long)]
        crew_login: Option<String>,
        /// The model, passed to `join --model`: the runtime label of the join request is
        /// `claude-code/<model>` for the runtime claude, else `codex/<model>` or `hermes/<model>`.
        #[arg(long)]
        model: Option<String>,
        /// The initiative, for the default names (default: that of the board config).
        #[arg(long)]
        initiative: Option<String>,
        /// dev (default), coordinator or headless-coordinator.
        #[arg(long, value_enum)]
        role: Option<ops::agent::AddRole>,
        /// With `--role headless-coordinator`: the agent dir of the interactive coordinator.
        #[arg(long)]
        from: Option<PathBuf>,
        /// After the join, print the exact `crewboard approve` line for the owner and wait for
        /// the approval (a check every 15 seconds, for up to 30 minutes; Ctrl-C is safe), then
        /// run `start`. Without it the command stops after the join and you run it again.
        #[arg(long)]
        wait_approval: bool,
        /// Test seam: seconds between two checks of `--wait-approval`.
        #[arg(long, hide = true, requires = "wait_approval")]
        wait_interval: Option<u64>,
        /// Test seam: the longest wait of `--wait-approval`, in seconds.
        #[arg(long, hide = true, requires = "wait_approval")]
        wait_max: Option<u64>,
        /// Read the token of the crew account from standard input (typed without echo, or piped
        /// from the first host), check it and write the env file first. The env file is
        /// `--env-file`, else `~/.config/crewboard/<initiative>.env` (then `--initiative` is
        /// needed). It replaces `agent token` and a copy of the file for a second host.
        #[arg(long)]
        token_stdin: bool,
        /// With `--token-stdin`: a one-person trial (one account for the owner and the agents).
        /// The token is the owner's, so the `workflow` scope passes with a note. A crew still
        /// refuses it.
        #[arg(long, requires = "token_stdin", conflicts_with = "crew_login")]
        insecure_one_account: bool,
        /// A dev agent: after a good `start` (and after `--wait-approval`), install and enable the
        /// scheduler unit with the code of `crewboard wake install`, with its defaults. Prints
        /// the status line and, if you must act, the `next:` line (on Linux the `loginctl
        /// enable-linger` command). Refuses, as `wake install` does, when a job of another name
        /// already wakes this agent: its fix line names `--replace` of `wake install`.
        #[arg(long)]
        install_wake: bool,
        /// With `--install-wake`: auto (default), launchd, systemd or cron.
        #[arg(long, value_enum, requires = "install_wake")]
        scheduler: Option<ops::agent::AddScheduler>,
        /// With `--install-wake`: the interval between two runs, for example 5m (default), 90s, 1h.
        #[arg(long, requires = "install_wake")]
        every: Option<String>,
        /// With `--install-wake`: one more allowed tool of the wake, as `wake install --allow`
        /// takes it (claude only). Repeat it for more.
        #[arg(long, requires = "install_wake")]
        allow: Vec<String>,
        /// Test seam: run `join` and `start`, and ask for the crew id, through this binary.
        #[arg(long, hide = true)]
        crewboard: Option<String>,
    },
    /// Write the gh stub of an agent dir again.
    ///
    /// Owner of the agent host: write the `gh` stub of an agent dir again (and, with `--work-dir`,
    /// the git identity of that checkout). The stub runs the real `gh` as the crew account.
    Shim {
        /// The agent dir: this option, else `--config-dir`, else a non-empty $CREWBOARD_CONFIG_DIR.
        /// There is no home default: with none of these the command exits 4.
        #[arg(long)]
        agent_dir: Option<PathBuf>,
        /// Also set the git identity of this checkout.
        #[arg(long)]
        work_dir: Option<PathBuf>,
        /// The real `gh` (default: the first `gh` on the PATH that is not the stub).
        #[arg(long)]
        real_gh: Option<PathBuf>,
        /// The login of the crew account (default: the `crew-id` file of the agent dir).
        #[arg(long)]
        crew_login: Option<String>,
        /// The dir of the stub (default: `<agent dir>/bin`).
        #[arg(long, hide = true)]
        bin_dir: Option<PathBuf>,
        /// The e-mail of the git identity.
        #[arg(long, hide = true)]
        email: Option<String>,
        /// Set only the git identity, no stub.
        #[arg(long, hide = true)]
        only_git: bool,
    },
}

/// The options of `monitor`. A flag that comes twice counts once, the last one: a caller adds
/// its flags after those of the re-arm command, and they win.
#[derive(Debug, Args, Clone)]
#[command(args_override_self = true)]
pub struct MonitorArgs {
    /// How long one `watch` waits (default: `timeout` of the `[loop]` table, else 30m).
    #[arg(long)]
    pub timeout: Option<String>,
    /// Time between polls inside one `watch` (default: `interval` of `[loop]`, else 60s).
    #[arg(long)]
    pub interval: Option<String>,
    /// Wait after a cycle without events, in seconds (default 2).
    #[arg(long)]
    pub pause: Option<String>,
    /// Stop silently with exit 0 after this many cycles without events (default 0 = never).
    #[arg(long, default_value_t = 0)]
    pub max_cycles: u64,
    /// Append one line per cycle to this file (never to standard output).
    #[arg(long)]
    pub log: Option<PathBuf>,
    /// Print the final error line as JSON: `{"monitor":"exit","code":..,"reason":"..","status":".."}`.
    #[arg(long)]
    pub json: bool,
    /// Replace a damaged cursor with "now".
    #[arg(long)]
    pub reset_cursor: bool,
    /// Test seam: run `watch` of this binary instead of in this process.
    #[arg(long, hide = true)]
    pub crewboard: Option<String>,
    #[command(flatten)]
    pub sel: Sel,
    /// Arguments for `watch`, after `--`.
    #[arg(last = true)]
    pub watch_args: Vec<String>,
}

/// The text of `repeat --help` after the options (the examples are tested).
const REPEAT_MORE: &str = "A run is green only if the command exits 0. With --result-re it must also have a result line in its \
own log, and no result line may match --fail-re. A missing result line is red. The head commit \
(git rev-parse HEAD) is read before and after every run; a change ends the loop with exit 2. The \
loop stops at the first red run. The command runs without a shell, in --work-dir.

Result lines of common runners (--result-re). The command is the one your repo uses; the exit code \
alone is enough for any runner, and --result-re only adds proof that tests ran:
  pytest:   --result-re '^=+ .*(passed|failed)'  (===== 5 passed in 0.12s =====)
  Jest:     --result-re '^Tests: '               (Tests: 1 failed, 4 passed, 5 total)
  Vitest:   --result-re '^\\s*Tests\\s'            (     Tests  5 passed (5))
  go test:  --result-re '^(ok|FAIL)\\s'           (ok  example.org/app  0.12s)
  Maven:    --result-re '^Tests run:'            (Tests run: 12, Failures: 0, Errors: 0)
  Gradle:   --result-re '^BUILD (SUCCESSFUL|FAILED)'
  xcodebuild: --result-re '^\\*\\* TEST (SUCCEEDED|FAILED) \\*\\*'
  cargo:    --result-re '^test result: '

Before each run `repeat` waits while a CI job runs or waits on this host (the host lock check).

Exit codes: 0 all N runs green on one commit; 1 a red run; 2 the commit changed (or --lock on a \
host without Unix process control); 3 CI did not finish within --ci-max-wait; 4 setup error (not a \
git checkout, cannot write the logs); 64 usage; 143 a TERM, INT or HUP stopped `repeat` (the \
command got it too, the lock is released).";

/// The options of `repeat`.
#[derive(Debug, Args)]
pub struct RepeatArgs {
    /// Number of runs in a row (a whole number, at least 1).
    #[arg(long = "times")]
    pub times: Option<String>,
    /// Take the host lock in this directory around EACH run.
    #[arg(long)]
    pub lock: Option<String>,
    /// The lock holder (default `$CREWBOARD_AGENT`, else `gate-loop`).
    #[arg(long = "holder")]
    pub holder: Option<String>,
    /// How long to wait for a live lock per run (default 3600 seconds; 90, 90s, 30m, 2h).
    #[arg(long, default_value = "3600")]
    pub lock_wait: String,
    /// Where `run-<n>.log` go (default: a new temp directory). Old files of that name are
    /// removed before the run, so they can never count.
    #[arg(long)]
    pub log_dir: Option<PathBuf>,
    /// The git checkout whose HEAD is recorded; CMD runs there (default: this directory).
    #[arg(long = "work-dir")]
    pub work_dir: Option<PathBuf>,
    /// Regular expression (the `regex` crate, per line) of a result line of the runner.
    #[arg(long)]
    pub result_re: Option<String>,
    /// Regular expression that makes a result line red (default: `FAILED` or a non-zero
    /// failed count). Used only with --result-re.
    #[arg(long)]
    pub fail_re: Option<String>,
    /// Do not look for CI before each run.
    #[arg(long)]
    pub no_ci_check: bool,
    /// Seconds between two CI checks while paused (default 10).
    #[arg(long, default_value = "10")]
    pub ci_poll: String,
    /// The longest pause for CI per run (default 3600 seconds). After that: exit 3.
    #[arg(long, default_value = "3600")]
    pub ci_max_wait: String,
    /// The command of one run, after `--`.
    #[arg(last = true)]
    pub cmd: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum TicketAction {
    /// Create a work ticket and put it on the board.
    ///
    /// Create the `crew:work` issue as the crew account, check its `Gate:` line, put it on the
    /// board (Kind Work, Status Backlog, Gate) and write the signed info comment, so that
    /// `status` lists it. Default: park it in Backlog.
    New {
        /// Title of the new issue.
        #[arg(long, required_unless_present = "template", default_value = "")]
        title: String,
        /// File with the ticket text (`-` reads standard input). It needs a `Gate:` line.
        #[arg(long, required_unless_present = "template", default_value = "")]
        body_file: String,
        /// Print the body template (the lines of a ticket body) and stop. It reads no config and
        /// calls no board.
        #[arg(long, conflicts_with_all = ["assign", "continue_pr", "backlog"])]
        template: bool,
        /// Repo of the issue (default: the main repo of the initiative).
        #[arg(long)]
        repo: Option<String>,
        /// Park the ticket in Backlog (the default).
        #[arg(long, conflicts_with = "assign")]
        backlog: bool,
        /// Assign it to this agent at once (the checks of `assign` run before anything is created).
        #[arg(long)]
        assign: Option<String>,
        /// Continue the OPEN PR of a Done ticket (a PR URL), or the open PR that the reassign notice
        /// of a ticket in Backlog or Ready names: `done` then accepts its branch for this ticket.
        /// See spec/tickets#a-follow-up-ticket-continues-an-open-pr.
        #[arg(long = "continue-pr")]
        continue_pr: Option<String>,
        /// A ticket whose Gate is CI only: write the Gate line `Gate: ci`. The headless `tick`
        /// merges a ticket only when its signed Gate is CI only (`ci`); this flag writes it.
        /// Leave the `Gate:` line out of the body (a body with another Gate is refused).
        #[arg(long, conflicts_with = "template")]
        gate_ci: bool,
        #[command(flatten)]
        sel: Sel,
    },
    /// Put an existing issue on the board.
    ///
    /// Put an existing issue (label `crew:work`, a `Gate:` line) on the board in the same way.
    /// A ticket that is on the board already keeps its Status.
    Adopt {
        /// The ticket: an issue number, or `<owner>/<repo>#<n>`.
        issue: String,
        /// Assign it to this agent after it is on the board.
        #[arg(long)]
        assign: Option<String>,
        #[command(flatten)]
        sel: Sel,
    },
    /// Move the open part to a new ticket.
    ///
    /// Move the open part of an In Progress ticket to a new ticket for the same agent, so that
    /// the finished part can merge. Links both ways; one signed info note on the old ticket.
    /// A second identical split changes nothing. Refused if AUTHORITY.md has
    /// `coordinator_may_split_tickets: false`.
    Split {
        /// The ticket: an issue number, or `<owner>/<repo>#<n>`.
        issue: String,
        /// Title of the new ticket.
        #[arg(long)]
        title: String,
        /// File with the text of the new ticket (`-` reads standard input). The `Gate:` line of
        /// the old ticket is copied unless the file has its own.
        #[arg(long)]
        body_file: String,
        #[command(flatten)]
        sel: Sel,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProfileAction {
    /// Change the declared profile.
    ///
    /// Change the declared profile. The agent changes its own; the owner uses `--agent <name>`
    /// with the owner token. Every change is recorded in the registry issue.
    Set {
        /// Add a capability tag (`[a-z0-9-]`, for example x86-builds). Repeatable.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Remove a capability tag. Repeatable.
        #[arg(long = "untag")]
        untags: Vec<String>,
        /// The model or cost tier, written like a tag (low, mid, high sort cheapest first).
        #[arg(long, conflicts_with = "clear_tier")]
        tier: Option<String>,
        /// Remove the tier.
        #[arg(long)]
        clear_tier: bool,
        /// File with the background text (`-` reads standard input). An empty file clears it.
        #[arg(long)]
        about_file: Option<String>,
        /// Owner only: the agent whose profile is changed.
        #[arg(long)]
        agent: Option<String>,
        #[command(flatten)]
        sel: Sel,
    },
    /// Show the profile, the detected facts and the recent changes.
    Show {
        /// The agent to show (default: this agent).
        #[arg(long)]
        agent: Option<String>,
        /// Print the profile as JSON.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        sel: Sel,
    },
}

impl Command {
    /// The command name as typed.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Crew(c) => c.name(),
            Command::Dev(c) => c.name(),
            Command::Plan(c) => c.name(),
            Command::Review(c) => c.name(),
            Command::Watch(c) => c.name(),
            Command::Host(c) => c.name(),
        }
    }

    /// `watch` and `poll` use the exit codes in their own way (spec/cli#exit-codes).
    pub fn family(&self) -> Family {
        match self {
            // `mine` uses exit 1 for "nothing", like `poll` (spec/cli#exit-codes)
            Command::Watch(
                WatchCmd::Watch { .. }
                | WatchCmd::Monitor { .. }
                | WatchCmd::Poll { .. }
                | WatchCmd::Mine { .. },
            ) => Family::Watch,
            Command::Host(HostCmd::Lock { .. }) => Family::Lock,
            Command::Host(HostCmd::Wake { .. }) => Family::Wake,
            Command::Host(HostCmd::Repeat(_)) => Family::Gate,
            _ => Family::Other,
        }
    }
}
