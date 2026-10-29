//! The AUTHORITY.md text of the fake tests. One builder, so a new key is added in one place.

/// The `needs_owner` list of the full authority.
pub const NEEDS_OWNER_ALL: &str = "release, ci-config, credentials, delete, new-scope, new-repo";

/// An AUTHORITY.md where the coordinator may create, split and merge. `extra` holds more
/// front matter lines (each ends in a newline), for example `ci: none\n`.
pub fn authority_with(owner: &str, needs_owner: &str, extra: &str) -> String {
    format!(
        "---\ninitiative: fake\nowner: {owner}\ncoordinator_may_create_tickets: true\ncoordinator_may_split_tickets: true\ncoordinator_may_merge: true\ndev_may_open_prs: true\ndev_branch_prefix: crew/\nprotected: [main]\nneeds_owner: [{needs_owner}]\n{extra}---\n\nTest authority.\n"
    )
}

/// The authority with the full `needs_owner` list and the owner `fake-owner`.
pub const AUTHORITY: &str = "---\ninitiative: fake\nowner: fake-owner\ncoordinator_may_create_tickets: true\ncoordinator_may_split_tickets: true\ncoordinator_may_merge: true\ndev_may_open_prs: true\ndev_branch_prefix: crew/\nprotected: [main]\nneeds_owner: [release, ci-config, credentials, delete, new-scope, new-repo]\n---\n\nTest authority: the coordinator may merge.\n";

/// The same, with `needs_owner: [release]` only.
pub const AUTHORITY_RELEASE_ONLY: &str = "---\ninitiative: fake\nowner: fake-owner\ncoordinator_may_create_tickets: true\ncoordinator_may_split_tickets: true\ncoordinator_may_merge: true\ndev_may_open_prs: true\ndev_branch_prefix: crew/\nprotected: [main]\nneeds_owner: [release]\n---\n\nTest authority: the coordinator may merge.\n";
