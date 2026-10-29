//! A free text must not read as an act (spec/messages#acts): `say` and `block --why` refuse
//! a body that a reader would take for a close, a verify result or another act.

use crate::support::cli::{GATE, crew, item_in, no_unhandled, run_ok, run_refused};

#[test]
fn say_and_block_refuse_act_text_and_post_plain_text() {
    let (fake, c, b) = crew();
    let t = fake.ticket_issue(
        "IT free text",
        &format!("Goal: free text test.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    );
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    for (kind, body, cmd) in [
        ("info", "closed: not needed", "close or merge"),
        ("info", "Reassigned. Reason: x", "reassign"),
        ("info", "Merged by the coordinator.", "close or merge"),
        ("report", "verify: pass", "verify"),
        ("info", "lifecycle: left", "stop"),
    ] {
        let o = run_refused(
            &fake,
            &c,
            &["say", &ts, "--to", &b.name, "--kind", kind, "--body", body],
            "text-reads-as-act",
        );
        assert!(o.stderr.contains(cmd), "{kind} {body}: {}", o.all());
    }
    // nothing was posted, the ticket is not closed in the log
    assert_eq!(
        fake.item_field(&item, "Status").as_deref(),
        Some("In Progress")
    );
    run_refused(
        &fake,
        &b,
        &["block", &ts, "--why", "parked for #9: x"],
        "text-reads-as-act",
    );
    // plain text still goes through
    run_ok(
        &fake,
        &c,
        &[
            "say",
            &ts,
            "--to",
            &b.name,
            "--kind",
            "info",
            "--body",
            "Closed the loop with the owner",
            "--override-unread",
        ],
    );
    no_unhandled(&fake);
}
