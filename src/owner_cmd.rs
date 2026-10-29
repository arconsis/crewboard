//! Owner commands (spec/board#lifecycle, spec/messages#owner-messages): `/crew
//! approve|reject|role|profile|pause|resume|stop ...` in the registry issue, and `/crew move
//! <status>` on a ticket (spec/tickets#owner-move).
//!
//! An owner command is the first line of an owner comment (a comment of the
//! owner's account without a crew header). Parsing is pure. Whether the author may issue
//! it is decided by the caller from the GitHub login.

use crate::protocol::names::AgentName;
use crate::rules::status::Status;
use crate::rules::{Refusal, Role};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerCommand {
    /// `/crew approve <name> [role=dev|coordinator] [key=<fingerprint>] [item=<id>]
    /// [login=<github-login>]`
    Approve {
        name: AgentName,
        role: Role,
        /// `SHA256:...` of the public key the owner approved.
        key_fp: Option<String>,
        item: Option<String>,
        /// The GitHub login that wrote the join: the account the owner approved. Without it the
        /// approval binds the account of the join.
        login: Option<String>,
    },
    /// `/crew reject <name>`
    Reject {
        name: AgentName,
    },
    /// `/crew role <name> coordinator|dev`
    SetRole {
        name: AgentName,
        role: Role,
    },
    /// `/crew profile <name> [+tag] [-tag] [tier=<t>|tier=none] [about=set|about=clear]`
    /// (spec/profiles#who-may-change-a-profile-and-the-history). With `about=set` the lines after
    /// the first are the new background text.
    Profile {
        name: AgentName,
        tags: Vec<String>,
        untags: Vec<String>,
        tier: TierOp,
        about: AboutOp,
    },
    Pause {
        name: AgentName,
    },
    Resume {
        name: AgentName,
    },
    Stop {
        name: AgentName,
    },
    /// `/crew move <status>` on a ticket: the owner moved the ticket by hand
    /// (spec/tickets#owner-move). The lines after the first are the reason. Done and Dropped are
    /// no target: a ticket ends through the coordinator's signed close or drop.
    Move {
        status: Status,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TierOp {
    Keep,
    Set(String),
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AboutOp {
    Keep,
    /// The text is in the lines after the first line of the comment.
    Set,
    Clear,
}

/// The three lifecycle commands of the owner (spec/board#lifecycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Pause,
    Resume,
    Stop,
}

impl Verb {
    pub fn word(self) -> &'static str {
        match self {
            Verb::Pause => "pause",
            Verb::Resume => "resume",
            Verb::Stop => "stop",
        }
    }
}

/// `(verb, agent)` of a lifecycle command; `None` for approve, reject and role.
pub fn lifecycle_of(c: &OwnerCommand) -> Option<(Verb, &AgentName)> {
    match c {
        OwnerCommand::Pause { name } => Some((Verb::Pause, name)),
        OwnerCommand::Resume { name } => Some((Verb::Resume, name)),
        OwnerCommand::Stop { name } => Some((Verb::Stop, name)),
        _ => None,
    }
}

/// The latest lifecycle command for `name` among `cmds` (comment id, command), by comment id.
pub fn latest_verb_for(cmds: &[(u64, OwnerCommand)], name: &str) -> Option<(u64, Verb)> {
    cmds.iter()
        .filter_map(|(id, c)| {
            lifecycle_of(c)
                .filter(|(_, n)| n.as_str() == name)
                .map(|(v, _)| (*id, v))
        })
        .max_by_key(|(id, _)| *id)
}

pub fn role_name(r: Role) -> &'static str {
    match r {
        Role::Coordinator => "coordinator",
        Role::Dev => "dev",
    }
}

fn parse_role(s: &str) -> Result<Role, Refusal> {
    Role::parse_named(s).map_err(|m| Refusal::new("owner-command-bad-role", m))
}

fn bad(m: impl Into<String>) -> Refusal {
    Refusal::new("owner-command-malformed", m)
}

fn name(s: Option<&str>) -> Result<AgentName, Refusal> {
    let s = s.ok_or_else(|| bad("agent name missing"))?;
    AgentName::parse(s).map_err(|e| bad(e.to_string()))
}

/// Parse an owner comment. `Ok(None)` if it is not a command (does not start with
/// `/crew`). `Err` if it starts with `/crew` but is malformed or unknown.
pub fn parse(comment: &str) -> Result<Option<OwnerCommand>, Refusal> {
    let first = comment.lines().next().unwrap_or("").trim();
    let Some(rest) = first
        .strip_prefix("/crew")
        .filter(|r| r.is_empty() || r.starts_with(' '))
    else {
        return Ok(None);
    };
    let mut toks = rest.split_whitespace();
    let verb = toks.next().ok_or_else(|| bad("`/crew` needs a verb"))?;
    let cmd = match verb {
        "approve" => {
            let n = name(toks.next())?;
            let (mut role, mut key_fp, mut item, mut login) = (None, None, None, None);
            for t in toks.by_ref() {
                let (k, v) = t
                    .split_once('=')
                    .ok_or_else(|| bad(format!("'{t}' is not key=value")))?;
                let slot_taken = match k {
                    "role" => role.replace(parse_role(v)?).is_some(),
                    "key" if v.starts_with("SHA256:") => key_fp.replace(v.to_string()).is_some(),
                    "key" => return Err(bad("key= must be a SHA256: fingerprint")),
                    "item" => item.replace(v.to_string()).is_some(),
                    "login" if valid_login(v) => login.replace(v.to_string()).is_some(),
                    "login" => return Err(bad(format!("login='{v}' is not a GitHub login"))),
                    other => return Err(bad(format!("unknown option '{other}'"))),
                };
                if slot_taken {
                    return Err(bad(format!("duplicate option '{k}'")));
                }
            }
            OwnerCommand::Approve {
                name: n,
                role: role.unwrap_or(Role::Dev),
                key_fp,
                item,
                login,
            }
        }
        "reject" => OwnerCommand::Reject {
            name: name(toks.next())?,
        },
        "role" => {
            let n = name(toks.next())?;
            let r = parse_role(toks.next().ok_or_else(|| bad("role missing"))?)?;
            OwnerCommand::SetRole { name: n, role: r }
        }
        "profile" => {
            let n = name(toks.next())?;
            let (mut tags, mut untags) = (Vec::new(), Vec::new());
            let (mut tier, mut about) = (TierOp::Keep, AboutOp::Keep);
            for t in toks.by_ref() {
                if let Some(tag) = t.strip_prefix('+') {
                    tags.push(crate::capabilities::validate_tag(tag).map_err(|e| bad(e.message))?);
                } else if let Some(tag) = t.strip_prefix('-') {
                    untags
                        .push(crate::capabilities::validate_tag(tag).map_err(|e| bad(e.message))?);
                } else if let Some(v) = t.strip_prefix("tier=") {
                    if tier != TierOp::Keep {
                        return Err(bad("duplicate option 'tier'"));
                    }
                    tier = if v == "none" {
                        TierOp::Clear
                    } else {
                        TierOp::Set(
                            crate::capabilities::validate_tier(v).map_err(|e| bad(e.message))?,
                        )
                    };
                } else if let Some(v) = t.strip_prefix("about=") {
                    if about != AboutOp::Keep {
                        return Err(bad("duplicate option 'about'"));
                    }
                    about = match v {
                        "set" => AboutOp::Set,
                        "clear" => AboutOp::Clear,
                        _ => return Err(bad("about= must be set or clear")),
                    };
                } else {
                    return Err(bad(format!(
                        "'{t}' is not +tag, -tag, tier=<t> or about=set|clear"
                    )));
                }
            }
            OwnerCommand::Profile {
                name: n,
                tags,
                untags,
                tier,
                about,
            }
        }
        "pause" => OwnerCommand::Pause {
            name: name(toks.next())?,
        },
        "resume" => OwnerCommand::Resume {
            name: name(toks.next())?,
        },
        "stop" => OwnerCommand::Stop {
            name: name(toks.next())?,
        },
        "move" => {
            let words: Vec<&str> = toks.by_ref().collect();
            if words.is_empty() {
                return Err(bad("`/crew move` needs a status"));
            }
            OwnerCommand::Move {
                status: move_target(&words.join(" ")).map_err(|r| bad(r.message))?,
            }
        }
        other => return Err(bad(format!("unknown command '/crew {other}'"))),
    };
    if toks.next().is_some() {
        return Err(bad("unexpected extra words"));
    }
    Ok(Some(cmd))
}

/// The comment that `crewboard agent profile set --agent <name>` posts: the command line, and for a
/// new background text the text itself after it. The text is `None` for a change without one.
pub fn profile_text(name: &AgentName, c: &crate::capabilities::Change) -> String {
    let mut line = format!("/crew profile {name}");
    for t in &c.tags {
        line.push_str(&format!(" +{t}"));
    }
    for t in &c.untags {
        line.push_str(&format!(" -{t}"));
    }
    if let Some(t) = &c.tier {
        line.push_str(&format!(" tier={t}"));
    }
    if c.clear_tier {
        line.push_str(" tier=none");
    }
    match c.about.as_deref() {
        Some("") => line.push_str(" about=clear"),
        Some(text) => {
            line.push_str(" about=set\n");
            line.push_str(text.trim());
            line.push('\n');
        }
        None => {}
    }
    line
}

/// The background text of a `/crew profile ... about=set` comment: everything after the
/// first line.
pub fn profile_about(comment: &str) -> String {
    comment
        .split_once('\n')
        .map_or(String::new(), |(_, rest)| rest.trim().to_string())
}

/// The change that an owner command asks for.
pub fn profile_change(cmd: &OwnerCommand, comment: &str) -> Option<crate::capabilities::Change> {
    let OwnerCommand::Profile {
        tags,
        untags,
        tier,
        about,
        ..
    } = cmd
    else {
        return None;
    };
    Some(crate::capabilities::Change {
        tags: tags.clone(),
        untags: untags.clone(),
        tier: match tier {
            TierOp::Set(t) => Some(t.clone()),
            _ => None,
        },
        clear_tier: *tier == TierOp::Clear,
        about: match about {
            AboutOp::Keep => None,
            AboutOp::Clear => Some(String::new()),
            AboutOp::Set => Some(profile_about(comment)),
        },
    })
}

/// The text of the approval comment that `crewboard approve` posts. `login` is the account that
/// wrote the join.
pub fn approve_text(name: &AgentName, role: Role, key_fp: &str, item: &str, login: &str) -> String {
    format!(
        "/crew approve {name} role={} key={key_fp} item={item} login={login}",
        role_name(role)
    )
}

/// The rule of a target status that is not one of the board statuses.
pub const MOVE_BAD_STATUS: &str = "move-bad-status";
/// The rule of the target Done: a ticket ends through the coordinator's signed close.
pub const MOVE_DONE_NEEDS_CLOSE: &str = "move-done-needs-close";
/// The rule of the target Dropped: a ticket is dropped through the coordinator's signed note.
pub const MOVE_DROPPED_NEEDS_CLOSE: &str = "move-dropped-needs-close";

/// The status that an owner move names: a board status by its name, in any case, with `-` or `_`
/// for the space (`in-progress`). Done, Dropped and an unknown name are refused. Pure.
pub fn move_target(s: &str) -> Result<Status, Refusal> {
    let want = s
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let status = Status::ALL
        .into_iter()
        .find(|x| x.name().to_lowercase() == want)
        .ok_or_else(|| {
            Refusal::new(
                MOVE_BAD_STATUS,
                format!(
                    "'{s}' is not a status: use Backlog, Ready, In Progress, Review or Blocked"
                ),
            )
        })?;
    match status {
        Status::Done => Err(Refusal::new(
            MOVE_DONE_NEEDS_CLOSE,
            "the owner does not move a ticket to Done: merge its PR, and the coordinator posts the signed close (`crewboard close <n>`)",
        )),
        Status::Dropped => Err(Refusal::new(
            MOVE_DROPPED_NEEDS_CLOSE,
            "the owner does not move a ticket to Dropped: ask the coordinator to drop it with a signed reason (`crewboard close <n> --why \"<reason>\"`)",
        )),
        _ => Ok(status),
    }
}

/// The comment that `crewboard move` posts: the command line, and the reason after it.
pub fn move_text(status: Status, why: Option<&str>) -> String {
    let mut t = format!("/crew move {}", status.name());
    if let Some(w) = why.map(str::trim).filter(|w| !w.is_empty()) {
        t.push('\n');
        t.push_str(w);
        t.push('\n');
    }
    t
}

/// A GitHub login as an owner command names it: letters, digits and `-`, at most 39 characters,
/// or the login of an app account with the suffix `[bot]`.
fn valid_login(s: &str) -> bool {
    let base = s.strip_suffix("[bot]").unwrap_or(s);
    !base.is_empty()
        && base.len() <= 39
        && !base.starts_with('-')
        && base.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> AgentName {
        AgentName::parse(s).unwrap()
    }

    #[test]
    fn approve_variants_parse() {
        assert_eq!(
            parse("/crew approve hermes@host-b").unwrap(),
            Some(OwnerCommand::Approve {
                name: n("hermes@host-b"),
                role: Role::Dev,
                key_fp: None,
                item: None,
                login: None
            })
        );
        let c = parse(
            "/crew approve claude@host-a role=coordinator key=SHA256:abc item=PVTI_x\nthanks",
        )
        .unwrap();
        assert_eq!(
            c,
            Some(OwnerCommand::Approve {
                name: n("claude@host-a"),
                role: Role::Coordinator,
                key_fp: Some("SHA256:abc".into()),
                item: Some("PVTI_x".into()),
                login: None
            })
        );
        assert_eq!(
            parse(&approve_text(
                &n("claude@host-a"),
                Role::Dev,
                "SHA256:q",
                "PVTI_1",
                "dev-account"
            ))
            .unwrap(),
            Some(OwnerCommand::Approve {
                name: n("claude@host-a"),
                role: Role::Dev,
                key_fp: Some("SHA256:q".into()),
                item: Some("PVTI_1".into()),
                login: Some("dev-account".into())
            })
        );
        // an app account keeps its suffix
        assert!(matches!(
            parse("/crew approve a@b login=crew-bot[bot]").unwrap(),
            Some(OwnerCommand::Approve { login: Some(l), .. }) if l == "crew-bot[bot]"
        ));
    }

    #[test]
    fn other_verbs_parse() {
        assert_eq!(
            parse("/crew reject hermes@host-b").unwrap(),
            Some(OwnerCommand::Reject {
                name: n("hermes@host-b")
            })
        );
        assert_eq!(
            parse("/crew role claude@host-a coordinator").unwrap(),
            Some(OwnerCommand::SetRole {
                name: n("claude@host-a"),
                role: Role::Coordinator
            })
        );
        assert!(matches!(
            parse("/crew pause claude@host-a").unwrap(),
            Some(OwnerCommand::Pause { .. })
        ));
        assert!(matches!(
            parse("/crew resume claude@host-a").unwrap(),
            Some(OwnerCommand::Resume { .. })
        ));
        assert!(matches!(
            parse("/crew stop claude@host-a").unwrap(),
            Some(OwnerCommand::Stop { .. })
        ));
    }

    #[test]
    fn a_move_names_one_board_status_and_keeps_the_reason_on_the_next_lines() {
        for (text, want) in [
            ("/crew move Ready", Status::Ready),
            ("/crew move In Progress", Status::InProgress),
            ("/crew move in-progress", Status::InProgress),
            ("/crew move  backlog ", Status::Backlog),
            ("/crew move Review\nthe fix was wrong", Status::Review),
            ("/crew move Blocked", Status::Blocked),
        ] {
            assert_eq!(
                parse(text).unwrap(),
                Some(OwnerCommand::Move { status: want }),
                "{text}"
            );
        }
        // the writer and the parser agree, with and without a reason
        for why in [None, Some("reopened: the release needs it")] {
            let t = move_text(Status::InProgress, why);
            assert_eq!(
                parse(&t).unwrap(),
                Some(OwnerCommand::Move {
                    status: Status::InProgress
                })
            );
            assert_eq!(t.lines().nth(1), why);
        }
        assert_eq!(move_text(Status::Ready, Some("  ")), "/crew move Ready");
        // Done, Dropped, an unknown status and no status are refused, not guessed
        for t in [
            "/crew move Done",
            "/crew move done",
            "/crew move Dropped",
            "/crew move dropped",
            "/crew move Needs Human",
            "/crew move Readyy",
            "/crew move",
        ] {
            assert_eq!(parse(t).unwrap_err().rule, "owner-command-malformed", "{t}");
        }
        assert_eq!(move_target("Done").unwrap_err().rule, MOVE_DONE_NEEDS_CLOSE);
        assert_eq!(
            move_target("dropped").unwrap_err().rule,
            MOVE_DROPPED_NEEDS_CLOSE
        );
        assert_eq!(move_target("Elsewhere").unwrap_err().rule, MOVE_BAD_STATUS);
        // the reason line is free text: a status word in it changes nothing
        assert_eq!(
            parse("/crew move Ready\nDone is wrong here").unwrap(),
            Some(OwnerCommand::Move {
                status: Status::Ready
            })
        );
    }

    #[test]
    fn plain_text_is_not_a_command() {
        for t in [
            "",
            "hello",
            "please /crew approve x@y",
            "/crewish approve a@b",
            "  hi\n/crew approve a@b",
        ] {
            assert_eq!(parse(t).unwrap(), None, "{t}");
        }
    }

    #[test]
    fn malformed_commands_are_refused() {
        for t in [
            "/crew",
            "/crew approve",
            "/crew approve Hermes",
            "/crew approve a@b role=admin",
            "/crew approve a@b role=dev role=coordinator",
            "/crew approve a@b key=abc",
            "/crew approve a@b login=",
            "/crew approve a@b login=two%words",
            "/crew approve a@b login=x login=y",
            "/crew approve a@b frob=1",
            "/crew approve a@b dev",
            "/crew role a@b",
            "/crew reject a@b extra",
            "/crew launch a@b",
        ] {
            let e = parse(t).expect_err(t);
            assert!(e.rule.starts_with("owner-command"), "{t}: {}", e.rule);
        }
    }

    #[test]
    fn the_latest_lifecycle_command_for_an_agent_wins_and_other_agents_are_ignored() {
        let cmd = |t: &str| parse(t).unwrap().unwrap();
        let cmds = vec![
            (10, cmd("/crew pause a@x")),
            (11, cmd("/crew pause b@x")),
            (12, cmd("/crew resume a@x")),
            (13, cmd("/crew approve c@x")),
            (14, cmd("/crew pause a@x")),
        ];
        assert_eq!(latest_verb_for(&cmds, "a@x"), Some((14, Verb::Pause)));
        assert_eq!(latest_verb_for(&cmds, "b@x"), Some((11, Verb::Pause)));
        // approve is not a lifecycle command; an agent without commands has none
        assert_eq!(latest_verb_for(&cmds, "c@x"), None);
        assert_eq!(latest_verb_for(&cmds, "d@x"), None);
        // the order is by comment id, not by position
        let shuffled = vec![(14, cmd("/crew pause a@x")), (12, cmd("/crew resume a@x"))];
        assert_eq!(latest_verb_for(&shuffled, "a@x"), Some((14, Verb::Pause)));
        assert_eq!(Verb::Stop.word(), "stop");
    }
    #[test]
    fn profile_command_round_trips_with_its_background_text() {
        use crate::capabilities::Change;
        let c = Change {
            tags: vec!["x86-builds".into(), "e2e".into()],
            untags: vec!["gpu".into()],
            tier: Some("high".into()),
            about: Some("Runs the x86 builds.\nSecond line.".into()),
            ..Change::default()
        };
        let text = profile_text(&n("hermes@host-b"), &c);
        assert!(text.starts_with(
            "/crew profile hermes@host-b +x86-builds +e2e -gpu tier=high about=set\n"
        ));
        let cmd = parse(&text).unwrap().unwrap();
        assert_eq!(profile_change(&cmd, &text).unwrap(), c);
        // clearing the tier and the background
        let c2 = Change {
            clear_tier: true,
            about: Some(String::new()),
            ..Change::default()
        };
        let t2 = profile_text(&n("a@b"), &c2);
        assert_eq!(t2, "/crew profile a@b tier=none about=clear");
        assert_eq!(
            profile_change(&parse(&t2).unwrap().unwrap(), &t2).unwrap(),
            c2
        );
        // another command has no profile change
        let p = parse("/crew pause a@b").unwrap().unwrap();
        assert_eq!(profile_change(&p, "/crew pause a@b"), None);
    }

    #[test]
    fn a_malformed_profile_command_is_refused_not_guessed() {
        for bad in [
            "/crew profile",
            "/crew profile a@b +Bad",
            "/crew profile a@b +arch:x86_64",
            "/crew profile a@b -",
            "/crew profile a@b tier=High",
            "/crew profile a@b tier=low tier=high",
            "/crew profile a@b about=maybe",
            "/crew profile a@b about=set about=clear",
            "/crew profile a@b stray",
        ] {
            let r = parse(bad);
            assert!(r.is_err(), "{bad}: {r:?}");
            assert_eq!(r.unwrap_err().rule, "owner-command-malformed", "{bad}");
        }
        // control: the plain form parses to no change
        assert!(matches!(
            parse("/crew profile a@b").unwrap(),
            Some(OwnerCommand::Profile { .. })
        ));
    }
}
