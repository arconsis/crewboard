//! The unread-message check of `say`, `done`, `block` and `ask`
//! (spec/events#unread-messages-on-a-ticket).
//!
//! An agent that posts while a message for it is still unread on the same ticket may
//! contradict that message. The commands therefore refuse (rule `unread-message-on-ticket`)
//! until the agent has read it, or until it passes `--override-unread`.
//!
//! The check is read-only. It never writes the cursor: only `watch` and `poll` move it.
//! The core is a pure function over the comments of the ticket and the cursor.

use crate::base::fields::Field;
use crate::comments::Comment;
use crate::cursor::Cursor;
use crate::error::{CbError, CbResult};
use crate::events::{Classified, Event, EventClass, FilterEnv, classify};

pub const RULE: &str = "unread-message-on-ticket";

/// The rule of the agent-wide event check of `done` (spec/events#event-checkpoints-and-done).
pub const EVENTS_RULE: &str = "unread-events";

/// The event check of `done`: no unread event passes (`None`); unread events refuse unless
/// `ignore`. With `ignore` the result is the override line for the signed body.
pub fn events_verdict(unread: &[Event], ignore: bool) -> CbResult<Option<String>> {
    if unread.is_empty() {
        return Ok(None);
    }
    if ignore {
        let ids: Vec<String> = unread.iter().map(|e| e.comment_id.to_string()).collect();
        return Ok(Some(format!(
            "override: {EVENTS_RULE} (comments {})",
            ids.join(", ")
        )));
    }
    let mut classes: Vec<String> = Vec::new();
    for label in ["OWNER MESSAGE", "MESSAGE"] {
        let n = unread
            .iter()
            .filter(|e| crate::events::event_label(e) == label)
            .count();
        if n > 0 {
            classes.push(format!("{n} {label}"));
        }
    }
    let mut s = format!(
        "{} unread event(s) for you ({}); read them before you deliver:\n",
        unread.len(),
        classes.join(", ")
    );
    s.push_str(&message_lines(unread));
    s.push_str(
        "Handle them. Then run `crewboard watch --timeout 1m` (it marks them as read) and run \
         `done` again. Use --override-unread only after you have read and handled them; never to skip them.",
    );
    Err(CbError::refused(EVENTS_RULE, s))
}

/// At most this many messages are shown with their first lines in a refusal; the others
/// with the sender line only.
pub const SHOWN_BODIES: usize = 5;
/// At most this many non-blank lines of one message, each cut to [`LINE_CHARS`] characters.
pub const BODY_LINES: usize = 3;
pub const LINE_CHARS: usize = 200;

/// One line per message (sender, kind, time, URL) and, for the first [`SHOWN_BODIES`]
/// messages, the first [`BODY_LINES`] non-blank lines of the body. So a refusal shows what
/// the agent must read, and the agent needs no extra `watch` call to see it.
pub fn message_lines(unread: &[Event]) -> String {
    let mut s = String::new();
    for (i, e) in unread.iter().enumerate() {
        s.push_str(&format!(
            "  from {} ({}) at {}: {}\n",
            e.from, e.kind, e.ts, e.url
        ));
        if i >= SHOWN_BODIES {
            continue;
        }
        let lines: Vec<&str> = e.body.lines().filter(|l| !l.trim().is_empty()).collect();
        for l in lines.iter().take(BODY_LINES) {
            let cut: String = l.chars().take(LINE_CHARS).collect();
            let more = if l.chars().count() > LINE_CHARS {
                " [...]"
            } else {
                ""
            };
            s.push_str(&format!("    {cut}{more}\n"));
        }
        if lines.len() > BODY_LINES {
            s.push_str(&format!(
                "    [... {} more line(s) at the URL]\n",
                lines.len() - BODY_LINES
            ));
        }
    }
    if unread.len() > SHOWN_BODIES {
        s.push_str(&format!(
            "  ({} more message(s) without text here; `crewboard watch` prints them)\n",
            unread.len() - SHOWN_BODIES
        ));
    }
    s
}

/// The messages on `repo#issue` that are addressed to `env.me` and newer than `cursor`.
///
/// Uses the same filter as `watch` (`classify`), so "addressed to me" means the same thing
/// in both. Messages of other agents' conversations, the agent's own messages, and
/// unsigned, foreign or forged comments are not returned: the last ones are security events,
/// not messages. Only agent messages and owner comments count. Comments up to `seen_upto`
/// (the sender's own ack, see `own_ack_watermark`) count as read. Oldest first.
pub fn unread_for_sender(
    comments: &[Comment],
    env: &FilterEnv<'_>,
    cursor: &Cursor,
    repo: &str,
    issue: u64,
    seen_upto: u64,
) -> Vec<Event> {
    let mut out: Vec<Event> = comments
        .iter()
        .filter(|c| c.repo == repo && c.issue == issue)
        .filter(|c| c.id > seen_upto)
        .filter(|c| cursor.is_fresh(c.id, c.updated_epoch))
        .filter_map(|c| match classify(c, env) {
            Classified::Deliver(e) => Some(*e),
            _ => None,
        })
        .filter(|e| matches!(e.class, EventClass::Message | EventClass::Owner))
        .collect();
    out.sort_by_key(|e| e.comment_id);
    out
}

/// The refusal text: names every unread message and says what to do.
pub fn refusal_text(unread: &[Event], repo: &str, issue: u64) -> String {
    let mut s = format!(
        "{} unread message(s) for you on {repo}#{issue}; read them before you post:\n",
        unread.len()
    );
    s.push_str(&message_lines(unread));
    s.push_str(
        "Handle them. Then run `crewboard watch --timeout 1s` (it marks them as read) and run \
         this command again. Use --override-unread only after you have read and handled them; never to skip them.",
    );
    s
}

/// Decide: no unread message passes (`None`); an unread message refuses unless `anyway`.
/// With `anyway` the result is the override line for the signed body
/// (spec/events#unread-messages-on-a-ticket): `override: unread-message-on-ticket (comments 123,
/// 456)`.
pub fn verdict(unread: &[Event], anyway: bool, repo: &str, issue: u64) -> CbResult<Option<String>> {
    if unread.is_empty() {
        return Ok(None);
    }
    if anyway {
        let ids: Vec<String> = unread.iter().map(|e| e.comment_id.to_string()).collect();
        return Ok(Some(format!(
            "override: {RULE} (comments {})",
            ids.join(", ")
        )));
    }
    Err(CbError::refused(RULE, refusal_text(unread, repo, issue)))
}

/// The body with the override line added as a new last line. No line: the body unchanged.
pub fn with_override(body: &str, line: Option<&str>) -> String {
    match line {
        Some(l) => format!("{}\n{l}", body.trim_end()),
        None => body.to_string(),
    }
}

/// The line for the command output.
pub fn note(line: Option<&str>) -> String {
    line.map(|l| format!("note: sent with --override-unread; {l}\n"))
        .unwrap_or_default()
}

/// Does the cached ticket index say that `me` holds `repo#issue`? Reads only the local
/// cache. A ticket that is not in the cache is not held.
pub fn held_in_cache(cache: &crate::state::StateCache, repo: &str, issue: u64, me: &str) -> bool {
    cache
        .index
        .tickets
        .get(&format!("{repo}#{issue}"))
        .and_then(|item| cache.index.ticket_snaps.get(item))
        .is_some_and(|s| s.agent.as_deref() == Some(me))
}

/// The comment id up to which `me` has seen the ticket: the latest signed `ack` (the `take`)
/// of `me` on it. `take` prints the messages before its ack, so they count as read. 0 if
/// there is none. Messages after the ack follow the cursor rule.
pub fn own_ack_watermark(msgs: &[crate::ticket::VMsg], me: &str) -> u64 {
    msgs.iter()
        .filter(|m| m.kind == Some(crate::protocol::names::Kind::Ack))
        .filter(|m| matches!(&m.sender, crate::ticket::Sender::Agent { name, .. } if name == me))
        .map(|m| m.comment_id)
        .max()
        .unwrap_or(0)
}

/// A ticket is positively defined: the issue has a board ticket item, shown by the ticket
/// index (`indexed`) or by the coordinator's signed info comment with `item=PVTI_...`.
/// Any other issue (the registry, a plain issue) is never guarded.
pub fn is_ticket(msgs: &[crate::ticket::VMsg], indexed: bool) -> bool {
    indexed || crate::ticket::find_item_id(msgs).is_some()
}

/// The unread messages for the sender on `repo#issue`, or `None` when the check does not
/// apply: the issue is no ticket, or the agent has no readable cursor (never watched).
/// Reads nothing new and writes nothing.
pub fn unread_events(
    ac: &crate::ctx::AgentCtx,
    world: &crate::ctx::World,
    comments: &[Comment],
    repo: &str,
    issue: u64,
    mine: bool,
    indexed: bool,
) -> Option<Vec<Event>> {
    let msgs = crate::ticket::verified_messages(comments, &ac.ctx.cfg, &world.trust);
    if !is_ticket(&msgs, indexed) {
        return None;
    }
    let cursor = world
        .items
        .get(&ac.agent.item_id)
        .and_then(|i| i.get(Field::Cursor))
        .and_then(|t| Cursor::decode(t).ok())?;
    let my_tickets: std::collections::HashSet<(String, u64)> = if mine {
        [(repo.to_string(), issue)].into_iter().collect()
    } else {
        Default::default()
    };
    let copies = crate::events::copy_map(comments, &ac.ctx.cfg, &world.trust);
    let env = FilterEnv {
        cfg: &ac.ctx.cfg,
        me: &ac.name,
        my_role: world.trust.get(ac.name.as_str()).map(|t| t.role),
        trust: &world.trust,
        my_tickets: &my_tickets,
        copies: &copies,
    };
    let seen_upto = own_ack_watermark(&msgs, ac.name.as_str());
    Some(unread_for_sender(
        comments, &env, &cursor, repo, issue, seen_upto,
    ))
}

/// The check for one command. `comments` are the comments of the ticket that the command
/// loaded anyway. `mine` is true when the ticket's `Agent` field is this agent (an owner
/// comment without a target then counts too). Returns the override line, if any.
///
/// A missing or unreadable cursor means that the agent never watched: there is nothing to
/// compare with, so the check passes. It reads no extra data and writes nothing.
#[allow(clippy::too_many_arguments)]
pub fn guard(
    ac: &crate::ctx::AgentCtx,
    world: &crate::ctx::World,
    comments: &[Comment],
    repo: &str,
    issue: u64,
    mine: bool,
    indexed: bool,
    anyway: bool,
) -> CbResult<Option<String>> {
    match unread_events(ac, world, comments, repo, issue, mine, indexed) {
        Some(unread) => verdict(&unread, anyway, repo, issue),
        None => Ok(None),
    }
}

/// What `take` prints: the messages that it marks as read (sender, kind, time, first lines,
/// URL). Empty text for no messages.
pub fn render_read(unread: &[Event]) -> String {
    if unread.is_empty() {
        return String::new();
    }
    let mut s = format!(
        "{} message(s) for you, read now and marked as read:\n",
        unread.len()
    );
    s.push_str(&message_lines(unread));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::format_ts;
    use crate::comments::tests::comment;
    use crate::config::BoardConfig;
    use crate::items::ItemSnapshot;
    use crate::protocol::names::{AgentName, Kind};
    use crate::protocol::signature::generate_key;
    use crate::registry::tests::{cfg, join_comment, owner, pub_line, signed};
    use crate::registry::{Trust, build_trust, resolve};
    use crate::rules::Role;
    use ssh_key::PrivateKey;
    use std::collections::{BTreeMap, HashMap, HashSet};

    /// The test comments are created at this time.
    const T0: i64 = 1_792_072_800;

    struct W {
        ka: PrivateKey,
        kb: PrivateKey,
        trust: Trust,
        cfg: BoardConfig,
    }

    fn item(id: &str, key: &str) -> ItemSnapshot {
        let mut fields = BTreeMap::new();
        fields.insert("Key".into(), key.to_string());
        fields.insert("State".into(), "active".into());
        ItemSnapshot {
            id: id.into(),
            updated_at: "2026-10-15T14:00:00Z".into(),
            item_type: "DRAFT_ISSUE".into(),
            project_number: Some(15),
            issue_state: None,
            fields,
        }
    }

    /// claude@a is the coordinator, hermes@b and codex@c are devs.
    fn world() -> (W, PrivateKey) {
        let (ka, kb, kc) = (
            generate_key().unwrap(),
            generate_key().unwrap(),
            generate_key().unwrap(),
        );
        let cfg = cfg();
        let reg = resolve(
            &cfg,
            &[
                join_comment(1, "claude@a", &ka, "PVTI_a"),
                join_comment(2, "hermes@b", &kb, "PVTI_b"),
                join_comment(3, "codex@c", &kc, "PVTI_c"),
                owner(4, "/crew approve claude@a role=coordinator"),
                owner(5, "/crew approve hermes@b role=dev"),
                owner(6, "/crew approve codex@c role=dev"),
            ],
        );
        let mut items = HashMap::new();
        for (id, k, n) in [
            ("PVTI_a", &ka, "claude@a"),
            ("PVTI_b", &kb, "hermes@b"),
            ("PVTI_c", &kc, "codex@c"),
        ] {
            items.insert(id.to_string(), item(id, &pub_line(k, n)));
        }
        let trust = build_trust(&reg, &items, 15);
        assert_eq!(trust.agents.len(), 3, "{:?}", trust.problems);
        (W { ka, kb, trust, cfg }, kc)
    }

    static NO_COPIES: std::sync::LazyLock<std::collections::HashMap<u64, u64>> =
        std::sync::LazyLock::new(Default::default);

    fn env<'a>(w: &'a W, me: &'a AgentName, mine: &'a HashSet<(String, u64)>) -> FilterEnv<'a> {
        FilterEnv {
            cfg: &w.cfg,
            me,
            my_role: Some(Role::Dev),
            trust: &w.trust,
            my_tickets: mine,
            copies: &NO_COPIES,
        }
    }

    fn none() -> HashSet<(String, u64)> {
        HashSet::new()
    }

    /// A cursor that is `delta` seconds after the test comments.
    fn cur(delta: i64) -> Cursor {
        Cursor::at(T0 + delta)
    }

    fn me() -> AgentName {
        AgentName::parse("hermes@b").unwrap()
    }

    #[test]
    fn a_newer_message_to_the_sender_refuses_and_names_it() {
        let (w, _) = world();
        let answer = signed(
            50,
            "owner-login",
            &w.ka,
            "claude@a",
            "hermes@b",
            Kind::Answer,
            "use crate x",
        );
        let m = me();
        let mine = none();
        // the cursor is older than the message
        let u = unread_for_sender(
            std::slice::from_ref(&answer),
            &env(&w, &m, &mine),
            &cur(-10),
            &answer.repo,
            answer.issue,
            0,
        );
        assert_eq!(u.len(), 1);
        assert_eq!(
            (u[0].from.as_str(), u[0].kind.as_str()),
            ("claude@a", "answer")
        );
        let e = verdict(&u, false, &answer.repo, answer.issue).unwrap_err();
        assert_eq!(e.exit(), crate::exit::Exit::Refused);
        let text = e.to_string();
        assert!(text.contains(RULE), "{text}");
        assert!(text.contains("claude@a"), "{text}");
        assert!(text.contains("answer"), "{text}");
        assert!(text.contains("issuecomment-50"), "{text}");
        assert!(text.contains("2026-10-15T14:00:00Z"), "{text}");
        assert!(
            text.contains("--override-unread") && text.contains("watch"),
            "{text}"
        );
    }

    #[test]
    fn a_message_older_than_the_cursor_passes() {
        let (w, _) = world();
        let answer = signed(
            50,
            "owner-login",
            &w.ka,
            "claude@a",
            "hermes@b",
            Kind::Answer,
            "x",
        );
        let m = me();
        let mine = none();
        // control: with an old cursor the same message is unread
        let old = cur(-10);
        assert_eq!(
            unread_for_sender(
                std::slice::from_ref(&answer),
                &env(&w, &m, &mine),
                &old,
                &answer.repo,
                answer.issue,
                0
            )
            .len(),
            1
        );
        // read: the cursor is past the message (the overlap window is 60 s)
        let read = cur(600);
        let u = unread_for_sender(
            std::slice::from_ref(&answer),
            &env(&w, &m, &mine),
            &read,
            &answer.repo,
            answer.issue,
            0,
        );
        assert!(u.is_empty());
        assert_eq!(verdict(&u, false, "r", 1).unwrap(), None);
        // read inside the overlap window: the cursor lists it as seen
        let seen = Cursor::at(T0 + 5).advance(&[(answer.id, answer.updated_epoch)]);
        let u = unread_for_sender(
            std::slice::from_ref(&answer),
            &env(&w, &m, &mine),
            &seen,
            &answer.repo,
            answer.issue,
            0,
        );
        assert!(u.is_empty());
    }

    #[test]
    fn messages_that_are_not_to_the_sender_do_not_count() {
        let (w, kc) = world();
        let m = me();
        let mine = none();
        let cases = [
            // to another agent
            signed(
                51,
                "owner-login",
                &w.ka,
                "claude@a",
                "codex@c",
                Kind::Answer,
                "x",
            ),
            // the sender's own message
            signed(
                52,
                "owner-login",
                &w.kb,
                "hermes@b",
                "coordinator",
                Kind::Report,
                "x",
            ),
            // to the owner
            signed(
                53,
                "owner-login",
                &kc,
                "codex@c",
                "owner",
                Kind::Question,
                "x",
            ),
        ];
        for c in &cases {
            let u = unread_for_sender(
                std::slice::from_ref(c),
                &env(&w, &m, &mine),
                &cur(-10),
                &c.repo,
                c.issue,
                0,
            );
            assert!(u.is_empty(), "{}: {u:?}", c.id);
        }
        // control: the same sender, addressed to me, counts
        let to_me = signed(
            54,
            "owner-login",
            &kc,
            "codex@c",
            "hermes@b",
            Kind::Info,
            "x",
        );
        assert_eq!(
            unread_for_sender(
                std::slice::from_ref(&to_me),
                &env(&w, &m, &mine),
                &cur(-10),
                &to_me.repo,
                to_me.issue,
                0
            )
            .len(),
            1
        );
    }

    #[test]
    fn unsigned_and_foreign_comments_do_not_count() {
        let (w, _) = world();
        let m = me();
        let mine = none();
        // a plain comment of a stranger, and a forged header by a stranger
        let stranger = comment(60, "someone", "hermes@b: do it");
        let mut forged = signed(
            61,
            "owner-login",
            &w.ka,
            "claude@a",
            "hermes@b",
            Kind::Answer,
            "x",
        );
        forged.author = "someone".into();
        // a signed message with a broken signature
        let mut bad = signed(
            62,
            "owner-login",
            &w.ka,
            "claude@a",
            "hermes@b",
            Kind::Answer,
            "x",
        );
        bad.body = bad.body.replace("x", "y");
        for c in [stranger, forged, bad] {
            let u = unread_for_sender(
                std::slice::from_ref(&c),
                &env(&w, &m, &mine),
                &cur(-10),
                &c.repo,
                c.issue,
                0,
            );
            assert!(u.is_empty(), "{}: {u:?}", c.id);
        }
    }

    #[test]
    fn a_message_on_another_ticket_does_not_count() {
        let (w, _) = world();
        let m = me();
        let mine = none();
        let c = signed(
            55,
            "owner-login",
            &w.ka,
            "claude@a",
            "hermes@b",
            Kind::Answer,
            "x",
        );
        // the message is on issue 2; the sender posts on issue 3
        let u = unread_for_sender(
            std::slice::from_ref(&c),
            &env(&w, &m, &mine),
            &cur(-10),
            &c.repo,
            3,
            0,
        );
        assert!(u.is_empty());
        let u = unread_for_sender(
            std::slice::from_ref(&c),
            &env(&w, &m, &mine),
            &cur(-10),
            &c.repo,
            c.issue,
            0,
        );
        assert_eq!(u.len(), 1);
    }

    #[test]
    fn an_owner_comment_counts_when_it_names_the_sender_or_the_ticket_is_mine() {
        let (w, _) = world();
        let m = me();
        let c = comment(56, "owner-login", "please use the other crate");
        let (repo, issue) = (c.repo.clone(), c.issue);
        assert!(
            unread_for_sender(
                std::slice::from_ref(&c),
                &env(&w, &m, &none()),
                &cur(-10),
                &repo,
                issue,
                0
            )
            .is_empty()
        );
        let mine: HashSet<(String, u64)> = [(repo.clone(), issue)].into_iter().collect();
        let u = unread_for_sender(
            std::slice::from_ref(&c),
            &env(&w, &m, &mine),
            &cur(-10),
            &repo,
            issue,
            0,
        );
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].from, "owner");
    }

    #[test]
    fn anyway_passes_and_notes_the_unread_messages() {
        let (w, _) = world();
        let m = me();
        let mine = none();
        let c = signed(
            57,
            "owner-login",
            &w.ka,
            "claude@a",
            "hermes@b",
            Kind::Answer,
            "x",
        );
        let u = unread_for_sender(
            std::slice::from_ref(&c),
            &env(&w, &m, &mine),
            &cur(-10),
            &c.repo,
            c.issue,
            0,
        );
        assert!(verdict(&u, false, &c.repo, c.issue).is_err());
        let line = verdict(&u, true, &c.repo, c.issue).unwrap();
        assert_eq!(
            line.as_deref(),
            Some("override: unread-message-on-ticket (comments 57)")
        );
        // the line is the last line of the signed body; the output notes it
        assert_eq!(
            with_override("text\n", line.as_deref()),
            "text\noverride: unread-message-on-ticket (comments 57)"
        );
        assert!(note(line.as_deref()).contains("--override-unread"));
        assert_eq!(with_override("text", None), "text");
        assert_eq!(note(None), "");
        // nothing unread: --override-unread adds no note
        assert_eq!(verdict(&[], true, "r", 1).unwrap(), None);
    }

    #[test]
    fn the_check_does_not_move_the_cursor() {
        let (w, _) = world();
        let m = me();
        let mine = none();
        let c = signed(
            58,
            "owner-login",
            &w.ka,
            "claude@a",
            "hermes@b",
            Kind::Answer,
            "x",
        );
        let cursor = cur(-10);
        let before = cursor.encode();
        let u = unread_for_sender(
            std::slice::from_ref(&c),
            &env(&w, &m, &mine),
            &cursor,
            &c.repo,
            c.issue,
            0,
        );
        assert_eq!(u.len(), 1);
        // a refusal
        assert!(verdict(&u, false, &c.repo, c.issue).is_err());
        assert_eq!(cursor.encode(), before);
        // an --override-unread send
        assert!(verdict(&u, true, &c.repo, c.issue).is_ok());
        assert_eq!(cursor.encode(), before);
        // and the next watch still finds the message as fresh (control: only a real read
        // advances the cursor)
        assert!(cursor.is_fresh(c.id, c.updated_epoch));
        let after_read = cursor.advance(&[(c.id, c.updated_epoch)]);
        assert!(!after_read.is_fresh(c.id, c.updated_epoch));
        assert_ne!(after_read.encode(), before);
        let _ = format_ts(T0);
    }

    #[test]
    fn a_ticket_counts_as_held_only_when_the_cache_says_so() {
        use crate::state::{StateCache, TicketSnap};
        let snap = |a: Option<&str>| TicketSnap {
            status: Some("In Progress".into()),
            agent: a.map(str::to_string),
        };
        let mut cache = StateCache::default();
        // not cached: not held
        assert!(!held_in_cache(&cache, "o/r", 7, "hermes@b"));
        cache.index.tickets.insert("o/r#7".into(), "PVTI_7".into());
        // indexed, no snapshot: not held
        assert!(!held_in_cache(&cache, "o/r", 7, "hermes@b"));
        // held by another agent: not held
        cache
            .index
            .ticket_snaps
            .insert("PVTI_7".into(), snap(Some("codex@c")));
        assert!(!held_in_cache(&cache, "o/r", 7, "hermes@b"));
        // held by me: held; another issue is still not
        cache
            .index
            .ticket_snaps
            .insert("PVTI_7".into(), snap(Some("hermes@b")));
        assert!(held_in_cache(&cache, "o/r", 7, "hermes@b"));
        assert!(!held_in_cache(&cache, "o/r", 8, "hermes@b"));
    }

    /// The cache decides whether an unaddressed owner comment refuses a `say`.
    #[test]
    fn an_unaddressed_owner_comment_refuses_say_only_on_a_held_ticket() {
        use crate::state::{StateCache, TicketSnap};
        let (w, _) = world();
        let m = me();
        let c = comment(70, "owner-login", "use the other crate");
        let key = format!("{}#{}", c.repo, c.issue);
        let mut cache = StateCache::default();
        let check = |cache: &StateCache| {
            let held = held_in_cache(cache, &c.repo, c.issue, "hermes@b");
            let mine: HashSet<(String, u64)> = if held {
                [(c.repo.clone(), c.issue)].into_iter().collect()
            } else {
                HashSet::new()
            };
            let u = unread_for_sender(
                std::slice::from_ref(&c),
                &env(&w, &m, &mine),
                &cur(-10),
                &c.repo,
                c.issue,
                0,
            );
            verdict(&u, false, &c.repo, c.issue)
        };
        // not cached: does not count
        assert_eq!(check(&cache).unwrap(), None);
        // cached, held by another agent: does not count
        cache.index.tickets.insert(key, "PVTI_9".into());
        let snap = |a: &str| TicketSnap {
            status: None,
            agent: Some(a.into()),
        };
        cache
            .index
            .ticket_snaps
            .insert("PVTI_9".into(), snap("codex@c"));
        assert_eq!(check(&cache).unwrap(), None);
        // cached and held by the sender: counts and refuses
        cache
            .index
            .ticket_snaps
            .insert("PVTI_9".into(), snap("hermes@b"));
        assert!(check(&cache).is_err());
    }

    fn verified(w: &W, comments: &[Comment]) -> Vec<crate::ticket::VMsg> {
        crate::ticket::verified_messages(comments, &w.cfg, &w.trust)
    }

    #[test]
    fn a_message_before_the_own_ack_is_read_and_one_after_it_is_not() {
        let (w, _) = world();
        let m = me();
        let mine = none();
        let before = signed(
            50,
            "owner-login",
            &w.ka,
            "claude@a",
            "hermes@b",
            Kind::Request,
            "do x",
        );
        let ack = signed(
            55,
            "owner-login",
            &w.kb,
            "hermes@b",
            "coordinator",
            Kind::Ack,
            "Taken.",
        );
        let after = signed(
            56,
            "owner-login",
            &w.ka,
            "claude@a",
            "hermes@b",
            Kind::Answer,
            "y",
        );
        let all = vec![before.clone(), ack.clone(), after.clone()];
        let upto = own_ack_watermark(&verified(&w, &all), "hermes@b");
        assert_eq!(upto, 55);
        let run = |upto: u64| -> Vec<u64> {
            unread_for_sender(
                &all,
                &env(&w, &m, &mine),
                &cur(-10),
                &before.repo,
                before.issue,
                upto,
            )
            .iter()
            .map(|e| e.comment_id)
            .collect()
        };
        // without the ack watermark both are unread (control)
        assert_eq!(run(0), vec![50, 56]);
        // with it only the message after the take is unread
        assert_eq!(run(upto), vec![56]);
        // only the sender's own ack counts, not another agent's
        let other = signed(
            57,
            "owner-login",
            &w.ka,
            "claude@a",
            "coordinator",
            Kind::Ack,
            "x",
        );
        assert_eq!(own_ack_watermark(&verified(&w, &[other]), "hermes@b"), 0);
        // and the cursor is not touched by any of this
        assert!(cur(-10).is_fresh(50, before.updated_epoch));
    }

    #[test]
    fn only_an_issue_with_a_board_ticket_item_is_a_ticket() {
        let (w, _) = world();
        // the registry: join requests and owner commands, no coordinator info with an item
        let reg = vec![
            join_comment(1, "hermes@b", &w.kb, "PVTI_b"),
            signed(
                12,
                "owner-login",
                &w.ka,
                "claude@a",
                "hermes@b",
                Kind::Answer,
                "hi",
            ),
        ];
        assert!(!is_ticket(&verified(&w, &reg), false));
        // the ticket index says it is a ticket
        assert!(is_ticket(&verified(&w, &reg), true));
        // the coordinator's signed info with the item id says it is a ticket
        // ids above the approvals (4 to 6): a message counts from its approval on
        let info = signed(
            13,
            "owner-login",
            &w.ka,
            "claude@a",
            "all",
            Kind::Info,
            "assigned\nitem=PVTI_t1",
        );
        assert!(is_ticket(&verified(&w, &[info]), false));
        // an info with an item id from a dev agent does not make a ticket
        let fake = signed(
            14,
            "owner-login",
            &w.kb,
            "hermes@b",
            "all",
            Kind::Info,
            "item=PVTI_t1",
        );
        assert!(!is_ticket(&verified(&w, &[fake]), false));
    }

    #[test]
    fn take_prints_what_it_marks_as_read() {
        let (w, _) = world();
        let m = me();
        let mine = none();
        let c = signed(
            50,
            "owner-login",
            &w.ka,
            "claude@a",
            "hermes@b",
            Kind::Request,
            "line one\nline two\n\nline three\nline four",
        );
        let u = unread_for_sender(
            std::slice::from_ref(&c),
            &env(&w, &m, &mine),
            &cur(-10),
            &c.repo,
            c.issue,
            0,
        );
        let text = render_read(&u);
        assert!(text.contains("claude@a (request)"), "{text}");
        assert!(
            text.contains("issuecomment-50") && text.contains("2026-10-15T14:00:00Z"),
            "{text}"
        );
        assert!(
            text.contains("line one") && text.contains("line three"),
            "{text}"
        );
        assert!(!text.contains("line four"), "{text}");
        assert_eq!(render_read(&[]), "");
    }

    fn ev(id: u64, body: &str) -> Event {
        Event {
            class: EventClass::Message,
            comment_id: id,
            repo: "a/b".into(),
            issue: 7,
            url: format!("https://github.com/a/b/issues/7#issuecomment-{id}"),
            author: "crew".into(),
            from: "claude@a".into(),
            to: "hermes@b".into(),
            kind: "answer".into(),
            re: String::new(),
            ts: "2026-10-15T14:05:00Z".into(),
            verified: true,
            body: body.into(),
        }
    }

    #[test]
    fn a_refusal_shows_the_first_lines_of_each_message_and_cuts_the_rest() {
        let long = "x".repeat(LINE_CHARS + 50);
        let u = vec![ev(1, &format!("first\n\nsecond\n{long}\nfourth\nfifth"))];
        let text = refusal_text(&u, "a/b", 7);
        assert!(text.contains("    first\n    second\n"), "{text}");
        assert!(
            text.contains(&format!("    {} [...]\n", "x".repeat(LINE_CHARS))),
            "{text}"
        );
        assert!(!text.contains("fourth"), "{text}");
        assert!(text.contains("[... 2 more line(s) at the URL]"), "{text}");
        assert!(
            text.contains("--override-unread") && text.contains("watch"),
            "{text}"
        );
        // the event check of `done` shows the same text
        let e = events_verdict(&u, false).unwrap_err().to_string();
        assert!(e.contains("    first") && e.contains(EVENTS_RULE), "{e}");
        assert!(
            e.contains(
                "Use --override-unread only after you have read and handled them; never to skip them."
            ),
            "{e}"
        );
        assert!(
            text.contains(
                "Use --override-unread only after you have read and handled them; never to skip them."
            ),
            "{text}"
        );
        // more than SHOWN_BODIES messages: the later ones get only their sender line
        let many: Vec<Event> = (1..=SHOWN_BODIES as u64 + 2)
            .map(|i| ev(i, &format!("body {i}")))
            .collect();
        let m = message_lines(&many);
        assert!(m.contains(&format!("    body {SHOWN_BODIES}\n")), "{m}");
        assert!(!m.contains(&format!("body {}", SHOWN_BODIES + 1)), "{m}");
        assert!(m.contains("issuecomment-7"), "every sender line stays: {m}");
        assert!(m.contains("(2 more message(s) without text here"), "{m}");
        // control: nothing unread, nothing to show
        assert_eq!(message_lines(&[]), "");
    }

    /// Each of the four commands goes through the check, before it posts.
    #[test]
    fn an_interrupt_is_refused_for_an_unread_message_on_the_old_ticket_and_passes_without() {
        let (w, _) = world();
        let note = signed(
            60,
            "owner-login",
            &w.ka,
            "claude@a",
            "hermes@b",
            Kind::Info,
            "do not park #32",
        );
        let m = me();
        let mine = none();
        // direction 1: the coordinator wrote to the agent after the cursor: refused
        let unread = unread_for_sender(
            std::slice::from_ref(&note),
            &env(&w, &m, &mine),
            &cur(-10),
            &note.repo,
            note.issue,
            0,
        );
        let e = verdict(&unread, false, &note.repo, note.issue).unwrap_err();
        assert!(e.to_string().contains(RULE), "{e}");
        // direction 2: the message is older than the cursor: the interrupt goes on
        let none_unread = unread_for_sender(
            std::slice::from_ref(&note),
            &env(&w, &m, &mine),
            &cur(600),
            &note.repo,
            note.issue,
            0,
        );
        assert!(none_unread.is_empty());
        assert_eq!(
            verdict(&none_unread, false, &note.repo, note.issue).unwrap(),
            None
        );
        // and `take --interrupt` runs the guard on the old ticket before it writes anything:
        // the guard is in `plan_park`, which only reads, and `take` plans before it acts
        let tickets = include_str!("ops/tickets/take.rs");
        let body_of = |sig: &str| {
            let rest = &tickets[tickets.find(sig).expect(sig)..];
            let end = rest[1..]
                .find("\nfn ")
                .into_iter()
                .chain(rest[1..].find("\npub fn "))
                .min()
                .map_or(rest.len(), |e| e + 1);
            &rest[..end]
        };
        let park = body_of("fn plan_park(");
        assert!(
            park.contains("let unread_line = unread_guard("),
            "no park guard"
        );
        let plan = body_of("fn plan_take(");
        assert!(plan.contains("plan_park("));
        for write in ["post(", "set_select(", "set_text("] {
            assert!(!park.contains(write), "plan_park writes: {write}");
            assert!(!plan.contains(write), "plan_take writes: {write}");
        }
        let take = body_of("pub fn take(");
        assert!(
            take.find("plan_take(").unwrap() < take.find("act_take(").unwrap(),
            "take acts before it plans"
        );
    }

    #[test]
    fn say_done_block_and_ask_call_the_check_before_they_post() {
        let say = include_str!("ops/say.rs");
        let tickets = include_str!("ops/tickets/mod.rs");
        let done = include_str!("ops/tickets/done.rs");
        let block = include_str!("ops/tickets/block.rs");
        let take = include_str!("ops/tickets/take.rs");
        // the entry of `done` calls the loading step, which holds the unread check
        let load_from = done.find("fn load_done(").expect("done: no load_done");
        let load_end = done[load_from..].find("\n}").expect("load_done ends");
        assert!(
            done[load_from..load_from + load_end].contains("unread_guard("),
            "load_done does not call the unread check"
        );
        let table = [
            ("say", say, "pub fn run(", "send_signed("),
            // `done` is load, plan, act: the check is in `load_done`, the post in `act_done`
            ("done", done, "pub fn done(", "act_done("),
            ("block", block, "pub fn block(", "post("),
            ("ask", block, "pub fn ask(", "post("),
            ("take", take, "pub fn take(", "post("),
        ];
        // the helper of the ticket commands calls the shared check
        assert!(tickets.contains("crate::unread::guard(ac, world"));
        for (name, src, start, post) in table {
            let from = src.find(start).unwrap_or_else(|| panic!("{name}: no fn"));
            let rest = &src[from + start.len()..];
            // the body ends at the next `pub fn` or at the test module, whichever comes first
            let end = ["\npub fn ", "\n#[cfg(test)]"]
                .iter()
                .filter_map(|e| rest.find(e))
                .min()
                .unwrap_or(rest.len());
            let body = &rest[..end];
            let guard = [
                "unread_guard(",
                "unread::guard(",
                "unread::unread_events(",
                "unread::render_read(",
                // the loading step of `done`, checked above
                "load_done(",
            ]
            .iter()
            .filter_map(|g| body.find(g))
            .min()
            .unwrap_or_else(|| panic!("{name} does not call the unread check"));
            let first_post = body
                .find(post)
                .unwrap_or_else(|| panic!("{name}: no post call"));
            assert!(guard < first_post, "{name}: the check comes after the post");
        }
    }
}
