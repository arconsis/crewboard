//! Single-select options (spec/board#fields). GitHub replaces the whole option list on each
//! update, and an option written without its id gets a new id, which empties every item
//! value of that field. So options are only written as a complete list that
//! keeps every existing id, and the list read back is verified.

use super::lifecycle::State;
use super::{Refusal, Verdict};
use crate::base::ids;

/// One option as GitHub returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionEntry {
    pub id: String,
    pub name: String,
}

/// One option in the list to write. `id: None` is a new option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredOption {
    pub id: Option<String>,
    pub name: String,
}

/// A change to the option list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionChange {
    Add {
        name: String,
    },
    Rename {
        id: String,
        name: String,
    },
    /// `prune` only: drop one option (its ticket values become empty, so the caller must
    /// have shown that no ticket uses it, see [`check_prune`]).
    Remove {
        id: String,
    },
}

/// Build the complete new list. Every existing option keeps its id and its position.
/// A new option goes last, without id.
pub fn merge_options(
    current: &[OptionEntry],
    change: &OptionChange,
) -> Result<Vec<DesiredOption>, Refusal> {
    let mut list: Vec<DesiredOption> = current
        .iter()
        .map(|o| DesiredOption {
            id: Some(o.id.clone()),
            name: o.name.clone(),
        })
        .collect();
    match change {
        OptionChange::Remove { id } => {
            let before = list.len();
            list.retain(|o| o.id.as_deref() != Some(id.as_str()));
            if list.len() == before {
                return Err(Refusal::new(
                    ids::OPTION_UNKNOWN_ID,
                    format!("no option with id '{id}'"),
                ));
            }
            if list.is_empty() {
                return Err(Refusal::new(
                    ids::OPTION_LAST,
                    "a single-select field keeps at least one option",
                ));
            }
        }
        OptionChange::Add { name } => {
            if current.iter().any(|o| &o.name == name) {
                return Err(Refusal::new(
                    ids::OPTION_DUPLICATE_NAME,
                    format!("option '{name}' exists already"),
                ));
            }
            list.push(DesiredOption {
                id: None,
                name: name.clone(),
            });
        }
        OptionChange::Rename { id, name } => {
            if current.iter().any(|o| &o.name == name && &o.id != id) {
                return Err(Refusal::new(
                    ids::OPTION_DUPLICATE_NAME,
                    format!("option '{name}' exists already"),
                ));
            }
            match list.iter_mut().find(|o| o.id.as_deref() == Some(id)) {
                Some(o) => o.name = name.clone(),
                None => {
                    return Err(Refusal::new(
                        ids::OPTION_UNKNOWN_ID,
                        format!("no option with id '{id}'"),
                    ));
                }
            }
        }
    }
    Ok(list)
}

/// Like [`verify_ids_kept`], but the options in `removed` are meant to be gone (`prune`).
/// Every other earlier id must still be there, and every removed id must really be gone.
/// `renames` lists the renames the caller planned (`from`, `to`, like [`plan_options`]): an
/// option that carries `from` before the write may carry `to` after it; any other changed
/// name is refused.
pub fn verify_ids_kept_except(
    before: &[OptionEntry],
    after: &[OptionEntry],
    removed: &[String],
    renames: &[(&str, &str)],
) -> Verdict {
    let expected: Vec<OptionEntry> = before
        .iter()
        .filter(|b| !removed.contains(&b.id))
        .cloned()
        .collect();
    verify_ids_kept(&expected, after)?;
    // a kept id keeps its name: a write that renames an option would rename its item values
    let renamed: Vec<String> = expected
        .iter()
        .filter_map(|b| {
            let a = after.iter().find(|a| a.id == b.id)?;
            let planned = renames.contains(&(b.name.as_str(), a.name.as_str()));
            (a.name != b.name && !planned)
                .then(|| format!("{} ('{}' became '{}')", b.id, b.name, a.name))
        })
        .collect();
    if !renamed.is_empty() {
        return Err(Refusal::new(
            ids::OPTION_NAME_CHANGED,
            format!("options renamed by the write: {}", renamed.join(", ")),
        ));
    }
    let still: Vec<&str> = after
        .iter()
        .filter(|a| removed.contains(&a.id))
        .map(|a| a.id.as_str())
        .collect();
    if still.is_empty() {
        Ok(())
    } else {
        Err(Refusal::new(
            ids::OPTION_NOT_REMOVED,
            format!("options still there after the write: {}", still.join(", ")),
        ))
    }
}

/// Compare the list before a write with the list read back after it. Every earlier id
/// must still be there; a lost id is an error that stops the command (spec/board#fields, loud).
/// New ids in `after` are fine.
pub fn verify_ids_kept(before: &[OptionEntry], after: &[OptionEntry]) -> Verdict {
    let lost: Vec<&str> = before
        .iter()
        .filter(|b| !after.iter().any(|a| a.id == b.id))
        .map(|b| b.id.as_str())
        .collect();
    if lost.is_empty() {
        Ok(())
    } else {
        Err(Refusal::new(
            ids::OPTION_ID_LOST,
            format!("option ids lost by the write: {}", lost.join(", ")),
        ))
    }
}

/// Put the options that a concurrent writer added into `desired` (spec/board#fields, a second
/// writer). `desired` was planned from a list that may be older than `fresh`, the list read right
/// before the write. A fresh option that `desired` does not carry (by id) and that is not meant to
/// go (`removing`) is appended with its id, so the write cannot delete it. A new option of
/// `desired` (no id) that has the name of such a fresh option takes its id: both writers wanted it.
pub fn rebase_on_fresh(
    desired: &[DesiredOption],
    fresh: &[OptionEntry],
    removing: &[String],
) -> Vec<DesiredOption> {
    let mut out = desired.to_vec();
    for f in fresh {
        if removing.contains(&f.id) || out.iter().any(|d| d.id.as_deref() == Some(f.id.as_str())) {
            continue;
        }
        match out.iter_mut().find(|d| d.id.is_none() && d.name == f.name) {
            Some(d) => d.id = Some(f.id.clone()),
            None => out.push(DesiredOption {
                id: Some(f.id.clone()),
                name: f.name.clone(),
            }),
        }
    }
    out
}

/// The names of `desired` that the list read back after the write does not have: an option that a
/// concurrent write took away again (`option-lost-concurrently`).
pub fn names_missing_after(desired: &[DesiredOption], after: &[OptionEntry]) -> Vec<String> {
    desired
        .iter()
        .filter(|d| !after.iter().any(|a| a.name == d.name))
        .map(|d| d.name.clone())
        .collect()
}

/// Build the complete list for "make this field have at least these options" (used by
/// `init`). It only goes through [`merge_options`]: every existing option keeps its id,
/// nothing is removed, missing names are appended, and `renames` (`from`, `to`) turn a
/// pristine built-in name into ours (so its id and item values survive). Afterwards the
/// list is put in the order of `wanted`; options that are not in `wanted` stay after them.
/// Reordering keeps ids.
pub fn plan_options(
    current: &[OptionEntry],
    wanted: &[&str],
    renames: &[(&str, &str)],
) -> Result<Vec<DesiredOption>, Refusal> {
    // Ids of options that GitHub has not created yet start with NUL; they leave as `None`.
    const NEW: char = '\u{0}';
    let entries = |list: &[DesiredOption]| -> Vec<OptionEntry> {
        list.iter()
            .enumerate()
            .map(|(i, d)| OptionEntry {
                id: d.id.clone().unwrap_or_else(|| format!("{NEW}new{i}")),
                name: d.name.clone(),
            })
            .collect()
    };
    let mut list: Vec<DesiredOption> = current
        .iter()
        .map(|o| DesiredOption {
            id: Some(o.id.clone()),
            name: o.name.clone(),
        })
        .collect();
    for (from, to) in renames {
        let cur = entries(&list);
        if let Some(f) = cur.iter().find(|o| o.name == *from)
            && !cur.iter().any(|o| o.name == *to)
        {
            list = merge_options(
                &cur,
                &OptionChange::Rename {
                    id: f.id.clone(),
                    name: (*to).to_string(),
                },
            )?;
        }
    }
    for name in wanted {
        let cur = entries(&list);
        if !cur.iter().any(|o| o.name == *name) {
            list = merge_options(
                &cur,
                &OptionChange::Add {
                    name: (*name).to_string(),
                },
            )?;
        }
    }
    // A renamed or kept option carries its real id; placeholders become `None`.
    for d in &mut list {
        if d.id.as_deref().is_some_and(|i| i.starts_with(NEW)) {
            d.id = None;
        }
    }
    let rank = |name: &str| wanted.iter().position(|w| *w == name).unwrap_or(usize::MAX);
    list.sort_by_key(|d| rank(&d.name));
    Ok(list)
}

/// True if writing `desired` would change nothing (same ids, names and order).
pub fn is_noop(current: &[OptionEntry], desired: &[DesiredOption]) -> bool {
    current.len() == desired.len()
        && current
            .iter()
            .zip(desired)
            .all(|(c, d)| d.id.as_deref() == Some(c.id.as_str()) && c.name == d.name)
}

/// `prune` may remove an option only if no ticket uses it and its agent is `left`.
pub fn check_prune(tickets_using: usize, agent_state: State) -> Verdict {
    if tickets_using > 0 {
        return Err(Refusal::new(
            ids::PRUNE_IN_USE,
            format!("{tickets_using} ticket(s) still use this option"),
        ));
    }
    if agent_state != State::Left {
        return Err(Refusal::new(
            ids::PRUNE_AGENT_NOT_LEFT,
            format!("the agent is {agent_state:?}, not Left"),
        ));
    }
    Ok(())
}

/// What `prune` will do with the options of one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunePlan {
    /// Options to remove: the agent is `left` and no ticket uses the option.
    pub remove: Vec<OptionEntry>,
    /// Options of agents that stay, with the rule that says why.
    pub kept: Vec<(String, String)>,
}

/// Decide which options `prune` removes. `agent_states` maps an agent name to its State,
/// `used` maps an option name to the number of tickets (any Status, also `Done`) that carry
/// it. An option whose name is no agent (`unassigned`, a hand-made one) is not touched and
/// not listed. Every removal passed [`check_prune`].
pub fn plan_prune(
    options: &[OptionEntry],
    agent_states: &std::collections::BTreeMap<String, State>,
    used: &std::collections::BTreeMap<String, usize>,
) -> PrunePlan {
    let mut plan = PrunePlan {
        remove: Vec::new(),
        kept: Vec::new(),
    };
    for o in options {
        let Some(state) = agent_states.get(&o.name) else {
            continue;
        };
        match check_prune(used.get(&o.name).copied().unwrap_or(0), *state) {
            Ok(()) => plan.remove.push(o.clone()),
            Err(r) => plan.kept.push((o.name.clone(), r.message)),
        }
    }
    plan
}

/// GitHub's limit: a single-select field holds at most 100 options. A write of more fails
/// with the GraphQL error "Settings options cannot contain more than 100 entries"
/// (spec/board#option-limit).
pub const OPTION_LIMIT: usize = 100;

/// `status` warns from this many `Agent` options on (spec/board#option-limit).
pub const AGENT_OPTIONS_WARN_AT: usize = 90;

/// Refuse a write of more options than GitHub allows, before the write (`option-field-full`).
/// Every single-select write goes through this check. Only the `Agent` field grows in normal
/// use, and it prunes first (see [`plan_cap_prune`]).
pub fn check_option_limit(field: &str, count: usize) -> Verdict {
    if count <= OPTION_LIMIT {
        return Ok(());
    }
    Err(Refusal::new(
        ids::OPTION_FIELD_FULL,
        format!(
            "field {field} would have {count} options; GitHub allows at most {OPTION_LIMIT} per single-select field"
        ),
    ))
}

/// The `status` warning for the `Agent` field, from [`AGENT_OPTIONS_WARN_AT`] options on.
pub fn agent_options_warning(count: usize) -> Option<String> {
    (count >= AGENT_OPTIONS_WARN_AT).then(|| {
        format!(
            "the Agent field has {count} of {OPTION_LIMIT} options (GitHub's limit); approve prunes the options of left or rejected agents that no item uses, and refuses with agent-field-full when none is free"
        )
    })
}

/// What the registry and the agent item say about the agent behind one `Agent` option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFacts {
    /// The `State` of the agent item. `None` when the item is missing or has no State.
    pub state: Option<State>,
    /// The owner rejected the agent (`/crew reject`).
    pub rejected: bool,
    /// The last known sign of life: the Heartbeat, else the time of the join (Unix seconds).
    pub last_seen: Option<i64>,
}

/// May the cap prune remove the option of this agent? Only an agent that has left, or a
/// rejected agent that never got to a running state. An agent that is `approved`, `active`,
/// `paused` or `draining` keeps its option, also when it is rejected: the owner
/// retires it first (State `left`).
pub fn cap_prunable(f: &AgentFacts) -> bool {
    match f.state {
        Some(State::Left) => true,
        Some(State::Requested) | None => f.rejected,
        Some(_) => false,
    }
}

/// Plan the room for `need` new options in a full field (spec/board#option-limit). `agents`
/// maps an option name to the facts of its agent. An option without an entry names no agent
/// (a hand-made option) and stays. `used_ids` and `used_names` are the options that some
/// board item carries in its `Agent` field. The candidates are the options of
/// [`cap_prunable`] agents that no item uses, the oldest `last_seen` first (an unknown time
/// counts as the oldest; ties by name). Returns exactly `need` options to remove, or the
/// refusal `agent-field-full` when there are fewer candidates.
pub fn plan_cap_prune(
    options: &[OptionEntry],
    agents: &std::collections::BTreeMap<String, AgentFacts>,
    used_ids: &std::collections::BTreeSet<String>,
    used_names: &std::collections::BTreeSet<String>,
    need: usize,
) -> Result<Vec<OptionEntry>, Refusal> {
    let (mut in_use, mut running, mut no_agent) = (0usize, 0usize, 0usize);
    let mut cands: Vec<(Option<i64>, &OptionEntry)> = Vec::new();
    for o in options {
        let used = used_ids.contains(&o.id) || used_names.contains(&o.name);
        match agents.get(&o.name) {
            _ if used => in_use += 1,
            None => no_agent += 1,
            Some(f) if !cap_prunable(f) => running += 1,
            Some(f) => cands.push((f.last_seen, o)),
        }
    }
    if cands.len() < need {
        return Err(Refusal::new(
            ids::AGENT_FIELD_FULL,
            format!(
                "the Agent field has {} options and GitHub allows {OPTION_LIMIT}; {need} must go for the new agent, but only {} can: every other option is in use ({in_use} carried by board items, {running} of agents that have not left, {no_agent} that name no agent). The owner can retire agents (`/crew reject <name>` and State left) or clear the Agent field of Done or Dropped items, then run the command again",
                options.len(),
                cands.len()
            ),
        ));
    }
    cands.sort_by(|a, b| {
        a.0.unwrap_or(i64::MIN)
            .cmp(&b.0.unwrap_or(i64::MIN))
            .then_with(|| a.1.name.cmp(&b.1.name))
    });
    Ok(cands
        .into_iter()
        .take(need)
        .map(|(_, o)| o.clone())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(id: &str, name: &str) -> OptionEntry {
        OptionEntry {
            id: id.into(),
            name: name.into(),
        }
    }
    fn cur() -> Vec<OptionEntry> {
        vec![o("a1", "claude@host-a"), o("b2", "hermes@host-b")]
    }

    #[test]
    fn add_keeps_every_id_and_appends_without_id() {
        let l = merge_options(
            &cur(),
            &OptionChange::Add {
                name: "codex@host-c".into(),
            },
        )
        .unwrap();
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].id.as_deref(), Some("a1"));
        assert_eq!(l[1].id.as_deref(), Some("b2"));
        assert_eq!(l[2].id, None);
        assert_eq!(l[2].name, "codex@host-c");
    }

    #[test]
    fn add_of_an_existing_name_is_refused() {
        let e = merge_options(
            &cur(),
            &OptionChange::Add {
                name: "claude@host-a".into(),
            },
        )
        .unwrap_err();
        assert_eq!(e.rule, "option-duplicate-name");
    }

    #[test]
    fn rename_keeps_the_id() {
        let l = merge_options(
            &cur(),
            &OptionChange::Rename {
                id: "b2".into(),
                name: "hermes@laptop".into(),
            },
        )
        .unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(l[1].id.as_deref(), Some("b2"));
        assert_eq!(l[1].name, "hermes@laptop");
        assert_eq!(l[0].name, "claude@host-a");
    }

    #[test]
    fn rename_of_an_unknown_id_or_to_a_taken_name_is_refused() {
        let e = merge_options(
            &cur(),
            &OptionChange::Rename {
                id: "zz".into(),
                name: "x".into(),
            },
        )
        .unwrap_err();
        assert_eq!(e.rule, "option-unknown-id");
        let e = merge_options(
            &cur(),
            &OptionChange::Rename {
                id: "b2".into(),
                name: "claude@host-a".into(),
            },
        )
        .unwrap_err();
        assert_eq!(e.rule, "option-duplicate-name");
    }

    #[test]
    fn verify_passes_when_every_id_is_kept_new_ids_allowed() {
        let after = vec![o("a1", "x"), o("b2", "y"), o("c3", "new")];
        assert!(verify_ids_kept(&cur(), &after).is_ok());
    }

    #[test]
    fn verify_refuses_a_lost_id_even_if_the_name_came_back() {
        // GitHub re-created both options with new ids: same names, ids lost.
        let after = vec![o("n1", "claude@host-a"), o("n2", "hermes@host-b")];
        let e = verify_ids_kept(&cur(), &after).unwrap_err();
        assert_eq!(e.rule, "option-id-lost");
        assert!(e.message.contains("a1") && e.message.contains("b2"));
        let e = verify_ids_kept(&cur(), &[]).unwrap_err();
        assert_eq!(e.rule, "option-id-lost");
    }

    #[test]
    fn prune_allowed_when_unused_and_left() {
        assert!(check_prune(0, State::Left).is_ok());
    }

    #[test]
    fn prune_refused_while_a_ticket_uses_it() {
        assert_eq!(
            check_prune(1, State::Left).unwrap_err().rule,
            "prune-in-use"
        );
    }

    #[test]
    fn prune_refused_unless_the_agent_is_left() {
        for s in [
            State::Requested,
            State::Approved,
            State::Active,
            State::Paused,
            State::Draining,
        ] {
            assert_eq!(
                check_prune(0, s).unwrap_err().rule,
                "prune-agent-not-left",
                "{s:?}"
            );
        }
    }

    #[test]
    fn plan_renames_a_pristine_builtin_keeps_ids_and_orders() {
        let cur = vec![o("t1", "Todo"), o("p2", "In Progress"), o("d3", "Done")];
        let wanted = ["Backlog", "Ready", "In Progress", "Review", "Done"];
        let l = plan_options(&cur, &wanted, &[("Todo", "Backlog")]).unwrap();
        let names: Vec<&str> = l.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, wanted);
        let id_of = |n: &str| l.iter().find(|d| d.name == n).unwrap().id.clone();
        assert_eq!(
            id_of("Backlog").as_deref(),
            Some("t1"),
            "renamed option keeps its id"
        );
        assert_eq!(id_of("In Progress").as_deref(), Some("p2"));
        assert_eq!(id_of("Done").as_deref(), Some("d3"));
        assert_eq!(id_of("Ready"), None);
        assert_eq!(id_of("Review"), None);
    }

    #[test]
    fn plan_never_removes_an_option_that_is_not_wanted() {
        let cur = vec![o("a1", "claude@host-a"), o("x9", "legacy")];
        let l = plan_options(&cur, &["claude@host-a", "codex@host-c"], &[]).unwrap();
        let names: Vec<&str> = l.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["claude@host-a", "codex@host-c", "legacy"]);
        // every current id is still in the list (the property verify_ids_kept checks)
        for c in &cur {
            assert!(
                l.iter().any(|d| d.id.as_deref() == Some(c.id.as_str())),
                "{c:?}"
            );
        }
    }

    #[test]
    fn plan_is_a_noop_when_everything_is_there_and_a_write_is_needed_otherwise() {
        let cur = vec![o("a", "Work"), o("b", "Agent")];
        let l = plan_options(&cur, &["Work", "Agent"], &[]).unwrap();
        assert!(is_noop(&cur, &l));
        let l = plan_options(&cur, &["Work", "Agent", "Other"], &[]).unwrap();
        assert!(!is_noop(&cur, &l));
        // order differs: a write is needed (ids kept)
        let l = plan_options(&cur, &["Agent", "Work"], &[]).unwrap();
        assert!(!is_noop(&cur, &l));
        assert_eq!(l[0].id.as_deref(), Some("b"));
    }

    #[test]
    fn plan_does_not_rename_when_the_target_name_exists() {
        let cur = vec![o("t1", "Todo"), o("b1", "Backlog")];
        let l = plan_options(&cur, &["Backlog"], &[("Todo", "Backlog")]).unwrap();
        assert_eq!(l.len(), 2, "both kept: {l:?}");
    }

    #[test]
    fn remove_drops_exactly_one_option_and_keeps_every_other_id_and_position() {
        let l = merge_options(&cur(), &OptionChange::Remove { id: "a1".into() }).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].id.as_deref(), Some("b2"));
        assert_eq!(l[0].name, "hermes@host-b");
        // control: the other one
        let l = merge_options(&cur(), &OptionChange::Remove { id: "b2".into() }).unwrap();
        assert_eq!(l[0].id.as_deref(), Some("a1"));
    }

    #[test]
    fn remove_of_an_unknown_id_or_of_the_last_option_is_refused() {
        let e = merge_options(&cur(), &OptionChange::Remove { id: "zz".into() }).unwrap_err();
        assert_eq!(e.rule, "option-unknown-id");
        let one = vec![o("a1", "x")];
        let e = merge_options(&one, &OptionChange::Remove { id: "a1".into() }).unwrap_err();
        assert_eq!(e.rule, "option-last");
    }

    #[test]
    fn a_prune_write_must_lose_exactly_the_removed_ids() {
        let three = vec![o("a1", "a"), o("b2", "b"), o("c3", "c")];
        let removed = vec!["b2".to_string()];
        // passes: b is gone, a and c kept
        assert!(
            verify_ids_kept_except(&three, &[o("a1", "a"), o("c3", "c")], &removed, &[]).is_ok()
        );
        // refused: another id was lost too
        let e = verify_ids_kept_except(&three, &[o("a1", "a")], &removed, &[]).unwrap_err();
        assert_eq!(e.rule, "option-id-lost");
        assert!(
            e.message.contains("c3") && !e.message.contains("b2"),
            "{}",
            e.message
        );
        // refused: the option to remove is still there
        let e = verify_ids_kept_except(&three, &three, &removed, &[]).unwrap_err();
        assert_eq!(e.rule, "option-not-removed");
        // the plain check still refuses any loss (a prune is the only caller that may lose)
        assert!(verify_ids_kept(&three, &[o("a1", "a"), o("c3", "c")]).is_err());
    }

    #[test]
    fn a_removal_write_must_not_rename_a_kept_option() {
        let three = vec![o("a1", "a"), o("b2", "b"), o("c3", "c")];
        let removed = vec!["b2".to_string()];
        let e = verify_ids_kept_except(&three, &[o("a1", "a"), o("c3", "renamed")], &removed, &[])
            .unwrap_err();
        assert_eq!(e.rule, "option-name-changed");
        assert!(
            e.message.contains("c3") && e.message.contains("renamed"),
            "{}",
            e.message
        );
        // control: the same ids with their names pass
        assert!(
            verify_ids_kept_except(&three, &[o("a1", "a"), o("c3", "c")], &removed, &[]).is_ok()
        );
    }

    #[test]
    fn a_planned_rename_passes_and_any_other_rename_is_refused() {
        let three = vec![o("a1", "Todo"), o("b2", "b"), o("c3", "c")];
        let after = [o("a1", "Backlog"), o("b2", "b"), o("c3", "c")];
        let plan = [("Todo", "Backlog")];
        assert!(verify_ids_kept_except(&three, &after, &[], &plan).is_ok());
        // the same write without the plan is refused
        let e = verify_ids_kept_except(&three, &after, &[], &[]).unwrap_err();
        assert_eq!(e.rule, "option-name-changed");
        // the plan covers only its own pair: a second, unplanned rename is refused
        let after = [o("a1", "Backlog"), o("b2", "other"), o("c3", "c")];
        let e = verify_ids_kept_except(&three, &after, &[], &plan).unwrap_err();
        assert_eq!(e.rule, "option-name-changed");
        assert!(
            e.message.contains("b2") && !e.message.contains("a1"),
            "{}",
            e.message
        );
        // a planned `from` renamed to another name is not the planned rename
        let after = [o("a1", "Wrong"), o("b2", "b"), o("c3", "c")];
        assert!(verify_ids_kept_except(&three, &after, &[], &plan).is_err());
    }

    #[test]
    fn the_prune_plan_removes_only_unused_options_of_agents_that_left() {
        use std::collections::BTreeMap;
        let options = vec![
            o("u0", "unassigned"),
            o("a1", "gone-unused@x"),
            o("a2", "gone-used@x"),
            o("a3", "alive@x"),
            o("a4", "draining@x"),
            o("a5", "hand-made"),
        ];
        let states: BTreeMap<String, State> = [
            ("gone-unused@x", State::Left),
            ("gone-used@x", State::Left),
            ("alive@x", State::Active),
            ("draining@x", State::Draining),
        ]
        .into_iter()
        .map(|(n, s)| (n.to_string(), s))
        .collect();
        let used: BTreeMap<String, usize> =
            [("gone-used@x".to_string(), 2), ("alive@x".to_string(), 1)]
                .into_iter()
                .collect();
        let p = plan_prune(&options, &states, &used);
        let removed: Vec<&str> = p.remove.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(removed, vec!["gone-unused@x"]);
        let kept: Vec<&str> = p.kept.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(kept, vec!["gone-used@x", "alive@x", "draining@x"]);
        assert!(p.kept[0].1.contains("2 ticket(s)"), "{:?}", p.kept);
        assert!(p.kept[2].1.contains("not Left"), "{:?}", p.kept);
        // the placeholder and the hand-made option are not agents: neither removed nor listed
        assert!(!removed.contains(&"unassigned") && !removed.contains(&"hand-made"));
        // control: when no item uses the used option, it is removed too
        let none_used = BTreeMap::new();
        let p2 = plan_prune(&options, &states, &none_used);
        let removed2: Vec<&str> = p2.remove.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(removed2, vec!["gone-unused@x", "gone-used@x"]);
    }

    fn facts(state: Option<State>, rejected: bool, last_seen: Option<i64>) -> AgentFacts {
        AgentFacts {
            state,
            rejected,
            last_seen,
        }
    }

    #[test]
    fn the_option_limit_refuses_only_above_100() {
        assert!(check_option_limit("Agent", 100).is_ok());
        let e = check_option_limit("Agent", 101).unwrap_err();
        assert_eq!(e.rule, "option-field-full");
        assert!(
            e.message.contains("101") && e.message.contains("100"),
            "{e}"
        );
    }

    #[test]
    fn the_status_warning_starts_at_90() {
        assert_eq!(agent_options_warning(89), None);
        let w = agent_options_warning(90).unwrap();
        assert!(w.contains("90 of 100"), "{w}");
        assert!(agent_options_warning(100).is_some());
    }

    #[test]
    fn only_left_or_rejected_and_never_started_agents_are_cap_prunable() {
        assert!(cap_prunable(&facts(Some(State::Left), false, None)));
        assert!(cap_prunable(&facts(Some(State::Left), true, None)));
        assert!(cap_prunable(&facts(Some(State::Requested), true, None)));
        assert!(cap_prunable(&facts(None, true, None)));
        // not rejected and not left: stays
        assert!(!cap_prunable(&facts(Some(State::Requested), false, None)));
        assert!(!cap_prunable(&facts(None, false, None)));
        for s in [
            State::Approved,
            State::Active,
            State::Paused,
            State::Draining,
        ] {
            assert!(!cap_prunable(&facts(Some(s), false, None)), "{s:?}");
            assert!(!cap_prunable(&facts(Some(s), true, None)), "rejected {s:?}");
        }
    }

    #[test]
    fn the_cap_prune_takes_the_oldest_unused_retired_options_and_exactly_as_many_as_needed() {
        use std::collections::{BTreeMap, BTreeSet};
        let options = vec![
            o("h0", "hand-made"),
            o("n1", "new@x"),
            o("m2", "mid@x"),
            o("u3", "used@x"),
            o("a4", "alive@x"),
            o("o5", "old@x"),
        ];
        let agents: BTreeMap<String, AgentFacts> = [
            ("new@x", facts(Some(State::Left), false, Some(300))),
            ("mid@x", facts(Some(State::Left), true, Some(200))),
            ("used@x", facts(Some(State::Left), false, Some(1))),
            ("alive@x", facts(Some(State::Active), false, Some(0))),
            ("old@x", facts(Some(State::Left), false, Some(100))),
        ]
        .into_iter()
        .map(|(n, f)| (n.to_string(), f))
        .collect();
        let used_ids: BTreeSet<String> = ["u3".to_string()].into();
        let none = BTreeSet::new();
        let names = |v: Vec<OptionEntry>| v.into_iter().map(|e| e.name).collect::<Vec<_>>();
        assert_eq!(
            names(plan_cap_prune(&options, &agents, &used_ids, &none, 1).unwrap()),
            ["old@x"]
        );
        assert_eq!(
            names(plan_cap_prune(&options, &agents, &used_ids, &none, 2).unwrap()),
            ["old@x", "mid@x"]
        );
        // a use by name counts as well (an item that carries the option)
        let used_names: BTreeSet<String> = ["old@x".to_string()].into();
        assert_eq!(
            names(plan_cap_prune(&options, &agents, &used_ids, &used_names, 1).unwrap()),
            ["mid@x"]
        );
        // control: without the use, the used option is the oldest and goes first
        assert_eq!(
            names(plan_cap_prune(&options, &agents, &none, &none, 1).unwrap()),
            ["used@x"]
        );
        // three candidates, four needed: refused, with the counts
        let e = plan_cap_prune(&options, &agents, &used_ids, &none, 4).unwrap_err();
        assert_eq!(e.rule, "agent-field-full");
        assert!(
            e.message.contains("only 3 can")
                && e.message.contains("1 carried by board items")
                && e.message.contains("1 of agents that have not left")
                && e.message.contains("1 that name no agent")
                && e.message.contains("/crew reject"),
            "{e}"
        );
    }

    #[test]
    fn an_option_that_a_concurrent_writer_added_is_kept_by_the_rebase() {
        let o = |id: &str, name: &str| OptionEntry {
            id: id.into(),
            name: name.into(),
        };
        let d = |id: Option<&str>, name: &str| DesiredOption {
            id: id.map(str::to_string),
            name: name.into(),
        };
        // planned from [a, b]; another writer added c meanwhile
        let planned = [d(Some("a1"), "a"), d(Some("b2"), "b"), d(None, "mine")];
        let fresh = [o("a1", "a"), o("b2", "b"), o("c3", "c")];
        let got = rebase_on_fresh(&planned, &fresh, &[]);
        assert_eq!(
            got,
            [
                d(Some("a1"), "a"),
                d(Some("b2"), "b"),
                d(None, "mine"),
                d(Some("c3"), "c")
            ]
        );
        // the rebased list passes the check that every fresh id is carried
        assert!(
            verify_ids_kept(
                &fresh,
                &got.iter()
                    .filter_map(|x| x.id.clone().map(|id| o(&id, &x.name)))
                    .collect::<Vec<_>>()
            )
            .is_ok()
        );
        // control: without the rebase the same check refuses
        assert!(verify_ids_kept(&fresh, &[o("a1", "a"), o("b2", "b")]).is_err());
        // an option that is meant to go stays out
        let got = rebase_on_fresh(&planned[..2], &fresh, &["c3".to_string()]);
        assert_eq!(got.len(), 2);
        // both writers added the same name: the new option takes the id that exists
        let got = rebase_on_fresh(&[d(Some("a1"), "a"), d(None, "c")], &fresh, &[]);
        assert!(got.contains(&d(Some("c3"), "c")));
        assert_eq!(got.iter().filter(|x| x.name == "c").count(), 1);
        // lost after the write
        let after = [o("a1", "a"), o("b2", "b")];
        assert_eq!(names_missing_after(&planned, &after), ["mine"]);
        assert!(names_missing_after(&planned[..2], &after).is_empty());
    }
}
