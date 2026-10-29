//! The text of the scheduler units: the systemd service and timer, the launchd plist, the cron
//! line and the wrapper script that the unit runs. The templates are constants with `@KEY@`
//! placeholders; [`fill`] replaces them in one pass, so a value never gets replaced again.
//! The unit texts are byte for byte the ones that `agent-wake.sh --install` wrote; the golden
//! tests in `tests.rs` keep them so.

/// The systemd service of one agent.
pub const SERVICE: &str = "[Unit]
Description=crewboard wake for @WHO@ in @INITIATIVE@

[Service]
Type=oneshot
ExecStart=/bin/bash @WRAPPER@
TimeoutStartSec=60min
";

/// The systemd timer of one agent.
pub const TIMER: &str = "[Unit]
Description=crewboard wake timer for @WHO@ in @INITIATIVE@

[Timer]
OnBootSec=2min
OnUnitActiveSec=@SECS@s

[Install]
WantedBy=timers.target
";

/// The launchd user agent of one agent.
pub const PLIST: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
<dict>
  <key>Label</key><string>@NAME@</string>
  <key>ProgramArguments</key>
  <array><string>/bin/bash</string><string>@WRAPPER@</string></array>
  <key>StartInterval</key><integer>@SECS@</integer>
  <key>StandardOutPath</key><string>@ERR@</string>
  <key>StandardErrorPath</key><string>@ERR@</string>
</dict>
</plist>
";

/// The crontab line of one agent; the marker at the end finds it again.
pub const CRON: &str = "*/@MINUTES@ * * * * /bin/bash @WRAPPER@ >>@ERR@ 2>&1 @MARKER@\n";

/// The wrapper script that a unit runs. It stores the PATH of the installing shell and the
/// absolute path of crewboard, because a scheduler starts a program with a minimal PATH.
pub const WRAPPER: &str = "#!/bin/bash
# Written by crewboard wake install (@SCHEDULER@) on @TS@. Run it again to change it.
# PATH: the PATH of the installing shell, so that the model finds the build tools.
export PATH=@PATH@
exec @CREWBOARD@ 'wake'@ARGS@ \"$@\"
";

/// Fill a template: every `@KEY@` of `vars` becomes its value. The scan is one pass over the
/// template, so the text of a value is never searched for keys. An unknown `@...@` stays.
pub fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 64);
    let mut rest = template;
    while let Some(start) = rest.find('@') {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 1..];
        let hit = tail.find('@').and_then(|end| {
            vars.iter()
                .find(|(k, _)| *k == &tail[..end])
                .map(|(_, v)| (end, *v))
        });
        match hit {
            Some((end, v)) => {
                out.push_str(v);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('@');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// One word for a shell: `'a'\''b'`.
pub fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `&`, `<` and `>` for the text of a plist.
pub fn xml_esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// One word of a systemd command line: as it is when it is plain, else in double quotes with
/// the escapes that systemd knows (`\\`, `\"`, `%%`, `$$`).
pub fn unit_arg(s: &str) -> String {
    let plain = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._/+:@,=-".contains(&b));
    if plain {
        return s.to_string();
    }
    let e = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
        .replace('$', "$$");
    format!("\"{e}\"")
}

/// The marker of the crontab line of a unit.
pub fn cron_marker(name: &str) -> String {
    format!("# crewboard:{name}")
}

/// What the units are made of.
pub struct UnitVars<'a> {
    pub name: &'a str,
    /// The agent name, or `None` for the coordinator.
    pub agent: Option<&'a str>,
    pub initiative: &'a str,
    pub wrapper: &'a str,
    pub err_log: &'a str,
    pub secs: u64,
}

pub fn service(v: &UnitVars) -> String {
    fill(
        SERVICE,
        &[
            ("WHO", v.agent.unwrap_or("the coordinator")),
            ("INITIATIVE", v.initiative),
            ("WRAPPER", &unit_arg(v.wrapper)),
        ],
    )
}

pub fn timer(v: &UnitVars) -> String {
    fill(
        TIMER,
        &[
            ("WHO", v.agent.unwrap_or("the coordinator")),
            ("INITIATIVE", v.initiative),
            ("SECS", &v.secs.to_string()),
        ],
    )
}

pub fn plist(v: &UnitVars) -> String {
    fill(
        PLIST,
        &[
            ("NAME", &xml_esc(v.name)),
            ("WRAPPER", &xml_esc(v.wrapper)),
            ("SECS", &v.secs.to_string()),
            ("ERR", &xml_esc(v.err_log)),
        ],
    )
}

pub fn cron(v: &UnitVars) -> String {
    fill(
        CRON,
        &[
            ("MINUTES", &v.secs.div_ceil(60).to_string()),
            ("WRAPPER", &sq(v.wrapper)),
            ("ERR", &sq(v.err_log)),
            ("MARKER", &cron_marker(v.name)),
        ],
    )
}

/// The wrapper script. `args` are the words after `crewboard wake`, quoted here.
pub fn wrapper(scheduler: &str, ts: &str, path: &str, crewboard: &str, args: &[String]) -> String {
    let words: String = args.iter().map(|a| format!(" {}", sq(a))).collect();
    fill(
        WRAPPER,
        &[
            ("SCHEDULER", scheduler),
            ("TS", ts),
            ("PATH", &sq(path)),
            ("CREWBOARD", &sq(crewboard)),
            ("ARGS", &words),
        ],
    )
}

/// The Hermes command that makes the cron job of a wrapper.
pub fn hermes_create(name: &str, script: &str, secs: u64) -> String {
    format!(
        "hermes cron create --name {name} --no-agent --script {script} --deliver local --failure-deliver origin \"every {}m\"",
        secs.div_ceil(60)
    )
}
