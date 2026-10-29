//! All tests against the fake GitHub (`tests/support`), linked as ONE test binary. Each
//! `component/fake_*.rs` file is a module: one behaviour group each. A test starts the real
//! `crewboard` binary against its own `FakeBoard` (own port, own temp dirs), so the tests of
//! all modules run in parallel in one process without shared state. Nothing here changes the
//! process environment.
//!
//! Run one group with `cargo test --test component fake_tickets::`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::disallowed_methods,
    reason = "a test fails by panic and waits for real processes"
)]

mod support;

#[path = "component/agent_add.rs"]
mod agent_add;
#[path = "component/agent_env.rs"]
mod agent_env;
#[path = "component/agent_exec.rs"]
mod agent_exec;
#[path = "component/agent_shim.rs"]
mod agent_shim;
#[path = "component/crew_token.rs"]
mod crew_token;
#[path = "component/fake_accounts.rs"]
mod fake_accounts;
#[path = "component/fake_acted.rs"]
mod fake_acted;
#[path = "component/fake_adopt_pr.rs"]
mod fake_adopt_pr;
#[path = "component/fake_agent_signed.rs"]
mod fake_agent_signed;
#[path = "component/fake_audit_fields.rs"]
mod fake_audit_fields;
#[path = "component/fake_board.rs"]
mod fake_board;
#[path = "component/fake_board_config.rs"]
mod fake_board_config;
#[path = "component/fake_branch_guard.rs"]
mod fake_branch_guard;
#[path = "component/fake_builtin_workflows.rs"]
mod fake_builtin_workflows;
#[path = "component/fake_capabilities.rs"]
mod fake_capabilities;
#[path = "component/fake_ci_finished.rs"]
mod fake_ci_finished;
#[path = "component/fake_ci_ignore.rs"]
mod fake_ci_ignore;
#[path = "component/fake_close.rs"]
mod fake_close;
#[path = "component/fake_deleted_note.rs"]
mod fake_deleted_note;
#[path = "component/fake_doctor_setup.rs"]
mod fake_doctor_setup;
#[path = "component/fake_done_events.rs"]
mod fake_done_events;
#[path = "component/fake_earlier_work.rs"]
mod fake_earlier_work;
#[path = "component/fake_edited_owner.rs"]
mod fake_edited_owner;
#[path = "component/fake_free_text.rs"]
mod fake_free_text;
#[path = "component/fake_handover.rs"]
mod fake_handover;
#[path = "component/fake_headless.rs"]
mod fake_headless;
#[path = "component/fake_headless_cursor.rs"]
mod fake_headless_cursor;
#[path = "component/fake_init.rs"]
mod fake_init;
#[path = "component/fake_join_ambiguous.rs"]
mod fake_join_ambiguous;
#[path = "component/fake_late.rs"]
mod fake_late;
#[path = "component/fake_leave.rs"]
mod fake_leave;
#[path = "component/fake_local_gate.rs"]
mod fake_local_gate;
#[path = "component/fake_log_check.rs"]
mod fake_log_check;
#[path = "component/fake_merge_first.rs"]
mod fake_merge_first;
#[path = "component/fake_merged_without_verify.rs"]
mod fake_merged_without_verify;
#[path = "component/fake_mine.rs"]
mod fake_mine;
#[path = "component/fake_monitor.rs"]
mod fake_monitor;
#[path = "component/fake_named_runs.rs"]
mod fake_named_runs;
#[path = "component/fake_needs_owner_label.rs"]
mod fake_needs_owner_label;
#[path = "component/fake_needs_owner_sync.rs"]
mod fake_needs_owner_sync;
#[path = "component/fake_network.rs"]
mod fake_network;
#[path = "component/fake_one_account.rs"]
mod fake_one_account;
#[path = "component/fake_option_cap.rs"]
mod fake_option_cap;
#[path = "component/fake_option_race.rs"]
mod fake_option_race;
#[path = "component/fake_owner_move.rs"]
mod fake_owner_move;
#[path = "component/fake_owner_question.rs"]
mod fake_owner_question;
#[path = "component/fake_owner_route.rs"]
mod fake_owner_route;
#[path = "component/fake_paths_ignored.rs"]
mod fake_paths_ignored;
#[path = "component/fake_pr_already_merged.rs"]
mod fake_pr_already_merged;
#[path = "component/fake_protected_paths.rs"]
mod fake_protected_paths;
#[path = "component/fake_queue.rs"]
mod fake_queue;
#[path = "component/fake_reassign_stop.rs"]
mod fake_reassign_stop;
#[path = "component/fake_rejoin.rs"]
mod fake_rejoin;
#[path = "component/fake_replay.rs"]
mod fake_replay;
#[path = "component/fake_repo_transfer.rs"]
mod fake_repo_transfer;
#[path = "component/fake_request_budget.rs"]
mod fake_request_budget;
#[path = "component/fake_signed_ci.rs"]
mod fake_signed_ci;
#[path = "component/fake_split.rs"]
mod fake_split;
#[path = "component/fake_stale_escalate.rs"]
mod fake_stale_escalate;
#[path = "component/fake_stale_work.rs"]
mod fake_stale_work;
#[path = "component/fake_status_guard.rs"]
mod fake_status_guard;
#[path = "component/fake_text_limit.rs"]
mod fake_text_limit;
#[path = "component/fake_tickets.rs"]
mod fake_tickets;
#[path = "component/fake_verify_fail.rs"]
mod fake_verify_fail;
#[cfg(unix)]
#[path = "component/fake_wake.rs"]
mod fake_wake;
#[cfg(unix)]
#[path = "component/lock_board.rs"]
mod lock_board;
#[cfg(unix)]
#[path = "component/lock_ci.rs"]
mod lock_ci;
#[cfg(unix)]
#[path = "component/lock_cli.rs"]
mod lock_cli;
#[cfg(unix)]
#[path = "component/lock_detach.rs"]
mod lock_detach;
#[path = "component/lock_platform.rs"]
mod lock_platform;
#[cfg(unix)]
#[path = "component/lock_reentry.rs"]
mod lock_reentry;
#[cfg(unix)]
#[path = "component/monitor_term.rs"]
mod monitor_term;
#[path = "component/owner_token.rs"]
mod owner_token;
#[path = "component/request_budget.rs"]
mod request_budget;
#[cfg(unix)]
#[path = "component/wake_budget.rs"]
mod wake_budget;

#[cfg(unix)]
#[path = "component/wake_install.rs"]
mod wake_install;
#[path = "component/wake_lock.rs"]
mod wake_lock;
