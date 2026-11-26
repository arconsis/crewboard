//! The branch of a ticket, as text.

/// Is `head_ref` the branch of ticket `issue` of `agent`, whatever the branch prefix
/// (`<prefix><agent>/<issue>-<slug>`, spec/tickets#branches)? A caller that does not read
/// AUTHORITY.md for the prefix uses this.
pub fn is_ticket_branch(head_ref: &str, agent: &str, issue: u64) -> bool {
    let want = format!("{agent}/{issue}-");
    head_ref.find(&want).is_some_and(|i| {
        (i == 0 || head_ref[..i].ends_with('/')) && head_ref.len() > i + want.len()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ticket_branch_matches_any_prefix_but_only_this_agent_and_issue() {
        assert!(is_ticket_branch("crew/me@x/12-fix", "me@x", 12));
        assert!(is_ticket_branch("team/x/me@x/12-fix", "me@x", 12));
        assert!(is_ticket_branch("me@x/12-fix", "me@x", 12));
        assert!(!is_ticket_branch("crew/me@x/123-fix", "me@x", 12));
        assert!(!is_ticket_branch("crew/me@x/12-", "me@x", 12));
        assert!(!is_ticket_branch("crew/other@x/12-fix", "me@x", 12));
        assert!(!is_ticket_branch("crew/xme@x/12-fix", "me@x", 12));
    }
}
