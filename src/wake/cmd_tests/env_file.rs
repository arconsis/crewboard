//! The env file and the environment of the wake child.

use super::*;

/// The child command of a wake with an env file that tries to replace what the wake sets.
#[test]
fn the_env_file_cannot_replace_what_the_wake_sets() {
    let rig = Rig::new();
    let wd = rig.p("wd");
    std::fs::create_dir_all(&wd).unwrap();
    let shim = rig.p("agent/bin");
    std::fs::create_dir_all(&shim).unwrap();
    std::fs::create_dir_all(rig.p("agent/gh-empty-config")).unwrap();
    let gh = shim.join("gh");
    std::fs::write(&gh, "#!/bin/sh\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let file = rig.p("agent.env");
    std::fs::write(
        &file,
        "GH_TOKEN=tok-in-file\nGH_CONFIG_DIR=/evil\nCREWBOARD_LOCK_RUN_DIRS=/etc\nCREWBOARD_WAKE=0\nCREWBOARD_EVENTS_FILE=/evil\nCREWBOARD_WAKE_REASON=evil\nCREWBOARD_RESULTS_DIR=/evil\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let loaded = crate::agent_env::load(&rig.p("cfg"), Some(&file)).unwrap();
    let lockdir = rig.p("locks/heavy.lock.d");
    let s = rig
        .settings(
            &[
                "--runtime",
                "claude",
                "--no-events-hook",
                "--agent",
                "dev@host-a",
                "--initiative",
                "demo",
                "--work-dir",
                wd.to_str().unwrap(),
                "--shim-dir",
                shim.to_str().unwrap(),
                "--host-lock",
                lockdir.to_str().unwrap(),
            ],
            &[("PATH", "/usr/bin:/bin")],
        )
        .unwrap();
    std::fs::create_dir_all(&s.paths.dir).unwrap();
    let log = Log::with_err(s.paths.log.clone(), &rig.clock, Box::new(rig.err.clone()));
    let mut st = Scripted::new(&[]);
    let flow = Flow {
        s: &s,
        steps: &mut st,
        clock: &rig.clock,
        pids: &SystemProcs,
        log: &log,
        host: "testhost",
        sig: Cell::new(None),
        signals: &|| None,
        heartbeat_steps: None,
    };
    let c = flow.wake_child("", Reason::Events, Some(&loaded));
    let envs: std::collections::HashMap<String, Option<String>> = c
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    let get = |k: &str| envs.get(k).cloned().flatten();
    assert_eq!(get("CREWBOARD_WAKE").as_deref(), Some("1"));
    assert_eq!(
        get("CREWBOARD_LOCK_RUN_DIRS").as_deref(),
        Some(lockdir.to_str().unwrap())
    );
    assert_eq!(
        get("GH_CONFIG_DIR").as_deref(),
        Some(rig.p("agent/gh-empty-config").to_str().unwrap())
    );
    assert_eq!(get("CREWBOARD_WAKE_REASON").as_deref(), Some("events"));
    assert_eq!(
        get("CREWBOARD_RESULTS_DIR"),
        Some(s.paths.results.to_string_lossy().into_owned())
    );
    assert_eq!(
        get("CREWBOARD_EVENTS_FILE"),
        Some(
            rig.state()
                .join("events.txt")
                .to_string_lossy()
                .into_owned()
        )
    );
    assert!(
        get("PATH")
            .unwrap()
            .starts_with(&format!("{}:", shim.display())),
        "{:?}",
        get("PATH")
    );
    assert_eq!(c.get_current_dir(), Some(wd.as_path()));
    // a token variable of the caller is removed from the child when an env file is in force
    let plain = rig.p("plain.env");
    std::fs::write(&plain, "GH_TOKEN=tok-plain\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let loaded2 = crate::agent_env::load(&rig.p("cfg"), Some(&plain)).unwrap();
    let c3 = flow.wake_child("", Reason::Events, Some(&loaded2));
    assert!(
        c3.get_envs()
            .any(|(k, v)| k == "GITHUB_TOKEN" && v.is_none()),
        "GITHUB_TOKEN of the caller is removed from the child"
    );
    // twin: a plain `sh -c` wake has none of the agent variables
    let s2 = rig.settings(&[], &[]).unwrap();
    let mut st2 = Scripted::new(&[]);
    let flow2 = Flow {
        s: &s2,
        steps: &mut st2,
        clock: &rig.clock,
        pids: &SystemProcs,
        log: &log,
        host: "testhost",
        sig: Cell::new(None),
        signals: &|| None,
        heartbeat_steps: None,
    };
    let c2 = flow2.wake_child("true", Reason::Work, None);
    assert!(
        c2.get_envs()
            .all(|(k, _)| k != "CREWBOARD_WAKE" && k != "GH_CONFIG_DIR")
    );
    assert_eq!(c2.get_program(), "sh");
}

#[test]
fn relative_paths_of_the_state_are_made_absolute_for_the_child() {
    let rig = Rig::new();
    let wd = rig.p("wd");
    std::fs::create_dir_all(&wd).unwrap();
    let s = rig
        .settings(
            &[
                "--runtime",
                "hermes",
                "--agent",
                "dev@host-a",
                "--initiative",
                "demo",
                "--work-dir",
                wd.to_str().unwrap(),
                "--state-dir",
                "rel/state",
                "--log",
                "rel/wake.log",
                "--events-file",
                "rel/ev.txt",
            ],
            &[],
        )
        .unwrap();
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(s.paths.dir, cwd.join("rel/state"));
    assert_eq!(s.paths.log, cwd.join("rel/wake.log"));
    assert_eq!(s.events_file, Some(cwd.join("rel/ev.txt")));
    let prompt = s.direct.unwrap().argv.pop().unwrap();
    assert!(
        prompt.contains(&format!(
            "Your events are in {}",
            cwd.join("rel/ev.txt").display()
        )),
        "{prompt}"
    );
}

#[test]
fn an_env_file_that_cannot_be_loaded_is_exit_4_each_run_and_leaves_no_stop_file() {
    let rig = Rig::new();
    let mut s = rig.settings(&[], &[]).unwrap();
    s.env_load_failed = true;
    let msg = "4||env file /x/a.env has mode 644; group and others must have no access. Run: chmod 600 /x/a.env";
    let mut st = Scripted::new(&[msg]);
    assert_eq!(rig.run_settings(&mut st, &s), 4);
    assert_eq!(rig.state_text("stopped"), None, "chmod could not lift it");
    let log = rig.log();
    assert!(
        log.contains("has mode 644") && log.contains("fix it"),
        "{log}"
    );
    assert_eq!(st.count("say"), 0, "nobody can be told with this file");
    // the next run, after the fix: it polls again at once
    let mut st = Scripted::new(&["1||"]);
    assert_eq!(rig.run_settings(&mut st, &s), 0);
    assert_eq!(st.count("poll --quiet"), 1);
    // twin: a failure of another kind stops the wakes, as before
    let rig = Rig::new();
    let mut st = Scripted::new(&["4||bad credentials"]).with_mine(&[(1, String::new().as_str())]);
    assert_eq!(rig.run(&mut st, &[]), 4);
    assert!(rig.state_text("stopped").is_some());
}
