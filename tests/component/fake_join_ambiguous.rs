//! An owner approval without `key=` binds no key that the owner did not see: with joins of two
//! keys under one name it is void, and `approve` does not pick one (spec/messages#trust).

use crate::support::board::{FakeBoard, INITIATIVE, OWNER, REGISTRY, sign_ts};
use crate::support::cli::{crew, no_unhandled};
use crewboard::clock::now_epoch;
use crewboard::protocol::names::Kind;
use crewboard::protocol::signature::generate_key;
use crewboard::registry::{fingerprint, join_body};

/// A second join of the name with another key, as any holder of the crew token can post it.
fn rival_join(fake: &FakeBoard, name: &str) -> String {
    let key = generate_key().unwrap();
    let mut public = key.public_key().clone();
    public.set_comment(name);
    let line = public.to_openssh().unwrap();
    let now = now_epoch() - 60;
    let body = sign_ts(
        &key,
        REGISTRY,
        name,
        "owner",
        Kind::Request,
        &join_body("PVTI_rival", &line, "fake/it", "fake-host"),
        now,
    );
    fake.comment_at(REGISTRY, OWNER, &body, now, now);
    fingerprint(key.public_key())
}

#[test]
fn approve_and_a_bare_approval_refuse_two_keys_and_a_keyed_approval_binds_the_named_one() {
    let (fake, c, _b) = crew();
    let z = fake.joined("claude@fake-z");
    let real_fp = fingerprint(z.key.public_key());
    let rival_fp = rival_join(&fake, &z.name);
    let owner = fake.owner_dir();
    // the CLI does not choose between the keys
    let o = fake.run(
        owner.path(),
        &["approve", &z.name, "--initiative", INITIATIVE],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("approve-ambiguous-join"), "{}", o.all());
    assert!(
        o.stderr.contains(&real_fp) && o.stderr.contains(&rival_fp),
        "{}",
        o.all()
    );
    // a bare approval typed by hand is void; status says how to approve
    fake.owner_comment_now(REGISTRY, &format!("/crew approve {}", z.name));
    let s = fake.run(&c.dir, &["status"]);
    assert!(
        s.stdout
            .contains("registry note: approval of claude@fake-z")
            && s.stdout.contains("approve with key=<fingerprint>"),
        "{}",
        s.all()
    );
    // the approval that names the key of the real join binds it
    fake.owner_comment_now(
        REGISTRY,
        &format!("/crew approve {} role=dev key={real_fp}", z.name),
    );
    let s = fake.run(&c.dir, &["status", "--json"]);
    let v: serde_json::Value = serde_json::from_str(s.stdout.trim()).unwrap();
    let agent = v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == z.name.as_str())
        .unwrap_or_else(|| panic!("{v}"));
    assert_eq!(agent["approved"], true, "{agent}");
    no_unhandled(&fake);
}
