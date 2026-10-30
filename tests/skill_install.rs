//! `crewboard skill install` and `crewboard skill path`, run as a process with a temporary HOME.
//! The cases cover the copy, the links, the stale files and the manifest.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::disallowed_methods,
    reason = "a test fails by panic and waits for real processes"
)]
#![cfg(unix)]

use crewboard::skill_embed::FILES;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Run {
    code: i32,
    out: String,
    err: String,
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_crewboard")
}

/// Run `crewboard skill <args>` with `home` as HOME, in `cwd`.
fn skill_in(home: &Path, cwd: &Path, args: &[&str]) -> Run {
    skill_path(home, cwd, None, args)
}

/// The same, with `path` as PATH (default: HOME, which has no binary).
fn skill_path(home: &Path, cwd: &Path, path: Option<&Path>, args: &[&str]) -> Run {
    let o: Output = Command::new(bin())
        .arg("skill")
        .args(args)
        .env("HOME", home)
        .env_remove("CREWBOARD_CONFIG_DIR")
        // no runtime binary is on PATH, unless a test puts one there
        .env("PATH", path.unwrap_or(home))
        .current_dir(cwd)
        .output()
        .unwrap();
    Run {
        code: o.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&o.stdout).into_owned(),
        err: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

fn skill(home: &Path, args: &[&str]) -> Run {
    skill_in(home, home, args)
}

fn home() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let h = t.path().canonicalize().unwrap();
    (t, h)
}

fn canon(h: &Path) -> PathBuf {
    h.join(".config/agent-skills/crewboard")
}

/// Every path under `dir`, sorted, with the kind (file, dir, link).
fn tree(dir: &Path) -> Vec<String> {
    fn rec(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let m = fs::symlink_metadata(&p).unwrap();
            let kind = if m.file_type().is_symlink() {
                "l"
            } else if m.is_dir() {
                "d"
            } else {
                "f"
            };
            out.push(format!(
                "{kind} {}",
                p.strip_prefix(root).unwrap().display()
            ));
            if m.is_dir() {
                rec(root, &p, out);
            }
        }
    }
    let mut v = Vec::new();
    rec(dir, dir, &mut v);
    v.sort();
    v
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// A clone: `skill/` of this repo and a `Cargo.toml` of the package `crewboard`.
fn clone_of_repo(dir: &Path) -> PathBuf {
    let root = dir.join("clone");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("skill");
    copy_dir(&src, &root.join("skill"));
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"crewboard\"\nversion = \"0.0.0\"\n",
    )
    .unwrap();
    root.canonicalize().unwrap()
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        let t = to.join(e.file_name());
        if p.is_dir() {
            copy_dir(&p, &t);
        } else {
            fs::copy(&p, &t).unwrap();
        }
    }
}

#[test]
fn dry_run_changes_nothing() {
    let (_t, h) = home();
    let before = tree(&h);
    let r = skill(&h, &["install", "--dry-run", "--runtime", "codex"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert_eq!(tree(&h), before, "a dry run wrote something");
    assert!(r.out.contains("dry run only: nothing changed"), "{}", r.out);
    // red direction: the same call without --dry-run changes HOME
    let r = skill(&h, &["install", "--runtime", "codex"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert_ne!(tree(&h), before);
}

#[test]
fn dry_run_prints_plan() {
    let (_t, h) = home();
    let r = skill(&h, &["install", "--dry-run", "--runtime", "codex"]);
    assert!(r.out.contains("mode: dry"), "{}", r.out);
    assert!(
        r.out
            .starts_with("mode: dry   source: embedded in crewboard "),
        "{}",
        r.out
    );
    assert!(r.out.contains("skill version: "), "{}", r.out);
    assert!(r.out.contains("plan: mkdir -p "), "{}", r.out);
    assert!(r.out.contains("plan: ln -s "), "{}", r.out);
    assert!(
        r.out.contains("skill would be linked for: codex"),
        "{}",
        r.out
    );
    assert!(!r.out.contains("do:   "), "{}", r.out);
    // `--apply` is the same as no flag; both together are a usage error
    let r = skill(&h, &["install", "--apply", "--dry-run"]);
    assert_eq!(r.code, 4, "{}", r.err);
}

#[test]
fn runtime_creates_dir_and_link() {
    let (_t, h) = home();
    fs::create_dir_all(h.join(".claude/skills")).unwrap();
    let r = skill(&h, &["install", "--runtime", "codex"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert!(h.join(".codex/skills").is_dir());
    assert_eq!(
        fs::read_link(h.join(".codex/skills/crewboard")).unwrap(),
        canon(&h)
    );
    assert!(
        r.out.contains("skill linked for: claude codex"),
        "{}",
        r.out
    );
    assert!(
        r.out.contains("not found, not linked: copilot hermes"),
        "{}",
        r.out
    );
    assert!(
        r.out
            .contains(&format!("done. Shared copy: {}", canon(&h).display())),
        "{}",
        r.out
    );
    // red direction: a runtime without --runtime and without a directory gets none
    assert!(!h.join(".copilot").exists() && !h.join(".hermes").exists());
    // the hermes path is the one under skills/shared
    let r = skill(
        &h,
        &["install", "--runtime", "hermes", "--runtime", "copilot"],
    );
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert!(h.join(".hermes/skills/shared/crewboard").is_symlink());
    assert!(h.join(".copilot/skills/crewboard").is_symlink());
}

#[test]
fn existing_dir_is_linked_without_flag() {
    let (_t, h) = home();
    fs::create_dir_all(h.join(".claude/skills")).unwrap();
    let r = skill(&h, &["install"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert_eq!(
        fs::read_link(h.join(".claude/skills/crewboard")).unwrap(),
        canon(&h)
    );
    assert!(canon(&h).join("SKILL.md").is_file());
    // the copy has no tests, no installer and no script
    assert!(canon(&h).join("references/loops.md").is_file());
    assert!(!canon(&h).join("loops/tests").exists());
    assert!(!canon(&h).join("install.sh").exists());
    assert!(!canon(&h).join("loops/watch-loop.sh").exists());
    // a second run is the same (idempotent) and says so
    let before = tree(&h);
    let r = skill(&h, &["install"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert_eq!(tree(&h), before);
    assert!(r.out.contains("ok:   "), "{}", r.out);
    assert!(r.out.contains("installed skill version: "), "{}", r.out);
}

#[test]
fn version_from_metadata_not_top_level() {
    let (_t, h) = home();
    let (_c, base) = home();
    let clone = clone_of_repo(&base);
    let md = clone.join("skill/SKILL.md");
    let run = |text: &str| {
        fs::write(&md, text).unwrap();
        let r = skill(
            &h,
            &[
                "install",
                "--dry-run",
                "--link",
                "--from",
                clone.to_str().unwrap(),
            ],
        );
        r.out
            .lines()
            .find(|l| l.starts_with("skill version: "))
            .unwrap_or("")
            .to_string()
    };
    assert_eq!(
        run("---\nname: crewboard\nmetadata:\n  version: \"9.9.9\"\n---\n"),
        "skill version: 9.9.9"
    );
    assert_eq!(
        run("---\nname: crewboard\nversion: 9.9.9\n---\n"),
        "skill version: unknown"
    );
    assert_eq!(
        run("---\nname: crewboard\n---\n\nversion: 9.9.9 in the body\n"),
        "skill version: unknown"
    );
    // the embedded copy prints the version of the binary
    let r = skill(&h, &["install", "--dry-run"]);
    let want = format!("skill version: {}", env!("CARGO_PKG_VERSION"));
    assert!(r.out.lines().any(|l| l == want), "{}", r.out);
}

#[test]
fn references_installed_next_to_skill_md() {
    let (_t, h) = home();
    let refs: Vec<_> = fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("skill/references"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert!(
        !refs.is_empty(),
        "the source has reference files, so the check can fire"
    );
    let r = skill(&h, &["install"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    for p in refs {
        let name = p.file_name().unwrap();
        let got = fs::read(canon(&h).join("references").join(name)).unwrap();
        assert_eq!(got, fs::read(&p).unwrap(), "{name:?}");
    }
    assert!(canon(&h).join("SKILL.md").is_file());
    // --link shows references/ next to SKILL.md through the clone
    let (_c, base) = home();
    let clone = clone_of_repo(&base);
    let (_t2, h2) = home();
    let r = skill(
        &h2,
        &["install", "--link", "--from", clone.to_str().unwrap()],
    );
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert!(
        canon(&h2).join("references/dev.md").is_file() && canon(&h2).join("SKILL.md").is_file()
    );
    assert!(
        canon(&h2).join("loops/tests").is_dir(),
        "--link shows the clone itself"
    );
}

#[test]
fn unknown_runtime_exit_2() {
    let (_t, h) = home();
    let r = skill(&h, &["install", "--runtime", "nosuch"]);
    assert_eq!(r.code, 2, "{}{}", r.out, r.err);
    assert!(r.err.contains("unknown runtime: nosuch"), "{}", r.err);
    assert!(tree(&h).is_empty(), "nothing is created: {:?}", tree(&h));
    // and a known name is no error
    assert_eq!(
        skill(&h, &["install", "--dry-run", "--runtime", "claude"]).code,
        0
    );
}

#[test]
fn real_dir_at_link_path_not_overwritten() {
    let (_t, h) = home();
    let real = h.join(".codex/skills/crewboard");
    fs::create_dir_all(&real).unwrap();
    fs::write(real.join("mine.txt"), "x").unwrap();
    let r = skill(&h, &["install", "--runtime", "codex"]);
    assert_eq!(r.code, 1, "{}{}", r.out, r.err);
    assert!(
        r.err.contains("SKIP: ") && r.err.contains("is a real file or directory"),
        "{}",
        r.err
    );
    assert!(r.err.contains("finished with problems"), "{}", r.err);
    assert!(!real.is_symlink() && real.join("mine.txt").is_file());
    // a link that points elsewhere is not replaced either
    let (_t2, h2) = home();
    fs::create_dir_all(h2.join(".codex/skills")).unwrap();
    std::os::unix::fs::symlink("/nonexistent/elsewhere", h2.join(".codex/skills/crewboard"))
        .unwrap();
    let r = skill(&h2, &["install", "--runtime", "codex"]);
    assert_eq!(r.code, 1, "{}{}", r.out, r.err);
    assert!(
        r.err.contains("SKIP: ") && r.err.contains("/nonexistent/elsewhere"),
        "{}",
        r.err
    );
    assert_eq!(
        fs::read_link(h2.join(".codex/skills/crewboard")).unwrap(),
        Path::new("/nonexistent/elsewhere")
    );
    // --link over a real shared copy is refused as well
    let (_c, base) = home();
    let clone = clone_of_repo(&base);
    let (_t3, h3) = home();
    assert_eq!(skill(&h3, &["install"]).code, 0);
    let r = skill(
        &h3,
        &["install", "--link", "--from", clone.to_str().unwrap()],
    );
    assert_eq!(r.code, 1, "{}{}", r.out, r.err);
    assert!(r.err.contains("exists as a real directory"), "{}", r.err);
    assert!(!canon(&h3).is_symlink());
}

fn hash_tree(dir: &Path) -> Vec<(String, Vec<u8>, u32)> {
    let mut v = Vec::new();
    fn rec(root: &Path, dir: &Path, v: &mut Vec<(String, Vec<u8>, u32)>) {
        for e in fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                rec(root, &p, v);
            } else {
                v.push((
                    p.strip_prefix(root).unwrap().display().to_string(),
                    fs::read(&p).unwrap(),
                    mode(&p),
                ));
            }
        }
    }
    rec(dir, dir, &mut v);
    v.sort();
    v
}

#[test]
fn install_changes_no_source_file() {
    let (_c, base) = home();
    let clone = clone_of_repo(&base);
    let before = hash_tree(&clone);
    let (_t, h) = home();
    // the copy comes from the binary: the clone is not read
    let r = skill_in(&h, &clone.join("skill"), &["install"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert_eq!(hash_tree(&clone), before);
    // --link: the shared path is the clone; no change
    let (_t2, h2) = home();
    let r = skill_in(&h2, &clone.join("skill/loops"), &["install", "--link"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert_eq!(
        fs::read_link(canon(&h2)).unwrap(),
        clone.join("skill"),
        "found by walking up"
    );
    assert_eq!(hash_tree(&clone), before);
}

#[test]
fn link_needs_a_clone() {
    let (_t, h) = home();
    let r = skill(&h, &["install", "--link"]);
    assert_eq!(r.code, 4, "{}{}", r.out, r.err);
    assert!(
        r.err
            .contains("--link needs a clone of crewboard (pass --from <dir>)"),
        "{}",
        r.err
    );
    assert!(tree(&h).is_empty());
    // a directory with skill/SKILL.md but a Cargo.toml of another package is no clone
    let (_c, base) = home();
    let fake = clone_of_repo(&base);
    fs::write(fake.join("Cargo.toml"), "[package]\nname = \"other\"\n").unwrap();
    let r = skill_in(&h, &fake, &["install", "--link"]);
    assert_eq!(r.code, 4, "{}{}", r.out, r.err);
    // --from without the skill directory is refused
    let r = skill(&h, &["install", "--link", "--from", base.to_str().unwrap()]);
    assert_eq!(r.code, 2, "{}{}", r.out, r.err);
}

#[test]
fn installed_copy_has_the_modes_of_the_table() {
    let (_t, h) = home();
    assert_eq!(skill(&h, &["install"]).code, 0);
    for (rel, _, exec) in FILES {
        let want = if *exec { 0o755 } else { 0o644 };
        assert_eq!(mode(&canon(&h).join(rel)), want, "{rel}");
    }
    assert_eq!(mode(&canon(&h).join("SKILL.md")), 0o644);
    // no temporary file is left behind
    assert!(tree(&canon(&h)).iter().all(|l| !l.ends_with(".new")));
}

#[test]
fn reinstall_removes_files_the_binary_no_longer_has() {
    let (_t, h) = home();
    assert_eq!(skill(&h, &["install"]).code, 0);
    let c = canon(&h);
    fs::create_dir_all(c.join("loops")).unwrap(); // the skill has no file in loops/ now
    // an earlier install wrote two files that this binary does not have
    fs::write(c.join("loops/old-script.sh"), "#!/bin/sh\n").unwrap();
    fs::create_dir_all(c.join("gone/deep")).unwrap();
    fs::write(c.join("gone/deep/x.md"), "x").unwrap();
    // a file of the user, not in the manifest
    fs::write(c.join("loops/mine.sh"), "#!/bin/sh\n").unwrap();
    let outside = h.join("outside.txt");
    fs::write(&outside, "keep").unwrap();
    let mf = c.join(".crewboard-skill.json");
    let mut m: serde_json::Value = serde_json::from_slice(&fs::read(&mf).unwrap()).unwrap();
    // the sha256 is that of the file: the earlier install wrote exactly these bytes
    for (p, body) in [
        ("loops/old-script.sh", "#!/bin/sh\n"),
        ("gone/deep/x.md", "x"),
        ("../../../outside.txt", "keep"),
        ("/etc/hosts-not-ours", "keep"),
    ] {
        m["files"][p] = serde_json::json!(sha(body));
    }
    fs::write(&mf, serde_json::to_vec(&m).unwrap()).unwrap();
    let r = skill(&h, &["install"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert!(!c.join("loops/old-script.sh").exists(), "stale file stays");
    assert!(
        !c.join("gone").exists(),
        "the empty directories of the stale file stay"
    );
    assert!(
        c.join("loops/mine.sh").exists(),
        "a file of the user was removed"
    );
    assert!(outside.exists(), "a path outside the copy was removed");
    assert!(c.join("SKILL.md").is_file() && c.join("references/loops.md").is_file());
    assert!(r.out.contains("do:   remove "), "{}", r.out);
    // the manifest lists exactly the files of this binary, with their sha256
    let m: serde_json::Value = serde_json::from_slice(&fs::read(&mf).unwrap()).unwrap();
    assert_eq!(m["files"].as_object().unwrap().len(), FILES.len());
    assert_eq!(m["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        m["files"]["SKILL.md"],
        crewboard::skill_embed::sha256_hex(crewboard::skill_embed::embedded("SKILL.md").unwrap())
    );
    // a dry run lists the removal and removes nothing
    fs::write(c.join("loops/old-script.sh"), "#!/bin/sh\n").unwrap();
    m_with(&mf, "loops/old-script.sh", &sha("#!/bin/sh\n"));
    let r = skill(&h, &["install", "--dry-run"]);
    assert!(r.out.contains("plan: remove "), "{}", r.out);
    assert!(c.join("loops/old-script.sh").exists());
    // a manifest that is not JSON removes nothing and breaks nothing
    fs::write(&mf, "{ not json").unwrap();
    assert_eq!(skill(&h, &["install"]).code, 0);
    assert!(c.join("loops/old-script.sh").exists());
}

fn sha(text: &str) -> String {
    crewboard::skill_embed::sha256_hex(text.as_bytes())
}

fn m_with(mf: &Path, extra: &str, sha: &str) {
    let mut m: serde_json::Value = serde_json::from_slice(&fs::read(mf).unwrap()).unwrap();
    m["files"][extra] = serde_json::json!(sha);
    fs::write(mf, serde_json::to_vec(&m).unwrap()).unwrap();
}

#[test]
fn an_old_install_loses_the_removed_shim_scripts() {
    let (_t, h) = home();
    assert_eq!(skill(&h, &["install"]).code, 0);
    let c = canon(&h);
    let mf = c.join(".crewboard-skill.json");
    // an install of an earlier version: its manifest lists the shims, and the files are there
    let shims = [
        "loops/with-env.sh",
        "loops/watch-loop.sh",
        "loops/gh-shim.sh",
        "loops/crewboard-as.sh",
        "loops/lib.sh",
        "install.sh",
    ];
    fs::create_dir_all(c.join("loops")).unwrap();
    for p in shims {
        fs::write(c.join(p), format!("old {p}\n")).unwrap();
        m_with(&mf, p, &sha(&format!("old {p}\n")));
    }
    // the user changed one of them: it is kept and named
    fs::write(c.join("loops/lib.sh"), "my own change\n").unwrap();
    let r = skill(&h, &["install"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    for p in shims.iter().filter(|p| **p != "loops/lib.sh") {
        assert!(!c.join(p).exists(), "{p} stays");
    }
    assert!(
        c.join("loops/lib.sh").exists(),
        "a changed file was removed"
    );
    assert!(r.out.contains("kept: "), "{}", r.out);
    assert!(c.join("references/loops.md").is_file());
}

#[test]
fn a_stale_file_that_the_user_changed_is_kept() {
    let (_t, h) = home();
    assert_eq!(skill(&h, &["install"]).code, 0);
    let c = canon(&h);
    fs::create_dir_all(c.join("loops")).unwrap(); // the skill has no file in loops/ now
    let mf = c.join(".crewboard-skill.json");
    fs::write(c.join("loops/changed.sh"), "edited by the user\n").unwrap();
    fs::write(c.join("loops/same.sh"), "as installed\n").unwrap();
    m_with(&mf, "loops/changed.sh", &sha("as installed\n"));
    m_with(&mf, "loops/same.sh", &sha("as installed\n"));
    let r = skill(&h, &["install"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert!(
        c.join("loops/changed.sh").exists(),
        "a changed file was removed"
    );
    assert!(
        r.out.contains(&format!(
            "kept: {} (changed)",
            c.join("loops/changed.sh").display()
        )),
        "{}",
        r.out
    );
    assert!(
        !c.join("loops/same.sh").exists(),
        "an unchanged stale file stays"
    );
}

#[test]
fn a_corrupt_manifest_cannot_delete_the_manifest_or_an_empty_path() {
    let (_t, h) = home();
    assert_eq!(skill(&h, &["install"]).code, 0);
    let c = canon(&h);
    let mf = c.join(".crewboard-skill.json");
    let body = fs::read(&mf).unwrap();
    m_with(
        &mf,
        ".crewboard-skill.json",
        &crewboard::skill_embed::sha256_hex(&body),
    );
    m_with(&mf, "", &sha(""));
    m_with(&mf, ".", &sha(""));
    let r = skill(&h, &["install"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    let m: serde_json::Value = serde_json::from_slice(&fs::read(&mf).unwrap()).unwrap();
    assert_eq!(m["files"].as_object().unwrap().len(), FILES.len());
    assert!(c.join("SKILL.md").is_file());
}

#[test]
fn nothing_is_written_or_removed_through_a_symlink_in_the_copy() {
    let (_t, h) = home();
    assert_eq!(skill(&h, &["install"]).code, 0);
    let c = canon(&h);
    fs::create_dir_all(c.join("loops")).unwrap(); // the skill has no file in loops/ now
    // a stale file reached through a link to a directory outside the copy
    let ext = h.join("ext");
    fs::create_dir_all(&ext).unwrap();
    fs::write(ext.join("x.md"), "x").unwrap();
    std::os::unix::fs::symlink(&ext, c.join("gone")).unwrap();
    m_with(&c.join(".crewboard-skill.json"), "gone/x.md", &sha("x"));
    let r = skill(&h, &["install"]);
    assert_eq!(r.code, 1, "{}{}", r.out, r.err);
    assert!(
        r.err.contains("SKIP: ") && r.err.contains("is a link"),
        "{}",
        r.err
    );
    assert!(ext.join("x.md").exists(), "removed through the link");
    // a link where the copy wants to write
    fs::remove_file(c.join("gone")).unwrap();
    let ext2 = h.join("ext2");
    fs::create_dir_all(&ext2).unwrap();
    fs::remove_dir_all(c.join("references")).unwrap();
    std::os::unix::fs::symlink(&ext2, c.join("references")).unwrap();
    let r = skill(&h, &["install"]);
    assert_eq!(r.code, 1, "{}{}", r.out, r.err);
    assert!(r.err.contains("SKIP: "), "{}", r.err);
    assert_eq!(
        fs::read_dir(&ext2).unwrap().count(),
        0,
        "written through the link"
    );
    // a link at loops/tests is not followed by the legacy removal
    fs::remove_file(c.join("references")).unwrap();
    fs::create_dir_all(c.join("references")).unwrap();
    let ext3 = h.join("ext3");
    fs::create_dir_all(&ext3).unwrap();
    fs::write(ext3.join("keep.sh"), "").unwrap();
    std::os::unix::fs::symlink(&ext3, c.join("loops/tests")).unwrap();
    let r = skill(&h, &["install"]);
    assert_eq!(r.code, 1, "{}{}", r.out, r.err);
    assert!(ext3.join("keep.sh").exists());
}

#[test]
fn a_copy_of_the_old_script_without_a_manifest_loses_its_extra_files() {
    let (_t, h) = home();
    assert_eq!(skill(&h, &["install"]).code, 0);
    let c = canon(&h);
    fs::create_dir_all(c.join("loops")).unwrap(); // the skill has no file in loops/ now
    fs::remove_file(c.join(".crewboard-skill.json")).unwrap();
    fs::write(c.join("loops/old.sh"), "").unwrap();
    fs::write(c.join("references/gone.md"), "").unwrap();
    fs::create_dir_all(c.join("loops/tests")).unwrap();
    fs::write(c.join("loops/tests/t.sh"), "").unwrap();
    fs::create_dir_all(c.join("notes")).unwrap();
    fs::write(c.join("notes/mine.txt"), "").unwrap();
    let target = h.join("target.txt");
    fs::write(&target, "keep").unwrap();
    std::os::unix::fs::symlink(&target, c.join("loops/link.sh")).unwrap();
    // a dry run plans the removals and writes nothing
    let r = skill(&h, &["install", "--dry-run"]);
    assert!(
        r.out.contains(&format!(
            "plan: remove {}",
            c.join("loops/old.sh").display()
        )),
        "{}",
        r.out
    );
    assert!(c.join("loops/old.sh").exists() && !c.join(".crewboard-skill.json").exists());
    let r = skill(&h, &["install"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert!(
        r.out.contains(&format!(
            "do:   remove {}",
            c.join("loops/old.sh").display()
        )),
        "{}",
        r.out
    );
    assert!(!c.join("loops/old.sh").exists() && !c.join("references/gone.md").exists());
    assert!(!c.join("loops/tests").exists());
    assert!(
        c.join("notes/mine.txt").exists(),
        "outside loops/ and references/"
    );
    assert!(
        c.join("loops/link.sh").is_symlink() && target.exists(),
        "a symlink is never removed"
    );
    assert!(
        c.join(".crewboard-skill.json").is_file(),
        "the manifest is written"
    );
    assert!(c.join("references/loops.md").is_file());
    // red direction: with a manifest, an extra file that is not in it stays
    fs::write(c.join("loops/mine.sh"), "").unwrap();
    assert_eq!(skill(&h, &["install"]).code, 0);
    assert!(c.join("loops/mine.sh").exists());
}

#[test]
fn a_plain_install_over_a_link_to_a_clone_is_refused_and_changes_nothing() {
    let (_c, base) = home();
    let clone = clone_of_repo(&base);
    let (_t, h) = home();
    let from = clone.to_str().unwrap();
    assert_eq!(skill(&h, &["install", "--link", "--from", from]).code, 0);
    let before = hash_tree(&clone);
    for args in [&["install"][..], &["install", "--dry-run"][..]] {
        let r = skill(&h, args);
        assert_eq!(r.code, 1, "{args:?}: {}{}", r.out, r.err);
        assert!(
            r.err.contains("SKIP: ") && r.err.contains("is a link to"),
            "{}",
            r.err
        );
        assert_eq!(hash_tree(&clone), before, "the clone changed");
        assert!(canon(&h).is_symlink());
    }
}

/// True when the tests run as root: a mode change then stops nothing.
fn is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

fn chmod(p: &Path, mode: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn a_failed_step_exits_3_and_not_1() {
    if is_root() {
        return;
    }
    let (_t, h) = home();
    assert_eq!(skill(&h, &["install"]).code, 0);
    let refs = canon(&h).join("references");
    chmod(&refs, 0o555); // no new file can be made in it
    let r = skill(&h, &["install"]);
    chmod(&refs, 0o755);
    assert_eq!(r.code, 3, "{}{}", r.out, r.err);
    assert!(r.err.contains("FAILED: "), "{}", r.err);
    assert!(r.err.contains("finished with problems"), "{}", r.err);
    // red direction: a SKIP alone stays exit 1
    let (_t2, h2) = home();
    fs::create_dir_all(h2.join(".codex/skills/crewboard")).unwrap();
    assert_eq!(skill(&h2, &["install", "--runtime", "codex"]).code, 1);
}

#[test]
fn a_path_whose_removal_failed_stays_in_the_manifest_for_the_next_run() {
    if is_root() {
        return;
    }
    let (_t, h) = home();
    assert_eq!(skill(&h, &["install"]).code, 0);
    let c = canon(&h);
    let mf = c.join(".crewboard-skill.json");
    fs::create_dir_all(c.join("gone")).unwrap();
    fs::write(c.join("gone/x.md"), "x").unwrap();
    m_with(&mf, "gone/x.md", &sha("x"));
    chmod(&c.join("gone"), 0o555); // the file cannot be unlinked
    let r = skill(&h, &["install"]);
    chmod(&c.join("gone"), 0o755);
    assert_eq!(r.code, 3, "{}{}", r.out, r.err);
    assert!(c.join("gone/x.md").exists());
    let m: serde_json::Value = serde_json::from_slice(&fs::read(&mf).unwrap()).unwrap();
    assert_eq!(
        m["files"]["gone/x.md"],
        sha("x"),
        "the failed path left the manifest"
    );
    // the next run removes it and drops it from the manifest
    let r = skill(&h, &["install"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert!(!c.join("gone").exists());
    let m: serde_json::Value = serde_json::from_slice(&fs::read(&mf).unwrap()).unwrap();
    assert!(m["files"].get("gone/x.md").is_none());
}

#[test]
fn a_planted_new_file_is_not_followed() {
    let (_t, h) = home();
    assert_eq!(skill(&h, &["install"]).code, 0);
    let c = canon(&h);
    let victim = h.join("victim.txt");
    fs::write(&victim, "keep").unwrap();
    std::os::unix::fs::symlink(&victim, c.join("SKILL.md.new")).unwrap();
    std::os::unix::fs::symlink(&victim, c.join(".crewboard-skill.json.new")).unwrap();
    let r = skill(&h, &["install"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    assert!(!c.join("SKILL.md").is_symlink() && c.join("SKILL.md").is_file());
    assert!(!c.join("SKILL.md.new").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn a_file_name_that_is_not_utf8_is_skipped_by_the_legacy_scan() {
    use std::os::unix::ffi::OsStrExt;
    let (_t, h) = home();
    assert_eq!(skill(&h, &["install"]).code, 0);
    let c = canon(&h);
    fs::remove_file(c.join(".crewboard-skill.json")).unwrap();
    fs::create_dir_all(c.join("loops")).unwrap(); // the skill has no file in loops/ now
    let odd = c
        .join("loops")
        .join(std::ffi::OsStr::from_bytes(b"bad\xff.sh"));
    fs::write(&odd, "").unwrap();
    let r = skill(&h, &["install"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert!(odd.exists());
    assert!(
        !r.out.contains("remove ") || !r.out.contains("bad"),
        "{}",
        r.out
    );
}

#[test]
fn an_old_copy_with_a_tests_directory_loses_it() {
    let (_t, h) = home();
    assert_eq!(skill(&h, &["install"]).code, 0);
    let t = canon(&h).join("loops/tests");
    fs::create_dir_all(&t).unwrap();
    fs::write(t.join("old.sh"), "").unwrap();
    assert_eq!(skill(&h, &["install"]).code, 0);
    assert!(!t.exists());
}

#[test]
fn path_prints_the_shared_copy() {
    let (_t, h) = home();
    let r = skill(&h, &["path"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert_eq!(r.out, format!("{}\n", canon(&h).display()));
    assert!(tree(&h).is_empty(), "path writes nothing");
}

#[test]
fn a_headless_config_may_print_the_path_but_not_install() {
    // `skill install` rewrites the skill and the loop scripts: a headless model must not do it
    let (_t, h) = home();
    let cfg = h.join("cfg");
    fs::create_dir_all(&cfg).unwrap();
    fs::write(
        cfg.join("t.toml"),
        "name = \"a\"\nitem_id = \"x\"\nboard = \"b\"\nheadless = true\n",
    )
    .unwrap();
    let run = |args: &[&str]| {
        Command::new(bin())
            .args(["--config-dir", cfg.to_str().unwrap()])
            .args(args)
            .env("HOME", &h)
            .output()
            .unwrap()
    };
    let o = run(&["skill", "install"]);
    assert_eq!(
        o.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(String::from_utf8_lossy(&o.stderr).contains("headless-scope"));
    assert!(
        tree(&h).iter().all(|l| !l.contains("agent-skills")),
        "it wrote"
    );
    let o = run(&["skill", "path"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    // red direction: without the headless key the same install works
    fs::remove_file(cfg.join("t.toml")).unwrap();
    let o = run(&["skill", "install", "--dry-run"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// A `bin` directory with an executable `name` in it.
fn fake_binary(dir: &Path, name: &str) -> PathBuf {
    let bin = dir.join("fakebin");
    fs::create_dir_all(&bin).unwrap();
    let f = bin.join(name);
    fs::write(&f, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&f, fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

#[test]
fn detects_runtimes_by_config_dir_and_binary() {
    let (_t, h) = home();
    // present: ~/.codex exists without a skills directory; `claude` is on PATH
    fs::create_dir_all(h.join(".codex")).unwrap();
    let bin = fake_binary(&h, "claude");
    let r = skill_path(&h, &h, Some(&bin), &["install"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert_eq!(
        fs::read_link(h.join(".codex/skills/crewboard")).unwrap(),
        canon(&h)
    );
    assert_eq!(
        fs::read_link(h.join(".claude/skills/crewboard")).unwrap(),
        canon(&h)
    );
    // absent: no directory, no binary
    assert!(!h.join(".copilot").exists() && !h.join(".hermes").exists());
    assert!(
        r.out.contains("skill linked for: claude codex"),
        "{}",
        r.out
    );
    assert!(
        r.out.contains("not found, not linked: copilot hermes"),
        "{}",
        r.out
    );
    assert!(
        r.out
            .contains("skip: copilot: not found (no ~/.copilot, no `copilot` on PATH"),
        "{}",
        r.out
    );
    // red direction: the same HOME and PATH without the binary and the config directory links nothing new
    let (_t2, h2) = home();
    let r = skill(&h2, &["install"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert!(!h2.join(".claude").exists() && !h2.join(".codex").exists());
}

#[test]
fn no_runtime_installs_the_shared_copy_only() {
    let (_t, h) = home();
    let r = skill(&h, &["install"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert!(canon(&h).join("SKILL.md").is_file());
    assert!(r.out.contains("skill linked for: none"), "{}", r.out);
    assert!(
        r.out
            .contains("no agent runtime found on this host: only the shared copy is installed"),
        "{}",
        r.out
    );
    for d in [".claude", ".codex", ".copilot", ".hermes"] {
        assert!(!h.join(d).exists(), "{d} was created");
    }
    // a dry run says the same and writes nothing
    let (_t2, h2) = home();
    let r = skill(&h2, &["install", "--dry-run"]);
    assert!(
        r.out.contains("no agent runtime found on this host"),
        "{}",
        r.out
    );
    assert!(tree(&h2).is_empty());
}

#[test]
fn explicit_runtime_links_even_when_absent() {
    let (_t, h) = home();
    let r = skill(&h, &["install", "--runtime", "hermes"]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert!(h.join(".hermes/skills/shared/crewboard").is_symlink());
    assert!(r.out.contains("skill linked for: hermes"), "{}", r.out);
}
