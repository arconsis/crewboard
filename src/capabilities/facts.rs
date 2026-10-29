//! HostFacts about the host of an agent (spec/profiles#facts): architecture, OS and version, CPU cores,
//! RAM, and the tools that are installed with their versions.
//!
//! HostFacts are detected, never typed: `join` and every `start` run [`detect`] again and write
//! the result. The detection reads the host through a [`Probe`], so the parsing is tested
//! with fake command output; [`RealProbe`] is the one implementation that runs commands.

pub use crate::base::{TOOL_NAME_RULE, valid_tool_name};
use crate::error::{CbError, CbResult};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The tools that are checked when the agent config has no `tools = [...]` list: common tools
/// of many stacks, not the build tools of the project. The list is a probe, not a requirement: a
/// tool that is not installed is left out. It is general first (`git`, `docker`, `make`), then
/// the runtimes and package managers of JavaScript, Python, Go, Rust, .NET and Apple. Tools that
/// can open an install dialog on a host without them (`java`, `swift` on macOS) or that start
/// a JVM (`gradle`, `mvn`) are not in it. The board adds its own with `detect_tools` in
/// `.crewboard.toml`; one agent adds with `join --tool`. Names stay as they were: a fact is
/// `tool:<name>`, and a board that has tags for them keeps working.
pub const DEFAULT_TOOLS: &[&str] = &[
    "git",
    "docker",
    "make",
    "node",
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "deno",
    "python3",
    "uv",
    "go",
    "cargo",
    "rustc",
    "dotnet",
    "xcodebuild",
];

/// One command may run this long; a tool that hangs counts as not available.
pub const TOOL_TIMEOUT: Duration = Duration::from_secs(5);

/// The result of one command: only a command that ran and exited 0 counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub stdout: String,
    pub stderr: String,
}

/// What the detection needs from the host. `Sync`: [`detect`] runs the tool commands in
/// parallel.
pub trait Probe: Sync {
    /// `std::env::consts::OS`: `macos`, `linux`, `windows`, ...
    fn os_name(&self) -> String;
    /// `std::env::consts::ARCH`, used when `uname -m` is not there.
    fn arch_raw(&self) -> String;
    fn cpus(&self) -> Option<usize>;
    /// Run `program args...`. `None` if it is not installed, fails, or times out.
    fn run(&self, program: &str, args: &[&str]) -> Option<CmdOutput>;
    fn read_file(&self, path: &str) -> Option<String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolFact {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostFacts {
    /// `x86_64` or `arm64` (normalised), or the raw value of an unknown one.
    pub arch: Option<String>,
    /// OS name without version: `macos`, `linux-ubuntu`, `windows`.
    pub os_name: Option<String>,
    /// Full OS version as the host reports it, for the profile text: `15.1.1`.
    pub os_version: Option<String>,
    pub cpus: Option<usize>,
    pub ram_gib: Option<u64>,
    pub tools: Vec<ToolFact>,
}

/// `aarch64` and `arm64` are one architecture; so are `x86_64`, `amd64` and `x64`.
pub fn normalize_arch(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "aarch64" | "arm64" | "arm64e" => "arm64".to_string(),
        "x86_64" | "amd64" | "x64" | "x86-64" => "x86_64".to_string(),
        other => other.to_string(),
    }
}

/// The first number of a version: `15.1.1` -> `15`, `24.04` -> `24`.
pub fn major(version: &str) -> Option<String> {
    let m: String = version
        .trim()
        .trim_start_matches('v')
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    (!m.is_empty()).then_some(m)
}

/// `/etc/os-release` -> (`linux-ubuntu`, `24.04`). The name is `linux-<ID>`; a file without
/// `ID=` gives plain `linux`. `VERSION_ID` may be missing (rolling releases).
pub fn parse_os_release(text: &str) -> (String, Option<String>) {
    let (mut id, mut ver) = (None, None);
    for l in text.lines() {
        let Some((k, v)) = l.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
        match k.trim() {
            "ID" => id = Some(v),
            "VERSION_ID" => ver = Some(v),
            _ => {}
        }
    }
    let id: Option<String> = id
        .map(|i| {
            i.to_ascii_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect::<String>()
        })
        .filter(|i| !i.is_empty());
    let name = match id {
        Some(i) => format!("linux-{i}"),
        None => "linux".to_string(),
    };
    (name, ver.filter(|v| !v.is_empty()))
}

/// `sysctl -n hw.memsize` (bytes) -> GiB, rounded to the nearest.
pub fn parse_memsize_bytes(text: &str) -> Option<u64> {
    let b: u64 = text.trim().parse().ok()?;
    (b > 0).then(|| (b + (1 << 29)) >> 30)
}

/// `/proc/meminfo` -> GiB, rounded to the nearest. `MemTotal` is in kB.
pub fn parse_meminfo(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
        .filter(|n| *n > 0)?;
    Some((kb * 1024 + (1 << 29)) >> 30)
}

/// Arguments that make a tool print its version. Most take `--version`.
pub fn version_args(tool: &str) -> &'static [&'static str] {
    match tool {
        "xcodebuild" | "java" | "javac" => &["-version"],
        "go" => &["version"],
        _ => &["--version"],
    }
}

/// The first version-looking word of the output: digits and dots, an optional leading `v`,
/// quotes and trailing punctuation removed (`cargo 1.91.0 (abc 2026-01-01)` -> `1.91.0`,
/// `openjdk version "21.0.5" 2024-10-15` -> `21.0.5`, `Docker version 27.3.1, build x` ->
/// `27.3.1`). A date such as `2026-01-01` is not a version.
pub fn parse_version(text: &str) -> Option<String> {
    for line in text.lines() {
        for w in line.split_whitespace() {
            let w = w.trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == ';');
            // `go version go1.23.4 darwin/arm64`: the version word starts with `go`
            let w = w
                .strip_prefix("go")
                .filter(|r| r.starts_with(|c: char| c.is_ascii_digit()))
                .unwrap_or(w)
                .trim_start_matches('v')
                .trim_end_matches('.');
            // digits and dots, then nothing or a letter (`2.56.0.windows.2`); a date such as
            // `2026-01-01` continues with '-' and is not a version
            let end = w
                .find(|c: char| !(c.is_ascii_digit() || c == '.'))
                .unwrap_or(w.len());
            let (num, rest) = w.split_at(end);
            let num = num.trim_end_matches('.');
            let ok = num.chars().next().is_some_and(|c| c.is_ascii_digit())
                && !num.contains("..")
                && rest.chars().next().is_none_or(|c| c.is_ascii_alphabetic());
            if ok {
                return Some(num.to_string());
            }
        }
    }
    None
}

/// The tools to check: the list of the agent config (else [`DEFAULT_TOOLS`]) plus the
/// `detect_tools` of the board, each name once, in this order. A bad name is a configuration
/// error (exit 4), never skipped silently.
pub fn tool_list(configured: Option<&[String]>, board: &[String]) -> CbResult<Vec<String>> {
    let mut list: Vec<String> = match configured {
        Some(l) => l.to_vec(),
        None => DEFAULT_TOOLS.iter().map(|s| (*s).to_string()).collect(),
    };
    for t in &list {
        if !valid_tool_name(t) {
            return Err(CbError::config(format!(
                "tools: '{t}' is not a tool name ({TOOL_NAME_RULE})"
            )));
        }
    }
    for t in board {
        if !valid_tool_name(t) {
            return Err(CbError::config(format!(
                ".crewboard.toml: detect_tools: '{t}' is not a tool name ({TOOL_NAME_RULE})"
            )));
        }
    }
    list.extend(board.iter().cloned());
    let mut seen = Vec::new();
    for t in list {
        if !seen.contains(&t) {
            seen.push(t);
        }
    }
    Ok(seen)
}

/// Read the host. Every part that cannot be read stays `None` (and is not a tag).
pub fn detect(p: &dyn Probe, tools: &[String]) -> HostFacts {
    let mut f = HostFacts::default();
    let os = p.os_name();
    let raw_arch = p
        .run("uname", &["-m"])
        .map(|o| o.stdout.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| p.arch_raw());
    f.arch = Some(normalize_arch(&raw_arch)).filter(|a| !a.is_empty());
    match os.as_str() {
        "macos" => {
            f.os_name = Some("macos".into());
            f.os_version = p
                .run("sw_vers", &["-productVersion"])
                .map(|o| o.stdout.trim().to_string())
                .filter(|v| !v.is_empty());
            f.ram_gib = p
                .run("sysctl", &["-n", "hw.memsize"])
                .and_then(|o| parse_memsize_bytes(&o.stdout));
        }
        "linux" => {
            let (name, ver) = p
                .read_file("/etc/os-release")
                .map(|t| parse_os_release(&t))
                .unwrap_or_else(|| ("linux".to_string(), None));
            f.os_name = Some(name);
            f.os_version = ver;
            f.ram_gib = p.read_file("/proc/meminfo").and_then(|t| parse_meminfo(&t));
        }
        "windows" => f.os_name = Some("windows".into()),
        other if !other.is_empty() => f.os_name = Some(other.to_ascii_lowercase()),
        _ => {}
    }
    f.cpus = p.cpus().filter(|n| *n > 0);
    // One thread per tool: the probe takes as long as the slowest tool, not the sum. A tool
    // that is not installed fails at once; one that hangs is killed after TOOL_TIMEOUT.
    let outs: Vec<Option<CmdOutput>> = std::thread::scope(|s| {
        let handles: Vec<_> = tools
            .iter()
            .map(|t| s.spawn(move || p.run(t, version_args(t))))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().ok().flatten())
            .collect()
    });
    for (t, out) in tools.iter().zip(outs) {
        let Some(out) = out else {
            continue;
        };
        // `java -version` prints to stderr; most tools print to stdout.
        let version = parse_version(&out.stdout)
            .or_else(|| parse_version(&out.stderr))
            .unwrap_or_else(|| "unknown".to_string());
        f.tools.push(ToolFact {
            name: t.clone(),
            version,
        });
    }
    f
}

impl HostFacts {
    /// `os:macos-15`: the OS name and the major version, or the name alone.
    pub fn os_token_value(&self) -> Option<String> {
        let n = self.os_name.as_ref()?;
        Some(match self.os_version.as_deref().and_then(major) {
            Some(m) => format!("{n}-{m}"),
            None => n.clone(),
        })
    }

    /// The tags of the `Capabilities` field, in priority order: `arch:`, `os:`, `cpu:`, `ram:`,
    /// then one `tool:` per tool.
    pub fn tokens(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(a) = &self.arch {
            v.push(format!("arch:{a}"));
        }
        if let Some(o) = self.os_token_value() {
            v.push(format!("os:{o}"));
        }
        if let Some(c) = self.cpus {
            v.push(format!("cpu:{c}"));
        }
        if let Some(r) = self.ram_gib {
            v.push(format!("ram:{r}g"));
        }
        for t in &self.tools {
            v.push(format!("tool:{}", t.name));
        }
        v
    }

    /// The facts as the bullet list of the agent item body.
    pub fn markdown(&self) -> String {
        let mut s = String::from(
            "Detected by crewboard. Never edit by hand: the agent detects them again at every `join` and `start`.\n\n",
        );
        let na = "not detected";
        s.push_str(&format!("- arch: {}\n", self.arch.as_deref().unwrap_or(na)));
        s.push_str(&format!(
            "- os: {}{}\n",
            self.os_name.as_deref().unwrap_or(na),
            self.os_version
                .as_ref()
                .map(|v| format!(" {v}"))
                .unwrap_or_default()
        ));
        s.push_str(&format!(
            "- cpu cores: {}\n",
            self.cpus.map_or(na.to_string(), |c| c.to_string())
        ));
        s.push_str(&format!(
            "- ram: {}\n",
            self.ram_gib.map_or(na.to_string(), |r| format!("{r} GiB"))
        ));
        if self.tools.is_empty() {
            s.push_str("- tools: none of the checked tools found\n");
        } else {
            s.push_str("- tools:\n");
            for t in &self.tools {
                s.push_str(&format!("  - {} {}\n", t.name, t.version));
            }
        }
        s
    }
}

/// The probe of this host.
pub struct RealProbe;

#[allow(
    clippy::disallowed_methods,
    reason = "real elapsed time of a child process"
)]
fn run_with_timeout(program: &str, args: &[&str], limit: Duration) -> Option<CmdOutput> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        // a corepack shim (yarn, pnpm) must not download a package manager for a version probe
        .env("COREPACK_ENABLE_NETWORK", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::agent_env::scrub_children(&mut cmd);
    let mut child = cmd.spawn().ok()?;
    let mut out = child.stdout.take()?;
    let mut err = child.stderr.take()?;
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        out.read_to_string(&mut s).ok(); // a read error keeps what was read so far
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = String::new();
        err.read_to_string(&mut s).ok(); // a read error keeps what was read so far
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() < limit => crate::clock::pause(Duration::from_millis(10)),
            _ => {
                child.kill().ok(); // the child may have ended already
                child.wait().ok(); // the child may have ended already
                break None;
            }
        }
    };
    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();
    status
        .filter(std::process::ExitStatus::success)
        .map(|_| CmdOutput { stdout, stderr })
}

impl Probe for RealProbe {
    fn os_name(&self) -> String {
        std::env::consts::OS.to_string()
    }
    fn arch_raw(&self) -> String {
        std::env::consts::ARCH.to_string()
    }
    fn cpus(&self) -> Option<usize> {
        std::thread::available_parallelism().ok().map(|n| n.get())
    }
    fn run(&self, program: &str, args: &[&str]) -> Option<CmdOutput> {
        run_with_timeout(program, args, TOOL_TIMEOUT)
    }
    fn read_file(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A host with canned command output. A command that is not in the map is "not installed".
    #[derive(Default)]
    pub struct Fake {
        pub os: &'static str,
        pub arch: &'static str,
        pub cpus: Option<usize>,
        pub cmds: HashMap<String, (&'static str, &'static str)>,
        pub files: HashMap<&'static str, &'static str>,
    }

    impl Probe for Fake {
        fn os_name(&self) -> String {
            self.os.to_string()
        }
        fn arch_raw(&self) -> String {
            self.arch.to_string()
        }
        fn cpus(&self) -> Option<usize> {
            self.cpus
        }
        fn run(&self, program: &str, args: &[&str]) -> Option<CmdOutput> {
            let key = format!("{program} {}", args.join(" "));
            self.cmds.get(&key).map(|(o, e)| CmdOutput {
                stdout: (*o).to_string(),
                stderr: (*e).to_string(),
            })
        }
        fn read_file(&self, path: &str) -> Option<String> {
            self.files.get(path).map(|s| (*s).to_string())
        }
    }

    pub fn mac() -> Fake {
        let mut f = Fake {
            os: "macos",
            arch: "aarch64",
            cpus: Some(10),
            ..Fake::default()
        };
        for (k, o, e) in [
            ("uname -m", "arm64\n", ""),
            ("sw_vers -productVersion", "15.1.1\n", ""),
            ("sysctl -n hw.memsize", "34359738368\n", ""),
            ("cargo --version", "cargo 1.91.0 (abc123 2026-01-01)\n", ""),
            ("rustc --version", "rustc 1.91.0 (def456 2026-01-01)\n", ""),
            ("node --version", "v22.11.0\n", ""),
            (
                "docker --version",
                "Docker version 27.3.1, build ce12230\n",
                "",
            ),
            (
                "xcodebuild -version",
                "Xcode 16.1\nBuild version 16B40\n",
                "",
            ),
            ("uv --version", "uv 0.5.4 (abc 2024-11-27)\n", ""),
            ("python3 --version", "Python 3.13.1\n", ""),
            ("git --version", "git version 2.47.1\n", ""),
        ] {
            f.cmds.insert(k.to_string(), (o, e));
        }
        f
    }

    fn defaults() -> Vec<String> {
        tool_list(None, &[]).unwrap()
    }

    #[test]
    fn a_mac_is_read_from_its_command_output() {
        let f = detect(&mac(), &defaults());
        assert_eq!(f.arch.as_deref(), Some("arm64"));
        assert_eq!(f.os_name.as_deref(), Some("macos"));
        assert_eq!(f.os_version.as_deref(), Some("15.1.1"));
        assert_eq!(f.os_token_value().as_deref(), Some("macos-15"));
        assert_eq!(f.cpus, Some(10));
        assert_eq!(f.ram_gib, Some(32));
        let v = |n: &str| {
            f.tools
                .iter()
                .find(|t| t.name == n)
                .map(|t| t.version.clone())
        };
        assert_eq!(v("cargo").as_deref(), Some("1.91.0"));
        assert_eq!(v("node").as_deref(), Some("22.11.0"));
        assert_eq!(v("docker").as_deref(), Some("27.3.1"));
        assert_eq!(v("xcodebuild").as_deref(), Some("16.1"));
        assert_eq!(v("uv").as_deref(), Some("0.5.4"));
        assert_eq!(v("python3").as_deref(), Some("3.13.1"));
        assert_eq!(v("git").as_deref(), Some("2.47.1"));
        let t = f.tokens();
        assert_eq!(&t[..4], ["arch:arm64", "os:macos-15", "cpu:10", "ram:32g"]);
        assert!(t.contains(&"tool:cargo".to_string()));
        assert_eq!(f.tools.len(), 8);
    }

    #[test]
    fn a_linux_host_is_read_from_os_release_and_meminfo() {
        let mut f = Fake {
            os: "linux",
            arch: "x86_64",
            cpus: Some(4),
            ..Fake::default()
        };
        f.cmds.insert("uname -m".into(), ("x86_64\n", ""));
        f.cmds
            .insert("git --version".into(), ("git version 2.43.0\n", ""));
        f.files.insert(
            "/etc/os-release",
            "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\n",
        );
        f.files.insert(
            "/proc/meminfo",
            "MemTotal:       16318464 kB\nMemFree: 1 kB\n",
        );
        let d = detect(&f, &["git".into(), "cargo".into()]);
        assert_eq!(d.arch.as_deref(), Some("x86_64"));
        assert_eq!(d.os_token_value().as_deref(), Some("linux-ubuntu-24"));
        assert_eq!(d.ram_gib, Some(16));
        // control: a tool that is not installed is not listed, an installed one is
        assert_eq!(d.tools.len(), 1);
        assert_eq!(d.tools[0].name, "git");
    }

    #[test]
    fn missing_pieces_stay_none_and_make_no_tag() {
        let f = Fake {
            os: "windows",
            arch: "AMD64",
            ..Fake::default()
        };
        let d = detect(&f, &defaults());
        assert_eq!(
            d.arch.as_deref(),
            Some("x86_64"),
            "arch falls back to the build value"
        );
        assert_eq!(d.os_token_value().as_deref(), Some("windows"));
        assert_eq!(d.cpus, None);
        assert_eq!(d.ram_gib, None);
        assert!(d.tools.is_empty());
        assert_eq!(d.tokens(), ["arch:x86_64", "os:windows"]);
        assert!(d.markdown().contains("none of the checked tools found"));
    }

    #[test]
    fn version_parsing_takes_the_first_real_version_and_never_a_date() {
        for (text, want) in [
            ("cargo 1.91.0 (abc 2026-01-01)", Some("1.91.0")),
            ("v22.11.0", Some("22.11.0")),
            ("openjdk version \"21.0.5\" 2024-10-15", Some("21.0.5")),
            ("Docker version 27.3.1, build ce12230", Some("27.3.1")),
            ("Xcode 16\nBuild version 16B40", Some("16")),
            ("go version go1.23.2 darwin/arm64", Some("1.23.2")),
            ("built 2026-01-01", None),
            ("", None),
            ("version 1..2", None),
            ("git version 2.56.0.windows.2", Some("2.56.0")),
            ("tool 2026-01-01", None),
        ] {
            assert_eq!(parse_version(text).as_deref(), want, "{text:?}");
        }
    }

    #[test]
    fn a_tool_with_unparsable_output_is_available_with_version_unknown() {
        let mut f = mac();
        f.cmds.insert("git --version".into(), ("hello\n", ""));
        let d = detect(&f, &["git".into()]);
        assert_eq!(d.tools[0].version, "unknown");
    }

    #[test]
    fn a_version_on_stderr_is_found_when_stdout_is_empty() {
        let mut f = mac();
        f.cmds.insert(
            "java -version".into(),
            ("", "openjdk version \"21.0.5\" 2024-10-15\n"),
        );
        let d = detect(&f, &["java".into()]);
        assert_eq!(d.tools[0].version, "21.0.5");
    }

    #[test]
    fn arch_names_are_normalised_both_ways() {
        for (raw, want) in [
            ("aarch64", "arm64"),
            ("arm64", "arm64"),
            ("AMD64", "x86_64"),
            ("x86_64", "x86_64"),
            ("x64", "x86_64"),
            ("riscv64", "riscv64"),
        ] {
            assert_eq!(normalize_arch(raw), want);
        }
        assert_ne!(normalize_arch("arm64"), normalize_arch("x86_64"));
    }

    #[test]
    fn os_release_and_memory_parsers_reject_garbage() {
        assert_eq!(
            parse_os_release("ID=arch\n"),
            ("linux-arch".to_string(), None)
        );
        assert_eq!(parse_os_release("nonsense"), ("linux".to_string(), None));
        assert_eq!(parse_memsize_bytes("abc"), None);
        assert_eq!(parse_memsize_bytes("0"), None);
        assert_eq!(parse_memsize_bytes("17179869184"), Some(16));
        assert_eq!(parse_meminfo("MemTotal: x kB"), None);
        assert_eq!(parse_meminfo("Other: 1 kB"), None);
        assert_eq!(major("24.04"), Some("24".into()));
        assert_eq!(major("rolling"), None);
    }

    #[test]
    fn the_tool_list_is_the_default_or_the_config_and_a_bad_name_is_refused() {
        assert_eq!(tool_list(None, &[]).unwrap().len(), DEFAULT_TOOLS.len());
        let own = vec!["cargo".to_string(), "go".to_string(), "cargo".to_string()];
        assert_eq!(
            tool_list(Some(&own), &[]).unwrap(),
            ["cargo", "go"],
            "dedup"
        );
        for bad in ["", "Cargo", "../x", "a b", "-rf", "x/y"] {
            let r = tool_list(Some(&[bad.to_string()]), &[]);
            assert!(r.is_err(), "{bad:?}");
        }
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_board_list_is_added_to_the_default_and_to_an_agent_list_without_duplicates() {
        let board = strs(&["java", "gradle", "git"]);
        let with_default = tool_list(None, &board).unwrap();
        assert_eq!(
            with_default.len(),
            DEFAULT_TOOLS.len() + 2,
            "git is not twice"
        );
        assert_eq!(with_default[..DEFAULT_TOOLS.len()], *DEFAULT_TOOLS);
        assert_eq!(&with_default[DEFAULT_TOOLS.len()..], ["java", "gradle"]);
        // an agent list replaces the default, and the board list still comes after it
        let own = strs(&["go"]);
        assert_eq!(
            tool_list(Some(&own), &board).unwrap(),
            ["go", "java", "gradle", "git"]
        );
    }

    #[test]
    fn a_bad_board_tool_is_a_config_error_that_names_the_file_and_the_entry() {
        let e = tool_list(None, &strs(&["java", "Gradle"]))
            .unwrap_err()
            .to_string();
        assert!(e.contains(".crewboard.toml"), "{e}");
        assert!(e.contains("detect_tools"), "{e}");
        assert!(e.contains("'Gradle'"), "{e}");
    }

    #[test]
    fn the_default_list_has_no_stack_in_front_and_keeps_every_old_name() {
        assert_eq!(DEFAULT_TOOLS[0], "git");
        for old in [
            "cargo",
            "rustc",
            "node",
            "npm",
            "pnpm",
            "yarn",
            "bun",
            "deno",
            "go",
            "docker",
            "xcodebuild",
            "uv",
            "python3",
            "git",
        ] {
            assert!(DEFAULT_TOOLS.contains(&old), "{old}");
        }
        for t in DEFAULT_TOOLS {
            assert!(valid_tool_name(t), "{t}");
        }
    }

    #[test]
    fn a_tool_of_the_board_list_is_found_when_it_is_on_the_path_and_not_otherwise() {
        // the Fake probe is the PATH: a command that is not in it does not exist
        let mut f = Fake {
            os: "linux",
            arch: "x86_64",
            ..Fake::default()
        };
        f.cmds
            .insert("git --version".into(), ("git version 2.47.1\n", ""));
        f.cmds.insert(
            "java -version".into(),
            ("", "openjdk version \"21.0.5\" 2024-10-15\n"),
        );
        f.cmds.insert(
            "go version".into(),
            ("go version go1.23.4 linux/amd64\n", ""),
        );
        let board = strs(&["java", "gradle"]);
        let d = detect(&f, &tool_list(None, &board).unwrap());
        let got: Vec<(&str, &str)> = d
            .tools
            .iter()
            .map(|t| (t.name.as_str(), t.version.as_str()))
            .collect();
        assert_eq!(
            got,
            [("git", "2.47.1"), ("go", "1.23.4"), ("java", "21.0.5")]
        );
        // the fact format is unchanged, and `gradle` (not installed) is no fact
        let tokens = d.tokens();
        assert!(tokens.contains(&"tool:java".to_string()), "{tokens:?}");
        assert!(!tokens.iter().any(|t| t == "tool:gradle"), "{tokens:?}");
    }

    #[test]
    fn the_real_probe_runs_a_command_and_reports_a_missing_one_as_none() {
        let p = RealProbe;
        // `git` is on every machine that builds this crate
        let git = p.run("git", &["--version"]).expect("git runs");
        assert!(parse_version(&git.stdout).is_some(), "{git:?}");
        assert!(
            p.run("crewboard-no-such-tool-xyz", &["--version"])
                .is_none()
        );
        assert!(p.cpus().is_some_and(|n| n >= 1));
        // a failing command (exit 1) is not "available"
        assert!(p.run("git", &["no-such-subcommand-xyz"]).is_none());
    }

    #[test]
    fn a_hanging_command_is_killed_at_the_limit() {
        if cfg!(windows) {
            return;
        }
        let t = Instant::now();
        let r = run_with_timeout("sleep", &["30"], Duration::from_millis(200));
        assert!(r.is_none());
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn the_default_list_probes_node_and_go_tools_and_reads_their_versions() {
        for t in ["npm", "pnpm", "yarn", "bun", "deno", "go"] {
            assert!(DEFAULT_TOOLS.contains(&t), "{t}");
        }
        let mut f = Fake {
            os: "linux",
            arch: "x86_64",
            ..Fake::default()
        };
        for (k, o) in [
            ("node --version", "v22.11.0\n"),
            ("npm --version", "10.9.0\n"),
            ("pnpm --version", "9.12.3\n"),
            ("yarn --version", "1.22.22\n"),
            ("bun --version", "1.1.38\n"),
            (
                "deno --version",
                "deno 2.1.4 (stable, release, x86_64-unknown-linux-gnu)\nv8 13.0\n",
            ),
            ("go version", "go version go1.23.4 linux/amd64\n"),
        ] {
            f.cmds.insert(k.to_string(), (o, ""));
        }
        let d = detect(&f, &defaults());
        let got: Vec<(String, String)> = d
            .tools
            .iter()
            .map(|t| (t.name.clone(), t.version.clone()))
            .collect();
        // the order of the list, whatever thread ends first
        assert_eq!(
            got,
            [
                ("node", "22.11.0"),
                ("npm", "10.9.0"),
                ("pnpm", "9.12.3"),
                ("yarn", "1.22.22"),
                ("bun", "1.1.38"),
                ("deno", "2.1.4"),
                ("go", "1.23.4"),
            ]
            .map(|(a, b)| (a.to_string(), b.to_string()))
        );
        // control: a host without them has none of these tags
        let bare = detect(&Fake::default(), &defaults());
        assert!(bare.tools.is_empty(), "{:?}", bare.tools);
        // a word that only starts with "go" is not a version
        assert_eq!(parse_version("good 1.0"), Some("1.0".into()));
        assert_eq!(parse_version("gopher"), None);
    }
}
