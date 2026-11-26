//! The next-step texts of `agent add`, for the owner and the agent to copy.

use super::output::PREFIX;
use super::plan::{Plan, Target};
use crate::ops::agent::AgentRuntime;
use crate::ops::agent::shim::sq;
use crate::ops::agent::wait::Wait;

/// The commands that this run printed or will print, for the owner to copy. Quoted for a shell.
fn agent_add_again(p: &Plan) -> String {
    let mut s = String::from("crewboard agent add ");
    match &p.target {
        Target::Repo(r) => s.push_str(r),
        Target::File(f) => {
            s.push_str(&sq(&f.display().to_string()));
            s.push_str(&format!(" --repo {}", p.repo));
        }
    }
    if let (Target::Repo(_), Some(f)) = (&p.target, &p.board_file) {
        s.push_str(&format!(" --board-file {}", sq(&f.display().to_string())));
    }
    s.push_str(&format!(
        " --agent-dir {} --agent {} --runtime {} --work-dir {} --env-file {}",
        sq(&p.agent_dir.display().to_string()),
        p.name,
        p.runtime.bin(),
        sq(&p.work_dir.display().to_string()),
        sq(&p.env_file.display().to_string()),
    ));
    if let Some(m) = &p.model {
        s.push_str(&format!(" --model {}", sq(m)));
    }
    if let Some(l) = &p.crew_login {
        s.push_str(&format!(" --crew-login {l}"));
    }
    if p.coordinator {
        s.push_str(" --role coordinator");
    }
    if let Some((w, _)) = &p.wake {
        s.push_str(&w.flags());
    }
    s
}

/// The line while the owner has not approved the agent.
pub(super) fn approve_line(p: &Plan, initiative: &str) -> String {
    let owner = if p.coordinator {
        format!(
            "crewboard approve {} --initiative {initiative} --role coordinator",
            p.name
        )
    } else {
        format!(
            "crewboard approve {} --initiative {initiative}; add --role coordinator for the coordinator",
            p.name
        )
    };
    format!(
        "next step: ask the owner to approve {} (owner: {owner}), then run: {}",
        p.name,
        agent_add_again(p)
    )
}

pub(in crate::ops::agent) fn scheduler_for(rt: AgentRuntime) -> &'static str {
    if rt == AgentRuntime::Hermes {
        "hermes"
    } else if cfg!(target_os = "macos") {
        "launchd"
    } else {
        "systemd"
    }
}

/// The first prompt of an interactive session of this agent, as one shell word. Claude Code and
/// Codex take it as the argument; Hermes runs from its cron job and gets none.
pub(super) fn first_prompt(p: &Plan, initiative: &str) -> Option<String> {
    if p.runtime == AgentRuntime::Hermes {
        return None;
    }
    let role = if p.coordinator {
        format!("the coordinator of {initiative}")
    } else {
        format!("a dev agent of {initiative}")
    };
    Some(sq(&format!(
        "Use the crewboard skill. You are {}, {role}. Run `crewboard resume`. Then start the watch command that it prints under Monitor (persistent).",
        p.name
    )))
}

/// The runtime command of the paste line: the executable and its first prompt.
fn runtime_words(p: &Plan, initiative: &str) -> String {
    match first_prompt(p, initiative) {
        Some(prompt) => format!("{} {prompt}", p.runtime.bin()),
        None => p.runtime.bin().to_string(),
    }
}

/// The lines after a good `start`. The first one introduces the command; the second is the bare
/// command to paste (no prefix, nothing after it); the third is an optional hint.
pub(super) fn after_start_lines(p: &Plan, initiative: &str) -> [String; 3] {
    let dir = sq(&p.agent_dir.display().to_string());
    let wd = sq(&p.work_dir.display().to_string());
    let env = sq(&p.env_file.display().to_string());
    let exec = format!("cd {wd} && crewboard agent exec --agent-dir {dir} --");
    let command = format!("{exec} {}", runtime_words(p, initiative));
    let hint = if p.coordinator {
        let head = format!("{}-headless", p.agent_dir.display());
        format!(
            "optional: the headless tick of this coordinator: crewboard agent add --role headless-coordinator --from {dir} --agent-dir {} --env-file {env}",
            sq(&head),
        )
    } else {
        format!(
            "optional: an unattended agent: crewboard wake install --scheduler {} --runtime {} --agent-dir {dir} --initiative {initiative} --env-file {env} --work-dir {wd} --shim-dir {}",
            scheduler_for(p.runtime),
            p.runtime.bin(),
            sq(&p.agent_dir.join("bin").display().to_string()),
        )
    };
    [
        "next step: start the runtime in the work dir:".to_string(),
        command,
        hint,
    ]
}

/// The last line after the unit is installed. Pure.
pub(super) fn after_wake_line(p: &Plan, next: Option<&str>) -> String {
    match next {
        Some(cmd) => format!(
            "next step: {cmd} (it keeps the loop of {} running after logout)",
            p.name
        ),
        None => format!(
            "next step: none; the scheduler wakes {} when work exists (check: crewboard wake status --agent-dir {})",
            p.name,
            sq(&p.agent_dir.display().to_string())
        ),
    }
}

/// The lines for the owner and for the wait. Pure.
pub(super) fn waiting_lines(p: &Plan, initiative: &str, w: &Wait) -> Vec<String> {
    vec![
        format!(
            "{PREFIX} [7/7] start: waiting: the owner has not approved {} yet",
            p.name
        ),
        format!(
            "{PREFIX} owner: run this on the owner machine: {}",
            owner_approve_command(p, initiative)
        ),
        format!(
            "{PREFIX} checking every {} s, for up to {} min; Ctrl-C stops, the same command run again goes on",
            w.interval.as_secs(),
            w.max.as_secs() / 60
        ),
    ]
}

/// The command that the owner runs to approve this agent.
pub(super) fn owner_approve_command(p: &Plan, initiative: &str) -> String {
    let role = if p.coordinator {
        " --role coordinator"
    } else {
        ""
    };
    format!(
        "crewboard approve {} --initiative {initiative}{role}",
        p.name
    )
}
