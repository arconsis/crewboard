//! The identity mode on an `init` rerun (spec/identity, guides/owner#crew-accounts).
//!
//! A board config names one of three modes: `crew_login` (the old name `app_login`) for a shared
//! crew account, `insecure_one_account = true` for a one-person trial, or neither key (each agent
//! its own account). Every approved agent is bound to its account, so a rerun of `init` (an
//! upgrade note asks for one) must not switch the mode by the way:
//!
//! - no identity flag: the mode of the existing board config stays (the local file, and
//!   `.crewboard.toml` on the default branch), with one line that names it;
//! - a flag of the same mode: as before (`--crew-login` may name another crew account);
//! - a flag of another mode than an existing mode key: refused before any write
//!   (`init-identity-mode-change`). A file without a mode key (an own-accounts board, or an old
//!   trial board) takes the mode of the flag.

use crate::error::{CbError, CbResult};

/// The rule of a rerun whose flag asks for another identity mode than the board config has.
pub const INIT_IDENTITY_MODE_CHANGE: &str = "init-identity-mode-change";

/// The identity mode of a board config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// `crew_login` (or `app_login`): the shared crew account.
    Crew(String),
    /// `insecure_one_account = true`: the one-person trial.
    OneAccount,
    /// Neither key: each agent acts as its own account.
    OwnAccounts,
}

impl Mode {
    /// The mode of a board config table. Pure.
    pub fn of(t: &toml::Table) -> Self {
        let crew = t
            .get("crew_login")
            .or_else(|| t.get("app_login"))
            .and_then(toml::Value::as_str);
        match crew {
            Some(l) => Mode::Crew(l.to_string()),
            None if t.get("insecure_one_account").and_then(toml::Value::as_bool) == Some(true) => {
                Mode::OneAccount
            }
            None => Mode::OwnAccounts,
        }
    }

    /// The mode that the flags of the rerun ask for; `None`: no identity flag.
    fn of_flags(crew_login: Option<&str>, one_account: bool) -> Option<Self> {
        match (crew_login, one_account) {
            (Some(l), _) => Some(Mode::Crew(l.to_string())),
            (None, true) => Some(Mode::OneAccount),
            (None, false) => None,
        }
    }

    fn describe(&self) -> String {
        match self {
            Mode::Crew(l) => format!("crew_login {l}"),
            Mode::OneAccount => "insecure_one_account (a one-person trial)".into(),
            Mode::OwnAccounts => "own accounts (no crew_login, no insecure_one_account)".into(),
        }
    }

    fn same_kind(&self, other: &Mode) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

/// The identity of the run: the crew login and the trial flag that init uses, and the line to
/// print when the mode of an existing file was kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub crew_login: Option<String>,
    pub one_account: bool,
    pub kept: Option<String>,
}

impl Identity {
    fn of(m: &Mode, kept: Option<String>) -> Self {
        Identity {
            crew_login: match m {
                Mode::Crew(l) => Some(l.clone()),
                _ => None,
            },
            one_account: *m == Mode::OneAccount,
            kept,
        }
    }
}

/// Decide the identity of the run. `existing`: the modes of the board configs that exist, each
/// with the name of its file. Refused (before any write) when a flag asks for another mode than
/// a file with a mode key, or when no flag is given and the files name different modes. Pure.
pub fn resolve(
    crew_login: Option<&str>,
    one_account: bool,
    existing: &[(String, Mode)],
) -> CbResult<Identity> {
    let how = "init does not switch the identity mode of a board: every approved agent is bound to its account. To switch on purpose, edit the identity keys (crew_login, insecure_one_account) in the local board config and in .crewboard.toml on the default branch (a pull request), run init again, and let every agent join again. Nothing was written";
    match Mode::of_flags(crew_login, one_account) {
        Some(want) => {
            if let Some((file, have)) = existing
                .iter()
                .find(|(_, m)| *m != Mode::OwnAccounts && !m.same_kind(&want))
            {
                return Err(CbError::refused(
                    INIT_IDENTITY_MODE_CHANGE,
                    format!(
                        "{file} sets the identity mode {}, and this run asks for {}. {how}",
                        have.describe(),
                        want.describe()
                    ),
                ));
            }
            Ok(Identity::of(&want, None))
        }
        None => {
            let Some((_, first)) = existing.first() else {
                return Ok(Identity::of(&Mode::OwnAccounts, None));
            };
            if let Some((file, other)) = existing.iter().find(|(_, m)| m != first) {
                return Err(CbError::refused(
                    INIT_IDENTITY_MODE_CHANGE,
                    format!(
                        "{} sets the identity mode {}, but {file} sets {}. Pass the flag of the mode you want (--crew-login or --insecure-one-account), or make both files name the same mode. {how}",
                        existing[0].0,
                        first.describe(),
                        other.describe()
                    ),
                ));
            }
            let change = match first {
                Mode::Crew(_) => "pass --crew-login <account> to use another crew account",
                _ => "edit the board config by hand to change the mode",
            };
            let line = format!("identity: {} kept ({change})", first.describe());
            Ok(Identity::of(first, Some(line)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str) -> toml::Table {
        toml::from_str(text).unwrap()
    }

    fn file(m: Mode) -> Vec<(String, Mode)> {
        vec![("demo.board.toml".into(), m)]
    }

    #[test]
    fn the_mode_of_a_file_comes_from_its_identity_keys() {
        assert_eq!(
            Mode::of(&t("crew_login = \"crew-bot\"")),
            Mode::Crew("crew-bot".into())
        );
        assert_eq!(
            Mode::of(&t("app_login = \"crew-bot\"")),
            Mode::Crew("crew-bot".into())
        );
        assert_eq!(
            Mode::of(&t("insecure_one_account = true")),
            Mode::OneAccount
        );
        assert_eq!(
            Mode::of(&t("insecure_one_account = false")),
            Mode::OwnAccounts
        );
        assert_eq!(Mode::of(&t("")), Mode::OwnAccounts);
    }

    #[test]
    fn a_rerun_without_a_flag_keeps_the_mode_and_says_so() {
        let id = resolve(None, false, &file(Mode::Crew("crew-bot".into()))).unwrap();
        assert_eq!(id.crew_login.as_deref(), Some("crew-bot"));
        assert!(!id.one_account);
        assert_eq!(
            id.kept.as_deref(),
            Some(
                "identity: crew_login crew-bot kept (pass --crew-login <account> to use another crew account)"
            )
        );
        let id = resolve(None, false, &file(Mode::OneAccount)).unwrap();
        assert!(id.one_account && id.crew_login.is_none() && id.kept.is_some());
        // a new board: no file, no flag, own accounts, nothing to say
        let id = resolve(None, false, &[]).unwrap();
        assert_eq!(
            (id.crew_login, id.one_account, id.kept),
            (None, false, None)
        );
    }

    #[test]
    fn a_flag_of_the_same_mode_or_on_a_file_without_a_mode_key_sets_it() {
        let id = resolve(
            Some("crew-bot"),
            false,
            &file(Mode::Crew("crew-bot".into())),
        )
        .unwrap();
        assert_eq!(id.crew_login.as_deref(), Some("crew-bot"));
        assert!(id.kept.is_none());
        // another crew account is the same mode
        let id = resolve(Some("crew-2"), false, &file(Mode::Crew("crew-bot".into()))).unwrap();
        assert_eq!(id.crew_login.as_deref(), Some("crew-2"));
        // an old trial board without the key keeps working with the flag (spec/identity)
        let id = resolve(None, true, &file(Mode::OwnAccounts)).unwrap();
        assert!(id.one_account);
    }

    #[test]
    fn a_flag_of_another_mode_or_two_files_that_disagree_are_refused() {
        let e = resolve(None, true, &file(Mode::Crew("crew-bot".into())))
            .unwrap_err()
            .to_string();
        assert!(e.contains(INIT_IDENTITY_MODE_CHANGE), "{e}");
        assert!(e.contains("crew_login crew-bot"), "{e}");
        assert!(e.contains("edit the identity keys"), "{e}");
        let e = resolve(Some("crew-bot"), false, &file(Mode::OneAccount))
            .unwrap_err()
            .to_string();
        assert!(e.contains(INIT_IDENTITY_MODE_CHANGE), "{e}");
        let two = vec![
            ("demo.board.toml".to_string(), Mode::Crew("crew-bot".into())),
            (".crewboard.toml".to_string(), Mode::OwnAccounts),
        ];
        let e = resolve(None, false, &two).unwrap_err().to_string();
        assert!(e.contains(INIT_IDENTITY_MODE_CHANGE), "{e}");
    }
}
