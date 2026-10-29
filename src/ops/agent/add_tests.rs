//! Unit tests of the pure helpers of `agent add`. The whole command is covered by
//! `tests/component/agent_add.rs`.

use super::*;

#[test]
fn a_host_name_is_cut_at_the_first_dot_and_made_safe() {
    assert_eq!(short_host("Build-01.example.org\n"), "build-01");
    assert_eq!(short_host("my_host"), "my-host");
}

#[test]
fn an_empty_host_name_becomes_host() {
    assert_eq!(short_host(""), "host");
    assert_eq!(short_host("  .example.org"), "host");
}

#[test]
fn the_default_agent_name_is_runtime_at_short_host() {
    assert_eq!(
        default_name(AgentRuntime::Claude, "Box.local").unwrap(),
        "claude@box"
    );
}

#[test]
fn an_origin_matches_with_ssh_or_https_urls_and_ignores_case_slash_and_dot_git() {
    assert!(origin_matches("git@github.com:Org/Repo.git", "org/repo"));
    assert!(origin_matches("https://github.com/org/repo/\n", "Org/Repo"));
    assert!(!origin_matches("https://github.com/org/repo2", "org/repo"));
    assert!(!origin_matches("https://github.com/other/repo", "org/repo"));
}
