//! The commands that set up a crew and its agents: `init`, `join`, `approve`, `start`,
//! `pause`, `resume`, `stop`, `heartbeat`, `prune` and `audit`. The code of `agent selftest` and
//! `agent profile` is shared (`selftest_cmd`, `profile_cmd`).

use super::{Cli, CrewCmd, Outcome, ProfileAction, Sel, done, fail, global, refused};
use crate::error::CbError;
use crate::exit::Exit;
use crate::ops::approve::Approval;
use crate::ops::{self, AgentCtx};
use crate::owner_cmd::Verb;
use crate::rules::Refusal;

/// The last line of `approve`: what the approved agents do next. Pure.
pub(super) fn approve_next_step(approved: &[String]) -> String {
    format!(
        "next step: tell the agent host of {} to run its `crewboard agent add` command again; it then runs `start`",
        approved.join(", ")
    )
}

/// `approve` of one or more agents. One name: the output
/// and the exit code of today. Several names: each is approved on its own, its output under a
/// header line; a failure does not stop the others. The exit code is that of the first
/// failure (all messages are printed on stderr), or 0. The next step names the agents whose
/// approval changed something.
pub(super) fn approve_all(
    names: &[String],
    one: impl FnMut(&str) -> Result<Approval, CbError>,
) -> Outcome {
    let mut out = String::new();
    let o = approve_to(&mut out, names, one);
    print!("{out}");
    o
}

/// [`approve_all`] with the standard output collected in `out`.
pub(super) fn approve_to(
    out: &mut String,
    names: &[String],
    mut one: impl FnMut(&str) -> Result<Approval, CbError>,
) -> Outcome {
    if let [name] = names {
        return match one(name) {
            Ok(a) => {
                // the text goes first, as before; the next step is the last line
                out.push_str(&a.text);
                if a.changed {
                    out.push_str(&format!("{}\n", approve_next_step(names)));
                }
                done_ok()
            }
            Err(e) => fail(e),
        };
    }
    let mut exit = Exit::Ok;
    let mut failed: Vec<String> = Vec::new();
    let mut changed: Vec<String> = Vec::new();
    for n in names {
        match one(n) {
            Ok(a) => {
                if a.changed {
                    changed.push(n.clone());
                }
                out.push_str(&format!("approve {n}:\n{}", a.text));
            }
            Err(e) => {
                if exit == Exit::Ok {
                    exit = e.exit();
                }
                out.push_str(&format!("approve {n}: FAILED (see stderr)\n"));
                failed.push(format!("approve {n}: {e}"));
            }
        }
    }
    let ok = names.len() - failed.len();
    out.push_str(&format!("approved {ok} of {}\n", names.len()));
    if !changed.is_empty() {
        out.push_str(&format!("{}\n", approve_next_step(&changed)));
    }
    Outcome {
        exit,
        code: None,
        message: (!failed.is_empty()).then(|| failed.join("\n")),
    }
}

fn done_ok() -> Outcome {
    Outcome {
        exit: Exit::Ok,
        code: None,
        message: None,
    }
}

fn lifecycle_command(cli: &Cli, sel: &Sel, agent: Option<&str>, verb: Verb) -> Outcome {
    match agent {
        Some(a) => done(ops::lifecycle::run_owner(&global(cli, sel, false), verb, a)),
        None => done(
            AgentCtx::load(&global(cli, sel, false))
                .and_then(|ac| ops::lifecycle::run_agent(&ac, verb)),
        ),
    }
}

/// `stop`: the agent's own, the owner's command, or `--force` (owner or coordinator).
fn stop_command(cli: &Cli, sel: &Sel, agent: Option<&str>, force: bool) -> Outcome {
    match (agent, force) {
        (Some(a), true) => done(ops::force_stop::run(&global(cli, sel, false), a)),
        _ => lifecycle_command(cli, sel, agent, Verb::Stop),
    }
}

/// `agent selftest` (old name `selftest`): prove on the live board that this agent sees events.
pub(super) fn selftest_cmd(cli: &Cli, sel: &Sel) -> Outcome {
    let r = AgentCtx::load(&global(cli, sel, false)).and_then(|ac| ops::selftest::run(&ac));
    match r {
        Ok((res, line)) if res.passed() => {
            println!("{line}");
            done_ok()
        }
        Ok((_, line)) => refused(Refusal::new("selftest-failed", line)),
        Err(e) => fail(e),
    }
}

/// `agent profile show|set` (old name `profile`).
pub(super) fn profile_cmd(cli: &Cli, action: &ProfileAction) -> Outcome {
    match action {
        ProfileAction::Set {
            tags,
            untags,
            tier,
            clear_tier,
            about_file,
            agent,
            sel,
        } => {
            let change = match ops::profile::prepare(&ops::profile::SetArgs {
                tags: tags.clone(),
                untags: untags.clone(),
                tier: tier.clone(),
                clear_tier: *clear_tier,
                about_file: about_file.clone(),
            }) {
                Ok(c) => c,
                Err(e) => return fail(e),
            };
            match agent {
                Some(a) => done(ops::profile::set_agent(
                    &global(cli, sel, false),
                    a,
                    &change,
                )),
                None => done(
                    AgentCtx::load(&global(cli, sel, false))
                        .and_then(|ac| ops::profile::set_own(&ac, &change)),
                ),
            }
        }
        ProfileAction::Show { agent, json, sel } => done(ops::profile::show(
            &global(cli, sel, false),
            agent.as_deref(),
            *json,
        )),
    }
}

#[inline(never)]
pub(super) fn dispatch(cli: &Cli, c: &CrewCmd) -> Outcome {
    let none = Sel::default();
    match c {
        CrewCmd::Init {
            initiative,
            repos,
            project,
            title,
            owner_login,
            crew_login,
            insecure_one_account,
            no_pr,
            no_branch_rule,
            with_ci,
        } => done(ops::init::run(
            &global(cli, &none, false),
            ops::init::InitArgs {
                initiative,
                repos,
                project: *project,
                title: title.as_deref(),
                owner_login: owner_login.as_deref(),
                crew_login: crew_login.as_deref(),
                insecure_one_account: *insecure_one_account,
                no_pr: *no_pr,
                no_branch_rule: *no_branch_rule,
                with_ci: *with_ci,
            },
        )),
        CrewCmd::Join {
            agent,
            runtime,
            model,
            host,
            repo,
            tools,
            insecure_one_account,
            sel,
        } => done(ops::join::run(
            &global(cli, sel, *insecure_one_account),
            ops::join::JoinArgs {
                name: agent,
                runtime: *runtime,
                model: model.as_deref(),
                host: host.as_deref(),
                repo: repo.as_deref(),
                tools,
            },
        )),
        CrewCmd::Approve { names, role, sel } => {
            let g = global(cli, sel, false);
            approve_all(names, |n| ops::approve::run(&g, n, *role))
        }
        CrewCmd::Start {
            insecure_one_account,
            sel,
        } => {
            let g = global(cli, sel, *insecure_one_account);
            done(AgentCtx::load(&g).and_then(|ac| ops::start::run(&ac)))
        }
        CrewCmd::Resume { agent: None, sel } => {
            done(AgentCtx::load(&global(cli, sel, false)).and_then(|ac| ops::restart::run(&ac)))
        }
        CrewCmd::Prune { sel } => done(
            AgentCtx::load(&global(cli, sel, false)).and_then(|mut ac| ops::prune::run(&mut ac)),
        ),
        CrewCmd::Pause { agent, sel } => lifecycle_command(cli, sel, agent.as_deref(), Verb::Pause),
        CrewCmd::Resume { agent, sel } => {
            lifecycle_command(cli, sel, agent.as_deref(), Verb::Resume)
        }
        CrewCmd::Stop { agent, force, sel } => stop_command(cli, sel, agent.as_deref(), *force),
        CrewCmd::Heartbeat { sel } => done(
            AgentCtx::load(&global(cli, sel, false)).and_then(|ac| ops::lifecycle::heartbeat(&ac)),
        ),
        CrewCmd::Audit { json, since, sel } => done(ops::audit::run(
            &global(cli, sel, false),
            *json,
            since.as_deref(),
        )),
        CrewCmd::Move {
            issue,
            status,
            why,
            sel,
        } => done(ops::owner_move::run(
            &global(cli, sel, false),
            issue,
            status,
            why.as_deref(),
        )),
    }
}
