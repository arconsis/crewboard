//! Re-entrancy with real process chains: `run` in `run`, `take` under `run`. The inner
//! calls are children of the outer `run` (the holder), so the ancestor walk is the real `ps`.
//! The inner scripts write their results to files, because the outer command owns the lock.
#![cfg(unix)]

use crate::support::lock::*;

fn read(r: &Rig, name: &str) -> String {
    std::fs::read_to_string(r.p(name))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// `crewboard lock run <l> --holder outer@x -- sh -c SCRIPT sh <BIN> <dir> <prefix>`
fn outer(r: &Rig, l: &str, script: &str) -> Out {
    let (lock, prefix) = (r.s(l), r.s(l));
    r.run(&[
        "run",
        &lock,
        "--holder",
        "outer@x",
        "--no-board",
        "--",
        "sh",
        "-c",
        script,
        "sh",
        BIN,
        &lock,
        &prefix,
    ])
}

#[test]
fn r1_a_nested_run_in_run_passes_at_once_and_the_outer_lock_stays() {
    let r = Rig::new();
    let o = outer(
        &r,
        "h19",
        r#"
  "$1" lock run "$2" --holder inner@x --no-board --wait 0 -- sh -c "echo inner-ran >\"$3.ran\""; echo $? >"$3.rc"
  grep -q "^agent=outer@x\$" "$2/owner" && echo kept >"$3.kept"
"#,
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(read(&r, "h19.rc"), "0");
    assert!(r.p("h19.ran").exists());
    assert_eq!(read(&r, "h19.kept"), "kept");
    assert!(
        !r.p("h19").exists(),
        "the outer lock is released when the outer ends"
    );
    assert!(o.stderr.contains("re-entrant"), "{}", o.stderr);
}

#[test]
fn r2_a_nested_take_writes_no_second_owner_file_and_its_release_leaves_the_outer_lock() {
    let r = Rig::new();
    outer(
        &r,
        "h20",
        r#"
  "$1" lock take "$2" --holder tool@x --no-board --wait 0; echo $? >"$3.take"
  [ "$(grep -c "" "$2/owner")" -eq 6 ] && grep -q "^agent=outer@x\$" "$2/owner" && echo oneowner >"$3.one"
  "$1" lock release "$2" --holder tool@x; echo $? >"$3.rel"
  [ -f "$2/owner" ] && grep -q "^agent=outer@x\$" "$2/owner" && echo stays >"$3.stays"
  "$1" lock release "$2" --holder outer@x; echo $? >"$3.rel2"
  [ -f "$2/owner" ] && echo stays2 >"$3.stays2"
"#,
    );
    assert_eq!(read(&r, "h20.take"), "0");
    assert_eq!(read(&r, "h20.one"), "oneowner");
    assert_eq!(read(&r, "h20.rel"), "0");
    assert_eq!(read(&r, "h20.stays"), "stays");
    // the release under the outer's own name is a nested release too: the outer lock stays
    assert_eq!(read(&r, "h20.rel2"), "0");
    assert_eq!(read(&r, "h20.stays2"), "stays2");
    assert!(!r.p("h20").exists());
}

#[test]
fn r3_a_holder_that_is_not_an_ancestor_still_blocks_take_75_and_release_3() {
    let r = Rig::new();
    r.plant("h21", "other@x", HOST, r.live(), now());
    let l = r.s("h21");
    let live = r.live().to_string();
    let env = |c: &mut std::process::Command| {
        c.env("CREWBOARD_HOST_LOCK", &l)
            .env("CREWBOARD_HOST_LOCK_PID", &live);
    };
    let mut c = r.cmd(&[
        "take",
        &l,
        "--holder",
        "me@x",
        "--no-board",
        "--pid",
        "0",
        "--expect",
        "10m",
    ]);
    env(&mut c);
    let t = out_of(c.output().unwrap());
    let mut c = r.cmd(&["release", &l, "--holder", "me@x"]);
    env(&mut c);
    let rel = out_of(c.output().unwrap());
    assert_eq!((t.code, rel.code), (75, 3), "{}\n{}", t.all(), rel.all());
    assert_eq!(
        owner_field(&r.p("h21"), "agent").as_deref(),
        Some("other@x")
    );
}

#[test]
fn r4_an_ancestor_holder_without_a_grant_is_77_at_once_take_and_run_and_the_message_names_the_parent()
 {
    let r = Rig::new();
    let l = r.s("h22");
    let script = r#"
  "$1" lock take "$2" --holder a@x --no-board --pid $$ || exit 1
  s=$(date +%s); "$1" lock take "$2" --holder b@x --no-board --wait 30 --poll 1 2>"$3.err"; echo $? >"$3.rc"; echo $(( $(date +%s) - s )) >"$3.secs"
  "$1" lock run "$2" --holder b@x --no-board --wait 30 -- true 2>/dev/null; echo $? >"$3.rcrun"
  "$1" lock release "$2" --holder a@x 2>/dev/null; echo $? >"$3.rcrel"
"#.to_string();
    // the shell that takes with --pid $$ is the ancestor of the second and third call
    let mut c = std::process::Command::new("sh");
    c.args(["-c", &script, "sh", BIN, &l, &l])
        .env("HOST_LOCK_HOST", HOST)
        .env("HOST_LOCK_PS_CMD", format!("cat {}", r.s("ps.txt")))
        .env_remove("CREWBOARD_HOST_LOCK")
        .env_remove("CREWBOARD_HOST_LOCK_PID");
    assert!(c.status().unwrap().success());
    assert_eq!(read(&r, "h22.rc"), "77");
    assert_eq!(read(&r, "h22.rcrun"), "77");
    assert!(read(&r, "h22.secs").parse::<i64>().unwrap() <= 5, "no wait");
    assert!(read(&r, "h22.err").contains("held by your own parent process"));
    // the owner's release under an ancestor pid without a grant frees the lock
    assert_eq!(read(&r, "h22.rcrel"), "0");
    assert!(!r.p("h22").exists());
}

#[test]
fn r5_run_exports_the_resolved_dir_and_the_holder_pid_to_its_child_and_only_there() {
    let r = Rig::new();
    let l = r.s("h23");
    let o = r.run(&[
        "run", &l, "--holder", "o@x", "--no-board", "--", "sh", "-c",
        r#"echo "$CREWBOARD_HOST_LOCK" >"$1.dir"; echo "$CREWBOARD_HOST_LOCK_PID" >"$1.pid"; sed -n "s/^pid=//p" "$2/owner" >"$1.owner""#,
        "sh", &l, &l,
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    let real = std::fs::canonicalize(r.root()).unwrap().join("h23");
    assert_eq!(read(&r, "h23.dir"), real.to_string_lossy());
    assert!(!read(&r, "h23.pid").is_empty());
    assert_eq!(read(&r, "h23.pid"), read(&r, "h23.owner"));
    // `take` sets no variable in its caller (it cannot: a child process), and a plain command has none
    let o = r.run(&[
        "take",
        &l,
        "--holder",
        "o@x",
        "--no-board",
        "--pid",
        "0",
        "--expect",
        "10m",
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    let mut c = std::process::Command::new("sh");
    c.args(["-c", "env"]).env_remove("CREWBOARD_HOST_LOCK");
    assert!(!String::from_utf8_lossy(&c.output().unwrap().stdout).contains("CREWBOARD_HOST_LOCK="));
}

#[test]
fn r6_a_symlink_and_a_dotdot_spelling_of_the_dir_are_the_same_lock() {
    let r = Rig::new();
    let real = r.s("h24");
    let lnk = r.s("h24.lnk");
    std::os::unix::fs::symlink(&real, &lnk).unwrap();
    std::fs::create_dir(r.p("sub")).unwrap();
    let dotdot = format!("{}/../h24", r.s("sub"));
    let o = r.run(&[
        "run",
        &real,
        "--holder",
        "o@x",
        "--no-board",
        "--",
        "sh",
        "-c",
        r#"
  "$1" lock take "$2" --holder t@x --no-board --wait 0; echo $? >"$4.a"
  "$1" lock take "$3" --holder t@x --no-board --wait 0; echo $? >"$4.b"
  "$1" lock release "$2" --holder t@x; "$1" lock release "$3" --holder t@x
  grep -q "^agent=o@x\$" "$5/owner" && echo stays >"$4.c"
"#,
        "sh",
        BIN,
        &lnk,
        &dotdot,
        &real,
        &real,
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(read(&r, "h24.a"), "0");
    assert_eq!(read(&r, "h24.b"), "0");
    assert_eq!(read(&r, "h24.c"), "stays");
}

#[test]
fn r7_the_grant_is_for_one_dir_the_inner_child_gets_its_own_dir_another_dir_held_by_an_ancestor_is_77()
 {
    let r = Rig::new();
    let (d, d2) = (r.s("h25"), r.s("h25b"));
    let o = r.run(&[
        "run", &d, "--holder", "o@x", "--no-board", "--", "sh", "-c",
        r#"
  "$1" lock run "$3" --holder x@x --no-board -- sh -c "\"\$1\" lock take \"\$2\" --holder y@x --no-board --wait 0 2>/dev/null; echo \$? >\"\$3\"" sh "$1" "$3" "$4.rc"
"#,
        "sh", BIN, &d, &d2, &d,
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(read(&r, "h25.rc"), "0");
    // a lock of a DIFFERENT dir that an ancestor holds is not covered by the outer grant
    let o = r.run(&[
        "run", &d, "--holder", "o@x", "--no-board", "--", "sh", "-c",
        r#"
  mkdir -p "$3"; printf "agent=z@x\nhost=hostA\npid=$CREWBOARD_HOST_LOCK_PID\nstarted=p\nstarted_epoch=$(date +%s)\nreason=p\n" >"$3/owner"
  "$1" lock take "$3" --holder y@x --no-board --wait 0 2>/dev/null; echo $? >"$4.rc2"
"#,
        "sh", BIN, &d, &d2, &d,
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(read(&r, "h25.rc2"), "77");
}
