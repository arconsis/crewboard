//! The commands that read events and the board: `status`, `watch`, `monitor`, `poll`, `mine`
//! and `acted`.

use super::{
    Cli, Command, MonitorArgs, Outcome, Sel, WatchCmd, done, fail, fail_with_line, global,
};
use crate::base::ignore::best_effort_output;
use crate::error::CbError;
use crate::exit::Exit;
use crate::ops::{self, AgentCtx, Global};
use clap::Parser;
use std::path::PathBuf;

/// The status line for an error that happened before a cycle could start (for example the
/// network was down while the board was loaded). Reads only the local cache.
fn early_status_line(g: &Global) -> String {
    let cache = g
        .dir()
        .and_then(|cd| {
            let a = cd.load_agent_auto(g.initiative.as_deref())?;
            let name = a.agent_name()?;
            Ok(crate::state::StateCache::load(
                &cd.state_path(&a.board.initiative, &name),
            ))
        })
        .unwrap_or_default();
    ops::watch::status_line(
        &cache.last_cursor_hint(),
        cache.poll.polls,
        cache.poll.last_ok.as_deref(),
    )
}

/// The outcome of `monitor` with the code of its last line: the line goes to standard output,
/// where a Monitor tool reads it.
fn monitor_outcome(code: u8) -> Outcome {
    Outcome {
        exit: match code {
            0 => Exit::Ok,
            3 => Exit::Network,
            4 => Exit::Config,
            _ => Exit::Refused,
        },
        code: Some(code),
        message: None,
    }
}

pub(super) fn monitor_fail(code: u8, reason: &str, status: &str, json: bool) -> Outcome {
    use std::io::Write;
    // a closed pipe must not panic: the exit code still tells
    let mut out = std::io::stdout();
    best_effort_output(writeln!(
        out,
        "{}",
        ops::monitor::exit_line(code, reason, status, json)
    ));
    best_effort_output(out.flush());
    monitor_outcome(code)
}

/// The line that `monitor` prints for a command line that clap refuses (`main` prints it).
pub fn monitor_usage_line(clap_error: &str, json: bool) -> String {
    ops::monitor::exit_line(4, &ops::monitor::first_line(clap_error), "", json)
}

/// What `watch` gets from the flags of `monitor` and from the arguments after `--` (they win).
#[derive(Debug)]
pub(super) struct WatchSettings {
    pub(super) timeout: Option<String>,
    pub(super) interval: Option<String>,
    pub(super) json: bool,
    pub(super) reset_cursor: bool,
    pub(super) sel: Sel,
}

/// The settings of `watch`: the flags of `monitor`, then the arguments after `--` on top. An
/// argument that `watch` does not take is an error, also a global option (`--config-dir` and
/// `--heartbeat-interval` go before `monitor`). Pure.
pub(super) fn watch_settings(m: &MonitorArgs) -> Result<WatchSettings, String> {
    let mut w = WatchSettings {
        timeout: m.timeout.clone(),
        interval: m.interval.clone(),
        json: false,
        reset_cursor: m.reset_cursor,
        sel: m.sel.clone(),
    };
    if m.watch_args.is_empty() {
        return Ok(w);
    }
    let argv = ["crewboard", "watch"]
        .into_iter()
        .map(String::from)
        .chain(m.watch_args.iter().cloned());
    let parsed = Cli::try_parse_from(argv).map_err(|e| ops::monitor::first_line(&e.to_string()))?;
    if parsed.config_dir.is_some() || parsed.env_file.is_some() {
        return Err("--config-dir and --env-file go before `monitor`".into());
    }
    let Command::Watch(WatchCmd::Watch {
        timeout,
        interval,
        json,
        reset_cursor,
        sel,
        ..
    }) = parsed.command
    else {
        return Err("only the arguments of `watch` are allowed".into());
    };
    w.timeout = timeout.or(w.timeout);
    w.interval = interval.or(w.interval);
    w.json = json;
    w.reset_cursor |= reset_cursor;
    w.sel.initiative = sel.initiative.or(w.sel.initiative);
    w.sel.initiative_file = sel.initiative_file.or(w.sel.initiative_file);
    Ok(w)
}

/// The command line of another binary's `watch` (the test seam): the arguments before `watch`
/// and those after it. Pure.
pub(super) fn watch_command_args(cli: &Cli, w: &WatchSettings) -> (Vec<String>, Vec<String>) {
    let mut before = Vec::new();
    if let Some(d) = &cli.config_dir {
        before.extend(["--config-dir".to_string(), d.to_string_lossy().into_owned()]);
    }
    let mut after = Vec::new();
    for (flag, v) in [("--timeout", &w.timeout), ("--interval", &w.interval)] {
        if let Some(v) = v {
            after.extend([flag.to_string(), v.clone()]);
        }
    }
    if let Some(i) = &w.sel.initiative {
        after.extend(["--initiative".to_string(), i.clone()]);
    }
    if let Some(f) = &w.sel.initiative_file {
        after.extend([
            "--initiative-file".to_string(),
            f.to_string_lossy().into_owned(),
        ]);
    }
    if w.reset_cursor {
        after.push("--reset-cursor".to_string());
    }
    if w.json {
        after.push("--json".to_string());
    }
    (before, after)
}

/// The options of `watch --loop` as those of `monitor`: the same loop, so the same code runs.
/// `--json` is the form of the last line, and it also goes to `watch` for the events. Pure.
pub(super) fn loop_args(
    timeout: &Option<String>,
    interval: &Option<String>,
    json: bool,
    reset_cursor: bool,
    extra: (&Option<String>, Option<u64>, &Option<PathBuf>),
    sel: &Sel,
) -> MonitorArgs {
    let (pause, max_cycles, log) = extra;
    MonitorArgs {
        timeout: timeout.clone(),
        interval: interval.clone(),
        pause: pause.clone(),
        max_cycles: max_cycles.unwrap_or(0),
        log: log.clone(),
        json,
        reset_cursor,
        crewboard: None,
        sel: sel.clone(),
        watch_args: if json {
            vec!["--json".to_string()]
        } else {
            Vec::new()
        },
    }
}

/// `monitor`: its own function so that the stack frame of `dispatch` stays small.
#[inline(never)]
fn monitor_cmd(cli: &Cli, m: &MonitorArgs) -> Outcome {
    use ops::monitor::{InProcess, LoopOpts, Subprocess, run_on_wall_clock};
    let json = m.json;
    // flags are checked before any config or network
    for (name, v) in [("--timeout", &m.timeout), ("--interval", &m.interval)] {
        if let Some(v) = v
            && let Err(e) = crate::clock::parse_duration(v)
        {
            return monitor_fail(4, &format!("{name}: {e}"), "", json);
        }
    }
    let pause = match m.pause.as_deref() {
        None => ops::monitor::DEFAULT_PAUSE,
        Some(p) => match ops::monitor::parse_pause(p) {
            Some(d) => d,
            None => {
                return monitor_fail(
                    4,
                    &format!("--pause: not a number of seconds (at most 86400): {p}"),
                    "",
                    json,
                );
            }
        },
    };
    let w = match watch_settings(m) {
        Ok(w) => w,
        Err(e) => return monitor_fail(4, &format!("watch arguments: {e}"), "", json),
    };
    for (name, v) in [("--timeout", &w.timeout), ("--interval", &w.interval)] {
        if let Some(v) = v
            && let Err(e) = crate::clock::parse_duration(v)
        {
            return monitor_fail(4, &format!("watch arguments: {name}: {e}"), "", json);
        }
    }
    let opts = LoopOpts {
        pause,
        max_cycles: m.max_cycles,
        log: m.log.clone(),
        json,
    };
    let mut out = std::io::stdout();
    let code = match m.crewboard.as_deref() {
        Some(bin) if !ops::lock_cmd::is_this_binary(bin) => {
            let (before, after) = watch_command_args(cli, &w);
            let mut cycle = Subprocess {
                bin: bin.to_string(),
                before,
                after,
            };
            run_on_wall_clock(&mut cycle, &opts, &mut out)
        }
        _ => {
            let g = global(cli, &w.sel, false);
            let ac = match AgentCtx::load(&g) {
                Ok(a) => a,
                Err(e) => {
                    let reason = ops::monitor::first_line(&e.to_string());
                    return monitor_fail(e.exit().code(), &reason, &early_status_line(&g), json);
                }
            };
            let (t, i) = match ops::watch::loop_durations(
                w.timeout.as_deref(),
                w.interval.as_deref(),
                ac.agent.loop_cfg.as_ref(),
            ) {
                Ok(d) => d,
                Err(e) => {
                    let reason = ops::monitor::first_line(&e.to_string());
                    return monitor_fail(e.exit().code(), &reason, &early_status_line(&g), json);
                }
            };
            let mut cycle = InProcess {
                ac: &ac,
                timeout: t,
                interval: i,
                opts: ops::watch::PollOpts {
                    json: w.json,
                    quiet: false,
                    reset_cursor: w.reset_cursor,
                },
                log: m.log.clone(),
            };
            run_on_wall_clock(&mut cycle, &opts, &mut out)
        }
    };
    monitor_outcome(code)
}

/// `watch` without `--loop`: wait once for events, print them, exit.
fn watch_once(
    cli: &Cli,
    (timeout, interval): (&Option<String>, &Option<String>),
    (json, reset_cursor): (bool, bool),
    sel: &Sel,
) -> Outcome {
    // flags are checked before any config or network
    for v in [timeout, interval].into_iter().flatten() {
        if let Err(e) = crate::clock::parse_duration(v) {
            return fail(CbError::config(e));
        }
    }
    let ac = match AgentCtx::load(&global(cli, sel, false)) {
        Ok(a) => a,
        Err(e) => return fail_with_line(e, early_status_line(&global(cli, sel, false))),
    };
    let (t, i) = match ops::watch::loop_durations(
        timeout.as_deref(),
        interval.as_deref(),
        ac.agent.loop_cfg.as_ref(),
    ) {
        Ok(d) => d,
        Err(e) => return fail(e),
    };
    let opts = ops::watch::PollOpts {
        json,
        quiet: false,
        reset_cursor,
    };
    match ops::watch::watch(
        &ac,
        t,
        i,
        opts,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    ) {
        Ok(exit) => Outcome {
            exit,
            code: None,
            message: None,
        },
        Err((e, line)) => fail_with_line(e, line),
    }
}

#[inline(never)]
pub(super) fn dispatch(cli: &Cli, c: &WatchCmd) -> Outcome {
    match c {
        // `status --agents` is the old command `agents`
        WatchCmd::Status {
            json,
            agents: true,
            can,
            sel,
            ..
        } => done(ops::agents::run(&global(cli, sel, false), can, *json)),
        WatchCmd::Status {
            json,
            verbose,
            queue,
            sel,
            ..
        } => done(ops::status::run(
            &global(cli, sel, false),
            *json,
            *verbose,
            *queue,
        )),
        // `watch --loop` is the old command `monitor`: the same loop and the same last line
        WatchCmd::Watch {
            timeout,
            interval,
            json,
            reset_cursor,
            watch_loop: true,
            pause,
            max_cycles,
            log,
            sel,
        } => monitor_cmd(
            cli,
            &loop_args(
                timeout,
                interval,
                *json,
                *reset_cursor,
                (pause, *max_cycles, log),
                sel,
            ),
        ),
        WatchCmd::Watch {
            timeout,
            interval,
            json,
            reset_cursor,
            sel,
            ..
        } => watch_once(cli, (timeout, interval), (*json, *reset_cursor), sel),
        WatchCmd::Monitor { m } => monitor_cmd(cli, m),
        WatchCmd::Poll {
            quiet,
            json,
            reset_cursor,
            sel,
        } => {
            let ac = match AgentCtx::load(&global(cli, sel, false)) {
                Ok(a) => a,
                Err(e) => return fail_with_line(e, early_status_line(&global(cli, sel, false))),
            };
            let opts = ops::watch::PollOpts {
                json: *json,
                quiet: *quiet,
                reset_cursor: *reset_cursor,
            };
            match ops::watch::poll(&ac, opts, &mut std::io::stdout()) {
                Ok(exit) => Outcome {
                    exit,
                    code: None,
                    message: None,
                },
                Err((e, line)) => fail_with_line(e, line),
            }
        }
        WatchCmd::Mine {
            issue: Some(issue),
            json,
            sel,
            ..
        } => match AgentCtx::load(&global(cli, sel, false))
            .and_then(|ac| ops::mine_digest::run(&ac, issue))
        {
            Ok(d) => {
                if *json {
                    println!("{}", d.to_json());
                } else {
                    print!("{}", d.render());
                }
                Outcome {
                    exit: Exit::Ok,
                    code: None,
                    message: None,
                }
            }
            Err(e) => fail(e),
        },
        WatchCmd::Mine {
            issue: None,
            quiet,
            json,
            sel,
        } => {
            match AgentCtx::load(&global(cli, sel, false)).and_then(|ac| ops::mine::run(&ac)) {
                Ok(view) => {
                    if *json {
                        println!("{}", view.to_json());
                    } else if !*quiet {
                        print!("{}", view.render());
                    }
                    Outcome {
                        // Exit 1 = nothing to do now, as `poll` exit 1 = no events
                        exit: if view.needs_agent() {
                            Exit::Ok
                        } else {
                            Exit::NoEvents
                        },
                        code: None,
                        message: None,
                    }
                }
                Err(e) => fail(e),
            }
        }
        WatchCmd::Acted {
            events_file,
            since,
            list,
            redeliver_to,
            sel,
        } => {
            if *list {
                done(
                    std::fs::read_to_string(events_file)
                        .map(|t| ops::acted::list(&t))
                        .map_err(|e| {
                            CbError::config(format!("cannot read {}: {e}", events_file.display()))
                        }),
                )
            } else {
                done(AgentCtx::load(&global(cli, sel, false)).and_then(|ac| {
                    ops::acted::run(&ac, events_file, since.as_deref(), redeliver_to.as_deref())
                }))
            }
        }
    }
}
