# The port of the shell suites

Part of the [design pages](README.md).

Two bash suites tested the binary through thin driver scripts: `skill/loops/tests/test-loops.sh`
(`crewboard monitor`, `wake`, `hook`, the `agent` commands) and `skill/loops/tests/test-locks.sh`
(`crewboard lock`, `crewboard repeat`). The shipped shell scripts were gone; only these suites and
their drivers (`agent-wake.sh`, `poll-and-wake.sh`, `host-lock.sh`, `gate-loop.sh`) were left.
They are retired. This page maps every case of the two suites, by its id, to the Rust tests that
make the same observable assertion (the same exit code, output, log line, file or process
state), not only the same command.

Status:

- **existing**: Rust tests that already made every assertion of the case.
- **new**: a Rust test written for the port; the other tests in the row cover the rest.
- **new assertion**: the missing assertion was added to the Rust test in the row (the note says
  which).
- **obsolete**: the case tested a driver script or the fake of the suite, not the binary.

A note **unit level** means that only unit tests (`src/...`) make the assertions: the shell case
ran the binary, the Rust tests run the same code in process with fakes (a scripted board, a fake
clock or process table). The status says whether those tests make every assertion of the case.

Count by case id (a case that ran once per runtime or per value is one id): `test-loops.sh` 152
ids, `test-locks.sh` 144 ids. Of the 296: 200 existing, 70 new, 16 new assertion, 10 obsolete.

Paths: `component/x` is `tests/component/x.rs`, `tests/x` is `tests/x.rs`, `src/...` is the unit
test module of that file (`src/wake/cmd_tests/*` are the tests of `src/wake/cmd.rs`).

The fake `crewboard` of the suites reached the binary through the hidden test option
`--crewboard BIN` (`wake`, `hook`, `lock`, `monitor`, `agent add`) and the variables
`HOST_LOCK_PS_CMD`, `HOST_LOCK_PARENT_CMD`, `HOST_LOCK_BOARD_TTL`, `HOST_LOCK_HOST`,
`CREWBOARD_WAKE_OS` and `CREWBOARD_RETRY_DELAYS`. Most Rust tests replace them with the fake
GitHub server of `tests/support` or with fakes of the `Steps`, `Cycle`, clock and process traits.

## The test seams after the port

| Seam | Used by the Rust tests | Now |
|---|---|---|
| `lock --crewboard BIN` | `component/agent_env`, the probe tests of `src/ops/lock_cmd` | read only by a build with debug assertions |
| `monitor --crewboard BIN` | `component/agent_env`, `src/ops/monitor` | read only by a build with debug assertions |
| `agent add --crewboard BIN` | `component/agent_add` | read only by a build with debug assertions |
| `CREWBOARD_WAKE_OS` | `component/wake_install`, `component/agent_add` | read only by a build with debug assertions |
| `HOST_LOCK_HOST` | `component/lock_*`, `tests/repeat_cli` | read only by a build with debug assertions |
| the `gh` calls of `wake --crewboard BIN` | none | removed: through the seam the wake knows no PR |
| `wake --crewboard BIN` (and the `hook --crewboard BIN` that it passes on) | `component/wake_lock`, `component/fake_wake` | unchanged: named in `docs/design/spec/cli.md`; `wake install --crewboard` is the binary of the wrapper |
| `--wake-cmd`, `CREWBOARD_WAKE_CMD` | `src/wake/cmd_tests`, `component/wake_*`, `component/fake_wake` | unchanged: named in the guide of the unattended Claude wake |
| `HOST_LOCK_PS_CMD`, `HOST_LOCK_PARENT_CMD` | `component/lock_*`, `tests/repeat_cli` | unchanged: named in `lock --help` and `docs/reference/cli.md` |
| `HOST_LOCK_BOARD_TTL` | none | unchanged: named in `lock --help` and `docs/reference/cli.md` |
| `CREWBOARD_RETRY_DELAYS` | `src/wake/cmd_tests/values` | unchanged: the default of `--retry-delays`; it runs nothing |

## test-loops.sh

| Case | What the case asserted | Status | Rust tests | Note |
|---|---|---|---|---|
| W1 | 20x exit 1 -> silent | existing | `src/ops/monitor::w1_twenty_silent_cycles_print_nothing`<br>`component/fake_monitor::monitor_is_silent_without_events_and_prints_the_events_once` |  |
| W2 | 7x exit 1 then exit 0 -> one block, exit 0 | existing | `src/ops/monitor::w2_events_after_silent_cycles_print_one_block` | unit level |
| W2b | cursor line of exit 1 does not leak | existing | `src/ops/monitor::w2b_cursor_line_does_not_leak` | unit level |
| W3 | exit 3 -> one error line, rc 3 | new | `src/ops/monitor::w3_w3e_a_stderr_of_one_reason_or_one_status_line_gives_only_that_part`<br>`src/ops/monitor::w3_network_error_prints_one_line_and_exits_3`<br>`src/ops/monitor::the_subprocess_cycle_reads_reason_status_and_code_from_the_binary` | unit level |
| W3b | exit 4 and exit 2 pass through | existing | `src/ops/monitor::w3b_exit_4_and_2_pass_through`<br>`src/ops/monitor::the_subprocess_cycle_reads_reason_status_and_code_from_the_binary` | unit level |
| W3d | exit 4 shows the first error line and the status | existing | `src/ops/monitor::w3d_reason_and_status`<br>`src/ops/monitor::the_subprocess_cycle_reads_reason_status_and_code_from_the_binary`<br>`src/ops/monitor::reason_is_the_first_nonblank_line_without_control_characters_cut_to_200`<br>`src/ops/monitor::w6_log_counts_silent_cycles` | unit level |
| W3e | status alone is shown; reason alone has no brackets | new | `src/ops/monitor::w3_w3e_a_stderr_of_one_reason_or_one_status_line_gives_only_that_part`<br>`src/ops/monitor::w3e_status_alone_reason_alone`<br>`src/ops/monitor::status_lines_are_told_from_reasons` | unit level |
| W4 | env file passed, not printed; mode 644 refused | new | `component/agent_env::w4_the_token_of_env_file_reaches_the_monitor_child_and_is_never_printed`<br>`component/agent_env::monitor_prints_a_bad_env_file_as_its_one_line` |  |
| W5 | arguments reach crewboard | existing | `src/cli/tests::monitor_flags_and_the_arguments_after_the_dashes_merge_for_watch`<br>`src/ops/monitor::w5_args_reach_watch` | unit level |
| W6 | --log counts 5 silent cycles | existing | `src/ops/monitor::w6_log_counts_silent_cycles` | unit level |
| P1 | 20x poll exit 1 -> 0 wakes, silent | existing | `src/wake/cmd_tests/precheck::p1_twenty_quiet_polls_start_no_wake_and_say_nothing` | unit level |
| P2 | one exit 0 among 20 -> exactly 1 wake, lock released | existing | `src/wake/cmd_tests/precheck::p2_one_exit_0_among_twenty_polls_gives_exactly_one_wake_and_releases_the_lock`<br>`component/wake_lock::p2_a_poll_with_events_runs_one_wake_prints_nothing_and_releases_the_lock` |  |
| P3 | exit 3 -> rc 3, no wake, logged | existing | `src/wake/cmd_tests/precheck::p3_exit_3_is_rc_3_with_no_wake_and_is_logged` | unit level |
| P3b | exit 4 passes through; no wake command -> 64 | existing | `src/wake/cmd_tests/precheck::p3b_exit_4_passes_through_and_a_missing_wake_command_is_refused`<br>`component/wake_lock::a_run_without_a_wake_command_or_with_a_bad_flag_value_is_exit_64` |  |
| P3d | failing poll logs its first stderr line, cut, no ctrl chars | existing | `src/wake/cmd_tests/precheck::p3d_a_failing_poll_logs_its_first_stderr_line_cut_and_cleaned` | unit level |
| P3e | quiet poll -> nothing logged | existing | `src/wake/cmd_tests/precheck::p3e_a_quiet_poll_logs_nothing_at_all` | unit level |
| Q1 | 5 quiet polls inside the hour -> no log line, count 5 | existing | `src/wake/cmd_tests/precheck::q1_quiet_polls_inside_one_hour_write_no_line_and_are_counted` | unit level |
| Q2 | first quiet poll after an hour -> one line, N=6 | existing | `src/wake/cmd_tests/precheck::q2_the_first_quiet_poll_after_the_hour_writes_one_summary_line`<br>`src/wake/state::say_goes_to_stderr_and_the_log_and_line_only_to_the_log` | unit level |
| Q3 | a wake logs as before; broken count file restarts | existing | `src/wake/cmd_tests/precheck::q3_a_wake_logs_as_before_and_a_broken_count_file_restarts` | unit level |
| P3c | failing wake -> rc 70, lock released | existing | `src/wake/cmd_tests/wake_lock::p3c_a_failing_wake_command_is_rc_70_and_the_lock_is_released` | unit level |
| P4 | lock held -> second run starts no second wake | existing | `component/wake_lock::p4_a_second_run_during_a_wake_starts_no_second_wake`<br>`src/wake/cmd_tests/wake_lock::p4_a_lock_of_a_live_wake_starts_no_second_wake`<br>`src/wake/lockdir::p4_a_live_lock_is_busy_and_names_its_pid_and_start` |  |
| P5 | stale lock (dead pid) taken over, wake runs once | existing | `src/wake/cmd_tests/wake_lock::p5_a_stale_lock_of_a_dead_pid_is_taken_over_and_said`<br>`src/wake/lockdir::p5_a_dead_pid_is_taken_over_and_said`<br>`component/wake_lock::p5_a_lock_of_a_dead_pid_is_taken_over_and_said` |  |
| P5b | live+young lock respected, live+over-budget taken | existing | `src/wake/cmd_tests/wake_lock::p5b_a_live_lock_over_the_budget_is_taken_a_young_one_is_not`<br>`src/wake/lockdir::p5b_a_live_lock_is_stale_only_over_the_budget`<br>`component/wake_lock::p5b_a_live_lock_is_stale_only_over_the_budget` |  |
| P5c | fresh empty lock dir is NOT taken over | existing | `src/wake/cmd_tests/wake_lock::p5c_p5d_an_empty_lock_dir_waits_for_the_grace`<br>`src/wake/lockdir::p5c_a_fresh_empty_lock_is_never_stale`<br>`component/wake_lock::p5c_p5d_an_empty_lock_dir_is_taken_over_only_after_the_grace` |  |
| P5d | empty lock dir older than the grace is taken over | existing | `src/wake/lockdir::p5d_an_empty_lock_older_than_the_grace_is_taken_with_the_named_reason` | unit level |
| P6 | --config-dir is passed to the wake command | existing | `src/wake/cmd_tests/options::p6_the_config_dir_reaches_the_wake_command`<br>`component/wake_lock::s1_two_agents_of_one_initiative_on_one_host_both_wake` |  |
| P7 | owned work, no events, no earlier wake -> 1 work wake | existing | `src/wake/cmd_tests/options::p7_owned_work_without_events_gives_one_work_wake` | unit level |
| P8 | no events, no owned ticket -> no wake, silent | existing | `src/wake/cmd_tests/options::p8_no_events_and_no_owned_ticket_is_a_silent_quiet_poll` | unit level |
| P9 | back-off not elapsed -> no wake | existing | `src/wake/cmd_tests/options::p9_p10_the_back_off_decides_whether_mine_is_even_asked` | unit level |
| P10 | back-off elapsed -> work wake, back-off restarts | existing | `src/wake/cmd_tests/options::p9_p10_the_back_off_decides_whether_mine_is_even_asked` | unit level |
| P10b | --work-backoff 5 wakes, off does not, junk -> 64 | existing | `src/wake/cmd_tests/options::p10b_the_back_off_is_configurable_off_never_wakes_and_junk_is_refused`<br>`component/wake_lock::a_run_without_a_wake_command_or_with_a_bad_flag_value_is_exit_64` |  |
| P11 | event wake starts the back-off: quiet tick after it -> no 2nd wake | existing | `src/wake/cmd_tests/options::p11_an_event_wake_starts_the_back_off_too` | unit level |
| P12 | heartbeat every 1 s during a 4 s wake, none after it | existing | `src/wake/cmd_tests/heartbeats::p12_a_long_wake_sends_heartbeats_from_its_own_thread_and_none_after_it`<br>`src/wake/heartbeat::p12_one_heartbeat_for_each_elapsed_interval_and_none_after_the_stop` | unit level |
| P12b | fast wake and --heartbeat-every 0 -> no heartbeat | existing | `src/wake/cmd_tests/heartbeats::p12_a_long_wake_sends_heartbeats_from_its_own_thread_and_none_after_it` | unit level |
| E1 | agent with-env passes token silently; mode 644 refused | existing | `component/agent_exec::with_env_gives_the_command_the_token_and_prints_nothing`<br>`component/agent_exec::with_env_refuses_what_the_loader_refuses_and_runs_nothing` |  |
| E1b | env file without a token or broken: exit 4, nothing run | existing | `component/agent_exec::with_env_refuses_what_the_loader_refuses_and_runs_nothing` |  |
| G1 | shim loads the agent token; a bare gh has none | new | `component/agent_shim::g1_a_given_real_gh_is_the_one_the_stub_runs_and_the_token_is_never_printed`<br>`component/agent_shim::the_stub_runs_the_real_gh_with_the_crew_token_and_a_bare_gh_has_none`<br>`component/agent_shim::the_stub_is_the_three_lines_with_absolute_paths_mode_755` |  |
| G1b | token is not written into the shim or printed | new | `component/agent_shim::g1_a_given_real_gh_is_the_one_the_stub_runs_and_the_token_is_never_printed`<br>`component/agent_shim::the_stub_is_the_three_lines_with_absolute_paths_mode_755`<br>`component/agent_add::b1b_no_token_in_output_or_log` |  |
| G1c | real gh found on the PATH, not the shim | existing | `component/agent_shim::a_second_run_is_already_done_and_the_real_gh_is_never_the_stub`<br>`component/agent_shim::the_stub_runs_the_real_gh_with_the_crew_token_and_a_bare_gh_has_none`<br>`src/ops/agent/shim::the_real_gh_is_the_first_one_that_is_not_the_shim` |  |
| G1d | missing env file -> rc 4 | existing | `component/agent_shim::no_real_gh_a_missing_dir_and_a_missing_env_file_are_exit_4` |  |
| G1e | make-shim names agent exec only for <agent-dir>/bin | new | `component/agent_shim::g1_a_given_real_gh_is_the_one_the_stub_runs_and_the_token_is_never_printed`<br>`component/agent_shim::the_compat_options_of_the_old_script_make_the_stub_in_another_dir_and_only_the_git_identity` | the second test has a new assertion |
| G2 | worktree gets the crew git config, the owner checkout none | new assertion | `component/agent_shim::a_linked_worktree_gets_the_identity_in_its_own_scope_and_the_main_checkout_keeps_its_own` | the credential helpers of the worktree and of the main checkout |
| G3 | plain clone gets local crew git config | existing | `component/agent_shim::the_compat_options_of_the_old_script_make_the_stub_in_another_dir_and_only_the_git_identity` |  |
| A1 | claude wake: cwd, -p, permission flags, -- before the prompt, prompt | existing | `src/wake/cmd_tests/events_file::a1_a_dev_agent_wake_runs_the_runtime_in_the_work_dir_with_its_environment`<br>`src/wake/adapters::a9_a10_the_prompt_is_one_template_and_names_the_host_locks`<br>`src/wake/cmd_tests/env_file::the_env_file_cannot_replace_what_the_wake_sets`<br>`src/wake/cmd_tests/options::p7_owned_work_without_events_gives_one_work_wake`<br>`src/wake/job::the_runtime_binary_is_a_path_or_a_name_on_the_path` | unit level |
| A1b | claude default allow list: crewboard, git, gh and the shell helpers | new assertion | `src/wake/adapters::a1_claude_default_argv` | unit level; the order of the shell helpers, grep among them |
| A2 | codex wake: exec, sandbox flags, prompt | existing | `src/wake/adapters::a2_codex_and_a9_hermes_argv_and_a5b_model` | unit level |
| A3 | quiet board -> no runtime started | existing | `src/wake/cmd_tests/precheck::p1_twenty_quiet_polls_start_no_wake_and_say_nothing`<br>`src/wake/cmd_tests/options::p8_no_events_and_no_owned_ticket_is_a_silent_quiet_poll`<br>`component/fake_wake::a_quiet_board_starts_no_wake_and_an_assignment_starts_one` |  |
| A4 | --shim-dir: shim first on PATH, empty GH_CONFIG_DIR | existing | `src/wake/job::a4_the_shim_dir_goes_first_on_the_path_and_names_the_empty_gh_config`<br>`src/wake/cmd_tests/env_file::the_env_file_cannot_replace_what_the_wake_sets` | unit level |
| A4b | no --shim-dir: GH_CONFIG_DIR stays unset | existing | `src/wake/job::a4_the_shim_dir_goes_first_on_the_path_and_names_the_empty_gh_config` | unit level |
| A5 | --runtime-flags replace defaults; quote in prompt survives | existing | `src/wake/adapters::a5_runtime_flags_replace_the_defaults`<br>`src/wake/adapters::the_shell_line_keeps_the_text_shape_of_the_old_wake_command` | unit level |
| A5b | claude, codex, hermes: --model ID reaches the runtime | existing | `src/wake/adapters::a2_codex_and_a9_hermes_argv_and_a5b_model` | unit level |
| A5c | no --model: the runtime default applies | existing | `src/wake/adapters::a2_codex_and_a9_hermes_argv_and_a5b_model`<br>`src/wake/job::an_empty_model_is_no_model_the_config_dir_reaches_the_hook_and_the_seam_is_filtered` | unit level |
| A6 | bad runtime, work dir, shim dir -> exit 4, nothing started | new | `src/wake/cmd_tests/values::a6_a7_hw4_a_wrong_option_of_the_dev_agent_is_a_config_error_exit_4`<br>`src/wake/job::a6_refusals` | unit level |
| C3 | agent env: shim first, empty gh config, config dir; no token | new | `component/agent_exec::c3_env_of_the_config_dir_with_an_agent_token_prints_the_three_lines_and_no_token`<br>`component/agent_exec::env_prints_the_three_export_lines_and_no_token`<br>`src/ops/agent/env::lines_of_a_full_agent_dir` |  |
| C3b | agent env without shim dirs: config dir only; no dir -> 4 | existing | `component/agent_exec::env_of_a_bare_dir_prints_the_config_dir_only_and_a_missing_dir_exits_4`<br>`src/ops/agent/env::a_bare_dir_gets_the_config_dir_only`<br>`src/ops/agent/env::a_missing_dir_is_a_config_error` |  |
| C4 | monitor: agent token, config dir, agent log, event printed | existing | `component/agent_env::the_monitor_child_gets_the_file_token_and_loses_the_caller_variables`<br>`src/cli/tests::monitor_flags_and_the_arguments_after_the_dashes_merge_for_watch`<br>`src/ops/monitor::w5_args_reach_watch`<br>`src/ops/monitor::w2_events_after_silent_cycles_print_one_block`<br>`src/ops/monitor::w6_log_counts_silent_cycles` |  |
| C4b | monitor: --log is the log file | existing | `src/ops/monitor::w6_log_counts_silent_cycles`<br>`component/fake_monitor::the_retry_notice_of_watch_goes_to_the_log_and_not_to_stderr` |  |
| XE1 | agent exec: agent exports set, caller token gone, exit code kept | existing | `component/agent_exec::exec_sets_the_agent_exports_and_removes_the_caller_tokens`<br>`component/agent_exec::exec_uses_the_config_dir_flag_when_there_is_no_agent_dir_flag`<br>`component/agent_exec::exec_keeps_the_exit_code_of_the_command` |  |
| XE1b | agent exec without agent dir -> 4, without command -> 64, nothing runs | existing | `component/agent_exec::exec_with_a_missing_dir_exits_4_and_runs_nothing` |  |
| B1 | bootstrap: 6 steps done, join posted, one next step | new assertion | `component/agent_add::b1_seven_steps_join_posted_one_next_step` | the token of the join request, the origin URL, the agent dir of the rerun hint |
| B1b | bootstrap prints no token (output and log) | existing | `component/agent_add::b1b_no_token_in_output_or_log` |  |
| B2 | rerun: 6x already done, start as the agent, still waiting | new assertion | `component/agent_add::b2_rerun_all_already_done_and_no_second_join` | the cwd, config dir, PATH head and token of the `start` child |
| B3 | rerun after approval: start done, next step runs the runtime | new assertion | `component/agent_add::b3_rerun_after_approval_start_is_done_and_the_next_step_runs_the_runtime` | the options of the wake install hint |
| B4 | start refused: step named, doctor --setup as fix, rc 2 | new assertion | `component/agent_add::b4_start_refused_names_the_step_and_doctor_as_the_fix_and_keeps_its_exit_code` | the full fix line |
| B5 | env file mode 644: step 1 FAIL with chmod fix, nothing runs | existing | `component/agent_add::b5_env_file_mode_644_fails_step_1_and_nothing_runs` |  |
| B5b | join fails: step 2 FAIL, rc 3, no shim, no clone | existing | `component/agent_add::b5b_join_fails_step_2_with_the_code_of_join_and_no_shim_no_clone`<br>`component/agent_add::b1c_a_token_that_a_step_prints_is_replaced_in_the_log_and_in_the_failure_text` |  |
| B5c | wrong crew login: step 4 FAIL, git identity untouched | existing | `component/agent_add::b5c_a_crew_login_without_an_id_fails_step_4_and_the_work_dir_is_untouched` |  |
| B5d | non-empty non-git work dir: step 5 FAIL | existing | `component/agent_add::b5d_a_non_empty_work_dir_that_is_no_checkout_fails_step_5` |  |
| B5e | checkout of another repo: step 5 FAIL | existing | `component/agent_add::b5e_a_checkout_of_another_repo_fails_step_5` |  |
| B5f | a changed git e-mail: step 6 sets it again | existing | `component/agent_add::b5f_a_changed_git_email_is_set_again_by_step_6` |  |
| B6 | bootstrap usage errors -> 64, nothing runs | existing | `component/agent_add::b6_usage_errors_exit_64_and_write_nothing` |  |
| B7 | coordinator bootstrap: 6 steps, approve line has --role coordinator | new assertion | `component/agent_add::b7_coordinator_role_names_its_role_in_the_approve_line_and_the_headless_command_after` | empty stderr |
| B7b | coordinator rerun after approval: next step is the headless one | new assertion | `component/agent_add::b7_coordinator_role_names_its_role_in_the_approve_line_and_the_headless_command_after` | one next step |
| B8 | headless coordinator: dir made, toml headless first, key 600 | new assertion | `component/agent_add::b8_headless_dir_toml_first_line_key_600_and_no_key_printed` | the rest of the toml unchanged, the wake install command and its env file |
| B8b | headless bootstrap prints no key or token | existing | `component/agent_add::b8_headless_dir_toml_first_line_key_600_and_no_key_printed` |  |
| B8c | headless rerun: 3x already done, one headless line | new assertion | `component/agent_add::b8c_headless_rerun_is_three_times_already_done_with_one_headless_line` | one next step |
| B9 | headless refusals: headless source, no key, same dir, usage | existing | `component/agent_add::b9_headless_refusals_leave_no_half_made_dir` |  |
| H1 | poll-and-wake: minimal PATH finds ~/.cargo/bin/crewboard | obsolete |  | the PATH that the driver added for a minimal scheduler PATH; the PATH that the binary gives its own wake child is `src/wake/job::the_install_dirs_go_to_the_end_when_they_exist_and_are_not_there` |
| H2 | poll-and-wake: ~/.local/bin added, missing ~/.cargo/bin not | obsolete |  | the PATH that the driver added for a minimal scheduler PATH; the PATH that the binary gives its own wake child is `src/wake/job::the_install_dirs_go_to_the_end_when_they_exist_and_are_not_there` |
| H3 | no install dir: rc 127 and said, not silent | obsolete |  | exit 127 and its message came from the driver |
| H4 | dirs already on PATH -> PATH unchanged | obsolete |  | the PATH that the driver added for a minimal scheduler PATH; the PATH that the binary gives its own wake child is `src/wake/job::the_install_dirs_go_to_the_end_when_they_exist_and_are_not_there` |
| H5 | agent-wake: minimal PATH finds crewboard; it gets the dev agent options | obsolete |  | the PATH and the option mapping of the driver agent-wake.sh |
| S1 | two agents, one host, one initiative: both wake | existing | `component/wake_lock::s1_two_agents_of_one_initiative_on_one_host_both_wake` |  |
| S1b | the same agent twice: one wake, the second waits | existing | `component/wake_lock::s1b_the_same_agent_twice_runs_one_wake_and_the_second_waits` |  |
| S2 | state dir per agent; --lock-dir and no agent as before | existing | `component/wake_lock::s2_print_state_dir_follows_the_agent_and_starts_nothing`<br>`src/wake/cmd_tests/options::s2_print_state_dir_follows_the_flags` |  |
| S3 | old last-wake is read once: the back-off holds | existing | `src/wake/cmd_tests/legacy::the_old_last_wake_is_read_once_and_the_back_off_holds`<br>`src/wake/state::the_legacy_dir_is_only_for_the_default_per_agent_state` | unit level |
| S3b | live old per-initiative lock: no wake; a dead one is ignored | existing | `src/wake/cmd_tests/legacy::a_live_old_lock_of_this_agent_starts_no_wake_another_agents_does_not` | unit level |
| S3c | old lock of another agent: this agent wakes; the same config dir waits | existing | `src/wake/cmd_tests/legacy::a_live_old_lock_of_this_agent_starts_no_wake_another_agents_does_not`<br>`src/wake/cmd_tests/legacy::the_holder_config_dir_is_the_word_after_the_flag` | unit level |
| R1 | exit 3, then events: retried, one wake | existing | `src/wake/cmd_tests/retry::r1_exit_3_is_retried_and_the_retry_finds_events` | unit level |
| R2 | exit 3 every try: rc 3; 2nd failed run pauses the next run | existing | `src/wake/cmd_tests/retry::r2_exit_3_in_every_try_pauses_the_runs_from_the_second_failed_run` | unit level |
| R2b | pause over, a good poll clears the back-off | existing | `src/wake/cmd_tests/retry::r2b_when_the_pause_is_over_a_good_poll_clears_the_back_off` | unit level |
| X1 | exit 4: stop, one line, say; next run calls nothing | existing | `src/wake/cmd_tests/retry::x1_exit_4_stops_says_once_tells_the_coordinator_and_silences_the_next_run`<br>`component/fake_wake::exit_4_of_poll_tells_the_coordinator_through_say_in_process_and_through_a_subprocess` |  |
| X1b | a newer env file lifts the stop: polls again | existing | `src/wake/cmd_tests/retry::x1b_a_newer_env_file_lifts_the_stop_and_the_run_polls_again` | unit level |
| X2 | exit 2 is no stop: the next run polls again | existing | `src/wake/cmd_tests/retry::x2_exit_2_is_no_stop_and_the_next_run_polls_again` | unit level |
| TB1 | over budget: stopped with its child, say, host lock released | new | `component/wake_budget::tb1_a_budget_stop_releases_the_host_lock_of_the_agent_tb2_a_wake_inside_its_budget_keeps_it`<br>`src/wake/cmd_tests/budget::tb1_a_wake_over_its_budget_is_stopped_the_coordinator_hears_of_it_and_the_run_exits_0` |  |
| TB2 | inside the budget: no say, host lock stays | new | `component/wake_budget::tb1_a_budget_stop_releases_the_host_lock_of_the_agent_tb2_a_wake_inside_its_budget_keeps_it` |  |
| TB10 | budget stop: run releases its lock cleanly, no lock dir left | new | `component/wake_budget::tb10_a_budget_stop_of_a_lock_run_leaves_no_lock_dir` |  |
| TB11 | budget stop: detached job and its lock stay | new | `component/wake_budget::tb11_a_budget_stop_keeps_a_detached_job_and_its_lock_tb11b_the_job_releases_it_later` |  |
| TB11b | the detached job ends after the stop: result, lock released | existing | `component/lock_detach::dt3_a_kill_of_the_caller_group_leaves_the_job_and_its_lock_the_job_ends_later` |  |
| TB3 | bad --max-wake -> 64, no poll | existing | `src/wake/cmd_tests/heartbeats::tb3_a_bad_budget_or_heartbeat_is_exit_64_as_a_flag_4_from_the_env_and_logged`<br>`src/wake/policy::the_values_of_the_options_parse`<br>`component/wake_lock::a_run_without_a_wake_command_or_with_a_bad_flag_value_is_exit_64` |  |
| TB4 | pushed ticket branch: branch, head SHA, pushed, PR | existing | `src/wake/describe_head::tb4_a_pushed_ticket_branch_in_a_clone_under_the_work_dir_and_the_pr_of_mine` | unit level |
| TB5 | PR of the ticket branch from gh pr list, decoy skipped | existing | `src/wake/describe_head::tb5_the_open_pr_of_the_ticket_branch_and_not_the_decoy_of_another_agent` | unit level |
| TB6 | no branch, no PR: the message says so | new assertion | `src/wake/describe_head::tb6_no_ticket_branch_and_no_pr_says_so` | unit level; an agent, no checkout and no open PR |
| TB7 | events read before say: no --override-unread; next wake gets them | existing | `src/wake/cmd_tests/budget::tb7_with_an_events_file_the_unread_events_are_read_before_say_and_kept`<br>`src/wake/cmd_tests/budget::a_wake_at_its_budget_keeps_its_events_and_names_uncommitted_work`<br>`src/wake/cmd_tests/budget::the_second_budget_stop_in_a_row_waits_for_the_backoff_and_tells_the_coordinator`<br>`src/wake/cmd_tests/events_file::ev2_a_failed_wake_keeps_its_events_for_the_next_wake` | unit level |
| EV1 | events and mine in a file; the prompt names it | new assertion | `src/wake/cmd_tests/events_file::ev1_the_events_file_has_the_poll_and_the_mine_text_and_the_wake_gets_its_path`<br>`src/wake/cmd_tests/events_file::a1_a_dev_agent_wake_runs_the_runtime_in_the_work_dir_with_its_environment` | unit level; the prompt names the default events file (second test) |
| EV2 | failed wake keeps its events for the next wake | existing | `src/wake/cmd_tests/events_file::ev2_a_failed_wake_keeps_its_events_for_the_next_wake` | unit level |
| EV3 | no --events-file: only poll --quiet, the cursor stays | existing | `src/wake/cmd_tests/events_file::ev1_the_events_file_has_the_poll_and_the_mine_text_and_the_wake_gets_its_path` | unit level |
| AC1 | receipts logged; a wake that acted: no re-delivery | existing | `src/wake/cmd_tests/receipts::ac1_receipts_are_logged_and_a_wake_that_acted_gets_no_redelivery` | unit level |
| AC2 | one miss: delivered again once; second miss: only the log | existing | `src/wake/cmd_tests/receipts::ac2_one_miss_is_delivered_again_once_and_a_second_miss_only_logs` | unit level |
| AC3 | failing wake: old path, no check | existing | `src/wake/cmd_tests/receipts::ac3_a_failing_wake_keeps_the_whole_file_and_runs_no_check` | unit level |
| AC4 | budget stop: old path, no check | new | `src/wake/cmd_tests/budget::ac4_a_budget_stop_runs_no_check_of_the_receipts` | unit level |
| AC5 | failing check: logged, nothing delivered again, rc 0 | existing | `src/wake/cmd_tests/receipts::ac5_a_check_that_fails_is_logged_and_changes_nothing` | unit level |
| AC6 | no --events-file: no acted call | existing | `src/wake/cmd_tests/receipts::ac6_without_an_events_file_there_is_no_receipt_and_no_check` | unit level |
| RS1 | finished detached job: quiet poll wakes once, then quiet | existing | `src/wake/cmd_tests/results::rs1_a_finished_result_wakes_a_quiet_poll_once_with_its_event_line` | unit level |
| RS1b | the events file has the line of the finished job | existing | `src/wake/results::rs1_a_finished_result_is_pending_until_it_is_marked_and_a_new_run_is_pending_again` | unit level |
| RS1c | a new result of the same file is reported; no ticket= is fine | existing | `src/wake/cmd_tests/results::rs1_a_finished_result_wakes_a_quiet_poll_once_with_its_event_line`<br>`src/wake/results::rs1_a_finished_result_is_pending_until_it_is_marked_and_a_new_run_is_pending_again` | unit level |
| RS2 | unfinished result: not reported, no wake alone | existing | `src/wake/cmd_tests/results::rs2_an_unfinished_result_wakes_nothing_and_is_not_marked`<br>`src/wake/results::rs2_only_finished_regular_files_that_are_no_scripts_are_pending` | unit level |
| RS3 | result of another dir: not read | existing | `src/wake/cmd_tests/results::rs3_a_result_of_another_agents_dir_is_not_read`<br>`src/wake/results::rs3_only_the_given_dir_is_read` | unit level |
| RS4 | script with rc= at column 0: not reported; a real result still is | new | `component/fake_wake::rs4_a_script_in_the_results_dir_is_no_result_and_a_real_result_still_wakes`<br>`src/wake/results::rs2_only_finished_regular_files_that_are_no_scripts_are_pending` |  |
| HK1 | hook not due: no poll, no output, events file unchanged | existing | `src/wake/hook::hk1_not_due_no_poll_no_output_and_an_unchanged_file`<br>`component/agent_env::the_events_hook_with_a_broken_agent_env_exits_0_says_nothing_and_uses_no_token` |  |
| HK1b | no stamp, fresh events file: the poll of the loop counts, no poll | existing | `src/wake/hook::hk1b_without_a_stamp_the_age_of_the_events_file_counts` | unit level |
| HK2 | hook due, events: JSON additionalContext, events file, same poll call | new | `component/fake_wake::hk2_a_due_hook_prints_the_new_events_as_json_and_appends_them_hk6_the_variable_sets_the_interval`<br>`src/wake/hook::hk2_due_with_events_json_events_file_log_and_stamp` |  |
| HK2b | the next call within the interval: no second poll | existing | `src/wake/hook::hk2_due_with_events_json_events_file_log_and_stamp` | unit level |
| HK3 | hook due, no events: no output, exit 0 | existing | `src/wake/hook::hk3_due_but_no_events_gives_no_output_and_no_change` | unit level |
| HK4 | hook due, poll fails: no output, exit 0, one log line | existing | `src/wake/hook::hk4_a_failed_poll_is_one_log_line_and_no_output`<br>`component/agent_env::the_events_hook_with_a_broken_agent_env_exits_0_says_nothing_and_uses_no_token` |  |
| HK4b | hook with a missing crewboard binary: no output, exit 0 | new | `src/wake/steps::hk4b_a_binary_that_is_not_there_is_exit_127`<br>`src/wake/hook::hk4_a_failed_poll_is_one_log_line_and_no_output`<br>`component/agent_env::the_events_hook_with_a_broken_agent_env_exits_0_says_nothing_and_uses_no_token` |  |
| HK5 | hook outside a wake: no poll, no output | existing | `src/wake/hook::hk5_outside_a_wake_there_is_nothing` | unit level |
| HK6 | CREWBOARD_HOOK_EVERY=0 makes the hook due | new | `component/fake_wake::hk2_a_due_hook_prints_the_new_events_as_json_and_appends_them_hk6_the_variable_sets_the_interval`<br>`src/wake/hook::hk6_the_interval_is_a_number_else_the_default` |  |
| CO1 | coordinator: tick lines logged, no model, no poll | existing | `src/wake/cmd_tests/tick::co1_tick_runs_its_lines_go_to_the_log_and_no_model_starts_and_no_poll_runs`<br>`component/fake_wake::tick_runs_in_process_in_a_headless_dir_and_a_refused_tick_is_logged` |  |
| CO2 | repeated and idle ticks write no new line | existing | `src/wake/cmd_tests/tick::co2_the_same_output_again_and_an_idle_tick_write_no_new_line` | unit level |
| CO3 | refused tick: rc 2, reason logged | existing | `src/wake/cmd_tests/tick::co3_a_refused_tick_is_logged_with_its_reason_and_exits_2`<br>`component/fake_wake::tick_runs_in_process_in_a_headless_dir_and_a_refused_tick_is_logged` |  |
| CO4 | --print-cmd of the coordinator: tick, no model | existing | `src/wake/cmd_tests/options::print_cmd_is_a_plan_not_a_run` | unit level |
| A7 | --allow appends to the claude tools; codex refuses it | new | `src/wake/cmd_tests/values::a6_a7_hw4_a_wrong_option_of_the_dev_agent_is_a_config_error_exit_4`<br>`src/wake/adapters::a7_allow_appends_and_a_wrong_combination_is_an_error` | unit level |
| A8 | codex: --add-dir config dir and lock dir; host-lock rule in prompt | new | `src/wake/job::a8_codex_may_write_the_config_dir_and_the_parent_of_the_host_lock`<br>`src/wake/adapters::a8_add_dirs_for_the_state_dir_the_config_dir_and_the_lock_parents`<br>`src/wake/adapters::a9_a10_the_prompt_is_one_template_and_names_the_host_locks`<br>`src/wake/job::a8c_a8d_the_wake_marks_itself_and_grants_lock_run_for_the_host_locks_only` | unit level |
| A8b | claude: --add-dir state dir and lock dir | existing | `src/wake/adapters::a8_add_dirs_for_the_state_dir_the_config_dir_and_the_lock_parents`<br>`src/wake/job::a8c_a8d_the_wake_marks_itself_and_grants_lock_run_for_the_host_locks_only`<br>`src/wake/adapters::a8c_skill_read_dirs_resolve_the_symlinks_and_skip_what_is_missing` | unit level |
| A8c | wake sets CREWBOARD_WAKE and CREWBOARD_LOCK_RUN_DIRS from --host-lock | existing | `src/wake/job::a8c_a8d_the_wake_marks_itself_and_grants_lock_run_for_the_host_locks_only`<br>`src/wake/cmd_tests/env_file::the_env_file_cannot_replace_what_the_wake_sets` | unit level |
| A8d | without --host-lock: wake=1 and no lock dir granted | existing | `src/wake/job::a8c_a8d_the_wake_marks_itself_and_grants_lock_run_for_the_host_locks_only` | unit level |
| A8e | claude: --add-dir skill dirs; refused-command and background rules in the prompt | new | `src/wake/job::a8e_a_claude_job_may_read_the_installed_skill_of_home`<br>`src/wake/adapters::a8b_claude_gets_the_skill_dirs_codex_and_hermes_do_not`<br>`src/wake/adapters::a8c_skill_read_dirs_resolve_the_symlinks_and_skip_what_is_missing`<br>`src/wake/adapters::a9_a10_the_prompt_is_one_template_and_names_the_host_locks` | unit level |
| A9 | hermes: chat -Q -s crewboard -q, the same prompt as claude | existing | `src/wake/adapters::a2_codex_and_a9_hermes_argv_and_a5b_model`<br>`src/wake/adapters::a9_a10_the_prompt_is_one_template_and_names_the_host_locks`<br>`src/wake/cmd_tests/options::print_cmd_is_a_plan_not_a_run` | unit level |
| A10 | the prompt points to the skill; no budget or checkpoint copy | existing | `src/wake/adapters::a9_a10_the_prompt_is_one_template_and_names_the_host_locks` | unit level |
| HW1 | claude wake command has the PostToolUse hook | new | `src/wake/job::hw1_the_hook_of_a_claude_job_polls_at_the_events_every_hw1b_300_s_by_default`<br>`src/wake/adapters::hw1_the_hook_settings_are_json_with_the_command_and_a_timeout` | unit level |
| HW1b | the default interval of the hook is 300 s | new | `src/wake/job::hw1_the_hook_of_a_claude_job_polls_at_the_events_every_hw1b_300_s_by_default` | unit level |
| HW2 | --no-events-hook: no hook in the wake command | existing | `src/wake/job::the_job_has_the_cwd_the_text_and_the_hook_only_for_claude`<br>`src/wake/adapters::a1_claude_default_argv`<br>`src/wake/adapters::the_shell_line_keeps_the_text_shape_of_the_old_wake_command` | unit level |
| HW3 | codex and hermes: no hook | existing | `src/wake/job::the_job_has_the_cwd_the_text_and_the_hook_only_for_claude` | unit level |
| HW4 | --events-every needs whole seconds -> 4 | new | `src/wake/cmd_tests/values::a6_a7_hw4_a_wrong_option_of_the_dev_agent_is_a_config_error_exit_4`<br>`src/wake/job::a6_refusals` | unit level |
| HW5 | a claude wake gets --settings and CREWBOARD_EVENTS_FILE | existing | `src/wake/job::the_job_has_the_cwd_the_text_and_the_hook_only_for_claude`<br>`src/wake/cmd_tests/env_file::the_env_file_cannot_replace_what_the_wake_sets` | unit level |
| IN1 | shim --install systemd: crewboard wake install, unit enabled | obsolete | `component/wake_install::systemd_install_writes_the_units_enables_them_and_prints_the_one_command_left`<br>`src/wake/install/tests::the_wrapper_stores_the_path_and_the_absolute_paths_and_runs_crewboard_wake` | `--install` of the driver; the binary side is in the tests named here |
| IN2 | shim --install --role coordinator: --tick, no runtime | obsolete | `component/wake_install::the_names_and_the_agent_come_from_the_agent_config_and_a_coordinator_ticks`<br>`src/wake/install/tests::a_coordinator_runs_the_tick_without_a_runtime` | `--role coordinator` to `--tick` of the driver; the binary side is in the tests named here |
| IN3 | shim --uninstall removes only that agent's unit | obsolete | `component/wake_install::uninstall_disables_removes_both_unit_files_and_keeps_the_log` | the option filter of the driver for `--uninstall`; the binary side is in the tests named here |
| IN4 | shim --install: a wrong scheduler or runtime is refused, nothing written | obsolete | `component/wake_install::a_wrong_option_exits_4_and_writes_nothing`<br>`src/wake/install/tests::refusals_name_the_wrong_option` | the pass-through of the driver; the binary side is in the tests named here |
| IN11 | no binary / a binary without wake: a clear line, exit 4, nothing written | obsolete |  | both messages and exit codes came from the drivers |
| IN12 | --print-cmd needs no env file; no HOME: the message names HOME | new | `component/fake_wake::in12_print_cmd_needs_no_env_file_and_without_home_the_message_names_home` |  |

## test-locks.sh

| Case | What the case asserted | Status | Rust tests | Note |
|---|---|---|---|---|
| H1 | free lock is taken, owner file complete | existing | `component/lock_cli::h1_a_free_lock_is_taken_and_the_owner_file_is_complete` |  |
| H2 | held live lock refuses the second taker (75) | existing | `component/lock_cli::h2_a_live_lock_refuses_the_second_taker_with_75` |  |
| H2b | --wait takes the lock after the owner releases | new | `component/lock_cli::h2b_wait_takes_the_lock_once_the_owner_releases_it`<br>`src/lock/run::h2b_wait_takes_the_lock_after_the_owner_releases` |  |
| H3 | dead pid is taken over, one log line with owner and reason | existing | `src/lock/run::h3_a_dead_pid_is_taken_over_with_one_log_line`<br>`component/lock_cli::h3_a_dead_pid_is_taken_over_with_one_log_line` |  |
| H3b | a live pid of another user is not stale (kill -0 EPERM) | existing | `component/lock_cli::h3b_a_live_pid_of_another_user_is_alive`<br>`src/lock/procs::eperm_a_live_pid_of_another_user_is_alive_for_the_real_procs` |  |
| H4 | live pid but older than --max-age is taken over | new | `component/lock_judge::h4_a_live_pid_older_than_max_age_is_taken_over` |  |
| H4b | live pid and young lock is NOT taken over | new | `component/lock_judge::h4_a_live_pid_older_than_max_age_is_taken_over`<br>`src/lock/judge::h4b_a_young_lock_with_a_live_pid_is_held` |  |
| H5 | other host: a dead pid number is not judged (refused) | new | `component/lock_judge::h5_another_host_is_not_judged_by_its_pid_h5b_but_by_the_age_limit`<br>`src/lock/judge::h5_another_host_a_dead_looking_pid_is_not_judged` |  |
| H5b | other host: too old is taken over by the age limit | new | `component/lock_judge::h5_another_host_is_not_judged_by_its_pid_h5b_but_by_the_age_limit` |  |
| H6 | release by a non-owner is refused (3), lock stays | existing | `component/lock_cli::h6_release_by_a_non_owner_other_host_and_owner`<br>`src/lock/run::h6_release_by_a_stranger_is_refused_by_the_owner_it_works` |  |
| H6b | release from another host is refused (3) | existing | `component/lock_cli::h6_release_by_a_non_owner_other_host_and_owner` |  |
| H6c | release by the owner removes the lock | existing | `component/lock_cli::h6_release_by_a_non_owner_other_host_and_owner` |  |
| H7 | 8 concurrent takers on a free lock: exactly one wins | existing | `component/lock_cli::h7_eight_concurrent_takers_on_a_free_lock_exactly_one_wins` |  |
| H7b | 8 takers on a stale lock: one holder, one takeover line | existing | `component/lock_cli::h7b_eight_takers_on_a_stale_lock_one_holder_one_takeover_line`<br>`src/lock/run::h7_a_takeover_guard_held_by_another_taker_stops_this_one_and_a_dead_taker_guard_goes` |  |
| H8 | owner agent dead on the board: taken over | existing | `component/lock_board::h8_e2e_a_dead_owner_agent_on_the_board_is_taken_over_an_alive_one_is_not`<br>`src/lock/board::h8_dead_left_alive_and_unlisted` |  |
| H8b | owner agent has left the board: taken over | existing | `component/lock_board::h8b_e2e_an_owner_agent_that_has_left_the_board_is_taken_over` |  |
| H8c | owner agent alive on the board: NOT taken over | existing | `component/lock_board::h8_e2e_a_dead_owner_agent_on_the_board_is_taken_over_an_alive_one_is_not` |  |
| H8d | owner agent not listed on the board: not judged | existing | `component/lock_board::h8d_e2e_an_owner_agent_that_the_board_does_not_list_is_not_judged` |  |
| H8e | --no-board skips the board check | existing | `component/lock_board::h8_e2e_the_same_dead_owner_is_not_judged_with_no_board`<br>`src/lock/board::h8e_no_board_never_asks` |  |
| H8f | board unreachable: skipped with a note, lock not stale | existing | `src/lock/board::h8f_an_unreachable_board_is_skipped_with_one_note`<br>`src/lock/board::a_failure_without_text_names_the_exit_code`<br>`src/lock/judge::h8_the_board_says_dead_or_left_is_stale_and_the_texts_are_the_old_ones` | unit level |
| H8g | board cache: 5 or more polls, one status call | new | `src/lock/run::h8g_a_wait_of_many_polls_asks_the_board_once`<br>`src/lock/board::h8g_the_cache_gives_one_call_for_many_polls` | unit level |
| H8h | 3 failures then ok: 1 skip note with cause, 1 works again | new | `src/lock/run::h8h_three_failed_board_checks_in_a_wait_give_one_note_and_one_works_again`<br>`src/lock/board::h8h_three_failures_then_ok_give_one_skip_note_and_one_works_again`<br>`src/ops/lock_cmd::h8h_the_board_ttl_variable_is_a_number_of_seconds_else_60` | unit level; the wait with a cache of 0 s runs on `Session`; the parse of `HOST_LOCK_BOARD_TTL` is the third test |
| H8h2 | board check failing: pid-dead takeover still works | existing | `src/lock/judge::h3_a_dead_pid_on_this_host_is_stale_a_live_one_is_held`<br>`src/lock/judge::the_pid_rule_comes_before_the_board` | unit level |
| H8i | dead owner: taken over as board-dead, one status call | new | `src/lock/run::h8i_a_dead_owner_is_taken_over_as_board_dead_with_one_board_call` | unit level |
| H9 | run holds the lock, releases it, returns the exit code | existing | `component/lock_cli::h9_run_holds_the_lock_while_the_command_runs_releases_and_returns_its_code` |  |
| H9b | run on a held live lock: CMD does not start (75) | existing | `component/lock_cli::h9b_run_on_a_held_live_lock_does_not_start_the_command_75` |  |
| H10 | status: free=1, held=0, stale=2 | existing | `component/lock_cli::h10_status_free_is_1_held_is_0_stale_is_2`<br>`src/lock/run::h10_status_free_held_stale` |  |
| H10b | status on a live lock: alive=yes and age= | existing | `component/lock_cli::h10b_status_on_a_live_lock_says_alive_and_age_a_dead_pid_is_stale_pid_dead`<br>`src/lock/judge::h10b_held_facts_say_alive_and_age` |  |
| H10c | status on a dead-pid lock: exit 2, pid-dead | existing | `component/lock_cli::h10b_status_on_a_live_lock_says_alive_and_age_a_dead_pid_is_stale_pid_dead` |  |
| H10d | break --if-dead on a live lock: exit 3, owner file unchanged | existing | `component/lock_cli::h10d_break_if_dead_on_a_live_lock_is_refused_3_and_the_owner_file_stays` |  |
| H10e | break --if-dead on a dead-pid lock: removed, exit 0 | existing | `component/lock_cli::h10e_break_if_dead_on_a_dead_pid_removes_it_with_one_log_line_in_the_log_file` |  |
| H10f | break --if-dead on a free dir: free, exit 0 | existing | `component/lock_cli::h10f_break_if_dead_on_a_free_dir_prints_free_0_h10g_without_if_dead_is_64` |  |
| H10g | break without --if-dead: exit 64, lock stays | existing | `component/lock_cli::h10f_break_if_dead_on_a_free_dir_prints_free_0_h10g_without_if_dead_is_64`<br>`src/lock/args::h10g_break_needs_if_dead_and_if_dead_is_for_break_only` |  |
| H11 | FRESH empty lock dir is NOT taken over | new | `component/lock_judge::h11_a_fresh_empty_dir_is_not_taken_h11b_one_older_than_the_grace_is` |  |
| H11b | empty lock dir older than the grace: taken, empty-after-grace | new | `component/lock_judge::h11_a_fresh_empty_dir_is_not_taken_h11b_one_older_than_the_grace_is` |  |
| H12 | every takeover line names a reason from the list | new | `component/lock_cli::h7b_eight_takers_on_a_stale_lock_one_holder_one_takeover_line`<br>`component/lock_judge::h4_a_live_pid_older_than_max_age_is_taken_over`<br>`src/lock/judge::h12_every_reason_has_a_distinct_old_name` | the H3 and H7b lines in `component/lock_cli`; `one_takeover` in `component/lock_judge` checks the reason list on every takeover line there |
| H13 | Runner.Worker runs: agent does not take a free lock (76) | existing | `component/lock_ci::h13_a_running_runner_worker_keeps_an_agent_from_a_free_lock_76_without_ci_it_takes` |  |
| H13b | no CI: the same take succeeds | existing | `component/lock_ci::h13_a_running_runner_worker_keeps_an_agent_from_a_free_lock_76_without_ci_it_takes` |  |
| H13c | agent waits for Runner.Worker, then takes the lock | existing | `component/lock_ci::h13c_the_agent_waits_while_runner_worker_runs_and_goes_on_when_it_is_gone`<br>`src/lock/run::h13c_an_agent_waits_for_ci_and_then_takes` |  |
| H14 | waiting-list entry: agent refused (76) | existing | `component/lock_ci::h14_a_waiting_list_entry_blocks_an_agent_76_and_ci_status_says_so` |  |
| H14b | ci-done: ci-status 0 then 1, agent takes the lock | existing | `component/lock_ci::h14b_ci_done_frees_it_ci_status_is_1_then_the_agent_takes_and_the_list_is_gone` |  |
| H14c | old waiting-list entry is ignored | new | `component/lock_judge::h14c_an_entry_older_than_ci_max_age_is_ignored` |  |
| H15 | CI take proceeds past waiting CI and Runner.Worker | new | `component/lock_judge::h15_a_ci_take_passes_a_waiting_entry_and_a_running_worker_h15b_but_not_a_held_lock` |  |
| H15b | CI take is blocked by a HELD lock (75), entry removed | new | `component/lock_judge::h15_a_ci_take_passes_a_waiting_entry_and_a_running_worker_h15b_but_not_a_held_lock`<br>`src/lock/run::h15_a_ci_take_passes_waiting_ci_and_a_runner_but_a_held_lock_blocks_it` |  |
| H15c | waiting CI take is on the list while it waits, then gets the lock | existing | `src/lock/run::h15c_a_waiting_ci_take_is_on_the_list_while_it_waits_then_gets_the_lock` | unit level |
| H16 | no takeover of a stale lock while CI waits (76) | existing | `src/lock/run::h16_no_takeover_of_a_stale_lock_while_ci_waits_after_it_the_takeover_works` | unit level |
| H16b | CI gone: the stale lock is taken over | existing | `src/lock/run::h16_no_takeover_of_a_stale_lock_while_ci_waits_after_it_the_takeover_works` | unit level |
| H17 | --pid runner records the Runner.Worker pid | existing | `component/lock_cli::h17_pid_runner_records_the_worker_alive_blocks_killed_frees` |  |
| H17b | Runner.Worker alive: an agent does not take the lock (75) | existing | `component/lock_cli::h17_pid_runner_records_the_worker_alive_blocks_killed_frees` |  |
| H17c | Runner.Worker killed: the agent takes over (pid-dead) | new assertion | `component/lock_cli::h17_pid_runner_records_the_worker_alive_blocks_killed_frees` | the new owner after the takeover |
| H17d | walk finds Runner.Worker behind a full-path comm | existing | `component/lock_cli::h17d_the_walk_finds_runner_worker_behind_a_full_path_comm_with_the_hook`<br>`src/lock/run::h17d_the_runner_walk_finds_the_worker_behind_a_full_path_comm`<br>`src/lock/procs::h17d_the_walk_line_gives_parent_and_name_with_a_full_path` |  |
| H18 | --pid runner without a Runner.Worker ancestor: 64, nothing written | existing | `component/lock_cli::h18_pid_runner_without_a_runner_ancestor_is_64_and_writes_nothing_pid_0_works`<br>`src/lock/run::dispatch_take_without_a_runner_is_64_and_writes_nothing` |  |
| H18b | --pid 0 still works as before | existing | `component/lock_cli::e1_pid_0_needs_expect_names_run_and_writes_no_lock_recorded_with_it_not_needed_with_a_pid` |  |
| R1 | nested run in run passes at once; the outer lock stays until the outer ends | existing | `component/lock_reentry::r1_a_nested_run_in_run_passes_at_once_and_the_outer_lock_stays` |  |
| R2 | nested take under run: no second owner file; its release (any name) leaves the outer lock | existing | `component/lock_reentry::r2_a_nested_take_writes_no_second_owner_file_and_its_release_leaves_the_outer_lock`<br>`src/lock/run::r_re_entry_a_granted_ancestor_gets_the_lock_at_once_without_a_second_owner_file` |  |
| R3 | a non-ancestor holder still blocks take (75) and release (3), env or not | existing | `component/lock_reentry::r3_a_holder_that_is_not_an_ancestor_still_blocks_take_75_and_release_3`<br>`src/lock/run::r3_a_non_ancestor_holder_blocks_even_when_the_environment_names_its_pid` |  |
| R4 | ancestor holder without a grant: 77 at once (take and run), message names the parent | existing | `component/lock_reentry::r4_an_ancestor_holder_without_a_grant_is_77_at_once_take_and_run_and_the_message_names_the_parent` |  |
| R4b | release by the owner under an ancestor pid without a grant still frees the lock | existing | `component/lock_reentry::r4_an_ancestor_holder_without_a_grant_is_77_at_once_take_and_run_and_the_message_names_the_parent` |  |
| R5 | run exports CREWBOARD_HOST_LOCK (resolved dir) and _PID (holder) to the child, not outside | existing | `component/lock_reentry::r5_run_exports_the_resolved_dir_and_the_holder_pid_to_its_child_and_only_there` |  |
| R5b | take sets no variable in its caller | existing | `component/lock_reentry::r5_run_exports_the_resolved_dir_and_the_holder_pid_to_its_child_and_only_there` |  |
| R6 | symlink and ../ spellings of the dir are the same lock (re-entry and nested release) | existing | `component/lock_reentry::r6_a_symlink_and_a_dotdot_spelling_of_the_dir_are_the_same_lock`<br>`src/lock/run::r6_a_symlink_spelling_of_the_dir_is_the_same_lock` |  |
| R7 | run in run on two dirs: the inner child is granted its own dir (take under it passes) | existing | `component/lock_reentry::r7_the_grant_is_for_one_dir_the_inner_child_gets_its_own_dir_another_dir_held_by_an_ancestor_is_77` |  |
| R7b | a lock of a DIFFERENT dir held by an ancestor is not covered by the outer grant (77) | existing | `src/lock/run::r7_the_grant_is_for_one_dir_another_dir_is_not_covered` | unit level |
| H19 | ci-wait records the pid in the entry | new | `component/lock_judge::h19_an_entry_records_its_pid_a_dead_one_is_removed_a_live_one_blocks` |  |
| H19b | entry with a dead pid does not block an agent, is removed | new | `component/lock_judge::h19_an_entry_records_its_pid_a_dead_one_is_removed_a_live_one_blocks` |  |
| H19c | entry with a live pid blocks an agent (76) | new | `component/lock_judge::h19_an_entry_records_its_pid_a_dead_one_is_removed_a_live_one_blocks` |  |
| H19d | foreign-host pid and old entry without pid still block (76) | new | `component/lock_judge::h19d_an_entry_of_another_host_and_an_entry_without_a_pid_still_block` |  |
| F1 | empty dir older than the grace: stale, empty-after-grace | new | `component/lock_judge::f1_an_old_empty_dir_is_stale_f1b_an_old_dir_with_a_file_is_held` |  |
| F1b | old dir with an info file is held, not empty-after-grace | new | `component/lock_judge::f1_an_old_empty_dir_is_stale_f1b_an_old_dir_with_a_file_is_held` |  |
| F2 | foreign info file older than the grace blocks a take (75) | new | `component/lock_judge::f2_a_foreign_file_older_than_the_grace_blocks_f3_older_than_max_age_it_is_taken` |  |
| F3 | foreign dir older than --max-age is stale (too-old) and taken; --max-age 0 never | new | `component/lock_judge::f2_a_foreign_file_older_than_the_grace_blocks_f3_older_than_max_age_it_is_taken` |  |
| F4 | status and wait message name file and first line (capped, no control chars) | new | `component/lock_judge::f4_status_and_the_wait_message_name_the_file_and_its_first_line_capped_and_clean` |  |
| F5 | foreign dir with a DEAD recorded pid: stale (pid-dead), taken | new | `component/lock_judge::f5_a_recorded_pid_dead_is_stale_live_blocks_another_host_is_not_judged` |  |
| F5b | foreign dir with a LIVE recorded pid blocks (75) | new | `component/lock_judge::f5_a_recorded_pid_dead_is_stale_live_blocks_another_host_is_not_judged` |  |
| F5c | pid of another host not judged (75); same host dead pid taken | new | `component/lock_judge::f5_a_recorded_pid_dead_is_stale_live_blocks_another_host_is_not_judged` |  |
| F6 | owner.tmp.* only: fresh is kept (75), old is empty-after-grace | new | `component/lock_judge::f6_a_dir_with_only_the_temp_file_of_a_taker_is_kept_fresh_and_empty_when_old` |  |
| F6b | release of a foreign dir is refused (3), its file stays | existing | `component/lock_cli::f6b_release_of_a_foreign_dir_is_refused_3_and_its_file_stays` |  |
| O1 | one-line owner file older than --max-age (file mtime): too-old, taken | new | `component/lock_judge::o1_an_old_one_line_owner_file_is_too_old_by_its_own_mtime_o1b_not_with_max_age_0` |  |
| O1b | old one-line owner file, --max-age 0: held (75), file unchanged | new | `component/lock_judge::o1_an_old_one_line_owner_file_is_too_old_by_its_own_mtime_o1b_not_with_max_age_0` |  |
| O2 | fresh one-line owner file (old dir): held (status 0, take 75), named | new | `component/lock_judge::o2_a_fresh_one_line_owner_file_in_an_old_dir_is_held_and_named` |  |
| O3 | owner file without started_epoch=, dead pid: pid-dead, taken | new | `component/lock_judge::o3_an_owner_file_without_started_epoch_and_a_dead_pid_is_pid_dead` |  |
| O3b | invalid owner file, live or foreign-host pid: held (75); problem named | new | `component/lock_judge::o3b_an_invalid_owner_file_with_a_live_or_foreign_pid_is_held_and_its_problem_named` |  |
| O4 | release of an invalid owner file is refused (3); valid, it is released | new assertion | `component/lock_cli::o4_release_of_an_invalid_owner_file_is_refused_3_once_valid_it_is_released` | the `refused:` prefix on the same line |
| O5 | valid owner file with an old mtime: held by started_epoch (75) | new | `component/lock_judge::o5_a_valid_owner_file_with_an_old_mtime_is_judged_by_started_epoch` |  |
| O6 | owner is a directory: fresh held, old taken (too-old) | new | `component/lock_judge::o6_an_owner_that_is_a_directory_is_held_fresh_and_too_old_by_its_mtime` |  |
| G1 | 3 green runs: pass, 3 result lines printed | existing | `tests/repeat_cli::three_green_runs_pass` |  |
| G2 | a red run stops the loop and fails (exit 1, no run 3) | existing | `tests/repeat_cli::red_run_stops_the_loop` |  |
| G3 | old green line + new red line in run-1.log is RED | existing | `src/repeat/verdict::old_result_line_never_counts`<br>`tests/repeat_cli::old_result_line_never_counts` |  |
| G3b | old green line + no new result line is RED | existing | `src/repeat/verdict::old_result_line_never_counts`<br>`tests/repeat_cli::old_result_line_never_counts` |  |
| G4 | SHA change between runs fails (exit 2), not counted | existing | `tests/repeat_cli::sha_change_fails_exit_2` |  |
| G4b | stable SHA passes | existing | `tests/repeat_cli::three_green_runs_pass` |  |
| G5 | exit 0 but no result line is RED, not green | existing | `src/repeat/verdict::exit_0_without_result_line_is_red`<br>`tests/repeat_cli::exit_0_without_result_line_is_red` |  |
| G6 | exit 0 with '2 failed' in the result line is RED | existing | `src/repeat/verdict::failed_count_is_red`<br>`tests/repeat_cli::failed_count_is_red` |  |
| G6b | --result-re for another runner passes on 2 green runs | existing | `src/repeat/verdict::failed_count_is_red`<br>`tests/repeat_cli::failed_count_is_red` |  |
| G6c | '1 failed, 46 passed' is RED | existing | `src/repeat/verdict::failed_count_is_red`<br>`tests/repeat_cli::failed_count_is_red` |  |
| G10 | no --result-re: Jest exit 0 is green (exit code only) | new assertion | `tests/repeat_cli::the_runner_examples_of_the_help_text_judge_as_documented` | `PASS: 2 of 2` |
| G10b | no --result-re: exit 1 is RED, the loop stops | existing | `tests/repeat_cli::red_run_stops_the_loop` |  |
| G10c | --result-re '^Tests: ': '1 failed' red, '5 passed' green | existing | `tests/repeat_cli::the_runner_examples_of_the_help_text_judge_as_documented` |  |
| G10d | pytest --result-re: summary green, no summary red | existing | `tests/repeat_cli::the_runner_examples_of_the_help_text_judge_as_documented` |  |
| G10e | --help shows a Jest and a pytest example | existing | `tests/repeat_cli::the_help_shows_result_line_examples_for_several_stacks` |  |
| G7 | --lock: the lock is held during each run, free after | new assertion | `tests/repeat_cli::lock_held_during_each_run` | `lock status` during each run says `held: gl@x@hostA` |
| G7b | --lock held by a live owner: the run does not start | existing | `tests/repeat_cli::live_owner_blocks_the_run` |  |
| G9 | pauses while CI runs, resumes after, evidence notes it | existing | `tests/repeat_cli::a_ci_job_pauses_the_gate_and_the_evidence_says_so` |  |
| G9b | CI that never ends: exit 3, command not run | existing | `tests/repeat_cli::a_ci_job_that_never_ends_gives_exit_3_and_the_command_never_runs` |  |
| G9c | waiting-list entry pauses the loop; ci-done resumes it | existing | `tests/repeat_cli::a_waiting_list_entry_next_to_the_lock_pauses_the_gate` |  |
| G9d | no CI: no pause | existing | `tests/repeat_cli::a_waiting_list_entry_next_to_the_lock_pauses_the_gate` |  |
| G8 | bad --runs / missing --cmd -> 64 | existing | `tests/repeat_cli::bad_runs_and_missing_cmd_exit_64` |  |
| E1 | take --pid 0 without --expect: 64, names run, no lock | existing | `component/lock_cli::e1_pid_0_needs_expect_names_run_and_writes_no_lock_recorded_with_it_not_needed_with_a_pid`<br>`src/lock/args::e1_pid_zero_needs_expect_on_take` |  |
| E1b | take --pid 0 --expect 1: taken, expect=1 in the owner file | existing | `component/lock_cli::e1_pid_0_needs_expect_names_run_and_writes_no_lock_recorded_with_it_not_needed_with_a_pid`<br>`src/lock/args::e1_pid_zero_needs_expect_on_take` |  |
| E1c | take with a pid needs no --expect (no expect= line) | existing | `component/lock_cli::e1_pid_0_needs_expect_names_run_and_writes_no_lock_recorded_with_it_not_needed_with_a_pid`<br>`src/lock/args::e1_pid_zero_needs_expect_on_take` |  |
| E2 | expect 1, 290 s old: still held (status 0, take 75) | new | `component/lock_judge::e2_expect_1_is_held_for_290_s_and_overdue_after_310_s` |  |
| E2b | expect 1, 310 s old: stale (overdue), taken over | new | `component/lock_judge::e2_expect_1_is_held_for_290_s_and_overdue_after_310_s` |  |
| E3 | pid=0 without expect=, 2000 s old: held (old rule) | new | `component/lock_judge::e3_pid_0_without_expect_keeps_the_age_rule_and_a_judged_pid_is_never_overdue` |  |
| E3b | pid=0 without expect=, 20000 s old: too-old after 4 h | new | `component/lock_judge::e3_pid_0_without_expect_keeps_the_age_rule_and_a_judged_pid_is_never_overdue` |  |
| E3c | live pid with expect=1, 5000 s old: not overdue (pid judged) | new | `component/lock_judge::e3_pid_0_without_expect_keeps_the_age_rule_and_a_judged_pid_is_never_overdue` |  |
| E3d | other host with expect=1, 5000 s old: overdue | new | `component/lock_judge::e3_pid_0_without_expect_keeps_the_age_rule_and_a_judged_pid_is_never_overdue` |  |
| E4 | status: pid=0 note and overdue by 5 min | new | `component/lock_judge::e4_status_names_pid_0_and_how_late_the_job_is` |  |
| E4b | wait message: both warnings | existing | `src/lock/run::e4b_the_wait_message_carries_the_pid0_and_overdue_notes` | unit level |
| E4c | before expect: the pid=0 note, no overdue | new | `component/lock_judge::e4_status_names_pid_0_and_how_late_the_job_is` |  |
| E4d | a real pid: no pid=0 note | new | `component/lock_judge::e4_status_names_pid_0_and_how_late_the_job_is` |  |
| E5 | --expect 90, 90s, 30m, 2h records expect=the seconds | existing | `component/lock_cli::e5b_bad_durations_are_64_and_leave_nothing_behind` |  |
| E5b | bad durations (2d, soon, m): 64 | existing | `component/lock_cli::e5b_bad_durations_are_64_and_leave_nothing_behind`<br>`src/lock/args::e5b_bad_durations_are_usage_errors` |  |
| E6 | CREWBOARD_CONFIG_DIR: crewboard --config-dir <dir> status | new | `component/lock_board::e6_the_config_dir_variable_reaches_the_board_check` |  |
| E6b | --config-dir beats the variable; --initiative is passed | new | `component/lock_board::e6b_the_config_dir_option_beats_the_variable_and_the_initiative_is_used` |  |
| E6c | neither set: no --config-dir argument | existing | `component/lock_board::e6f_e2e_with_a_config_dir_the_check_works_and_without_one_status_says_unavailable_once`<br>`src/ops/lock_cmd::e6c_neither_set_gives_no_config_dir_argument` | the argument line of the stub is the shape of the `lock --crewboard` seam |
| E6d | no config: status says 'unavailable' once, no failure note | existing | `component/lock_board::e6f_e2e_with_a_config_dir_the_check_works_and_without_one_status_says_unavailable_once`<br>`src/lock/board::e6d_no_config_status_says_unavailable_once_and_no_failure_note` |  |
| E6e | no config: take and wait stay silent about it | existing | `src/lock/board::e6e_no_config_take_and_wait_stay_silent` | unit level |
| E6f | with the config dir the board check works: board-dead takeover | new | `component/lock_board::e6_the_config_dir_variable_reaches_the_board_check`<br>`component/lock_board::e6f_e2e_with_a_config_dir_the_check_works_and_without_one_status_says_unavailable_once` |  |
| DT1 | detach: returns at once, job holds the lock, owner has its pid | existing | `component/lock_detach::dt1_detach_returns_at_once_the_job_holds_the_lock_with_its_pid_and_detached_1` |  |
| DT1b | detached lock: another taker gets 75, status says detached | existing | `component/lock_detach::dt1_detach_returns_at_once_the_job_holds_the_lock_with_its_pid_and_detached_1` |  |
| DT2 | detach: result file rc=5 end= log=, lock released | existing | `component/lock_detach::dt1_detach_returns_at_once_the_job_holds_the_lock_with_its_pid_and_detached_1` |  |
| DT2b | detach --ticket --sha: result file has ticket= and sha= | existing | `component/lock_detach::dt2b_ticket_and_sha_add_their_lines_the_first_line_stays` |  |
| DT2c | --ticket needs digits and --detach | existing | `component/lock_detach::dt2c_ticket_needs_digits_and_detach`<br>`src/lock/args::dt2c_ticket_needs_digits_and_detach_sha_needs_alphanumerics` |  |
| DT3 | killed wake: detached job and its lock stay | existing | `component/lock_detach::dt3_a_kill_of_the_caller_group_leaves_the_job_and_its_lock_the_job_ends_later` |  |
| DT3b | the job finishes after the wake: result rc=0, lock released | existing | `component/lock_detach::dt3_a_kill_of_the_caller_group_leaves_the_job_and_its_lock_the_job_ends_later` |  |
| DT4 | --detach without --result / --result without --detach -> 64 | existing | `component/lock_cli::dt4_detach_and_result_go_together_64`<br>`src/lock/args::dt4_detach_and_result_go_together_and_only_with_run` |  |
| DT6 | release by a stranger: exit 3, lock complete | existing | `component/lock_cli::dt6_release_by_a_stranger_is_3_and_the_lock_is_complete` |  |
| GT1 | CI job runs: plain run -> 76, run --gate runs the job | existing | `component/lock_ci::gt1_a_ci_job_runs_plain_run_is_76_run_gate_runs_the_job`<br>`src/lock/run::gt1_a_gate_skips_the_ci_wait_but_a_held_lock_still_blocks_it` |  |
| GT2 | --gate still waits for a held lock (75) | existing | `component/lock_ci::gt2_gate_still_waits_for_a_held_lock_75` |  |
| GT3 | --gate --detach skips the CI wait | existing | `component/lock_ci::gt3_gate_with_detach_skips_the_ci_wait` |  |
| GT4 | --detach without --gate waits for CI first (76) | existing | `component/lock_ci::gt4_detach_without_gate_waits_for_ci_first_76_nothing_started` |  |
| OP1 | release --wait: exit 64, the lock stays | existing | `component/lock_cli::op1_to_op3_an_option_that_does_not_apply_is_64_the_lock_stays` |  |
| OP2 | status --pid: exit 64 | existing | `component/lock_cli::op1_to_op3_an_option_that_does_not_apply_is_64_the_lock_stays` |  |
| OP3 | break --reason: exit 64, the lock stays | existing | `src/lock/args::op1_to_op3_an_option_that_does_not_apply_is_refused_with_its_name` | unit level |
