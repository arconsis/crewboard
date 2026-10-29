//! Refresh the agent's copy of the board config from the default branch.
//!
//! `start` (and so `resume`) reads `.crewboard.toml` from the default branch of the main repo
//! once. A change is written to the agent config and named in one line. A change of an
//! identity key (see [`crate::config::BoardConfig::identity_changes`]) is refused with rule
//! `board-identity-changed`: a moved board is a deliberate re-join. A failed read keeps the
//! copy, prints one note and never stops `start`. `status` reads the file once and says when
//! the copy differs.

use super::AgentCtx;
use super::join::{FetchedBoard, fetch_board_file};
use crate::config::{BoardConfig, INITIATIVE_FILE};
use crate::error::{CbError, CbResult};

/// What `start` learned from the default branch.
pub struct Refresh {
    /// Lines for the output of `start` (empty when nothing changed).
    pub text: String,
    /// The file as read, for the toolchain check; `None` when it could not be read.
    pub fresh: Option<BoardConfig>,
}

/// The note for a read that failed. The copy stays in force.
pub fn read_note(repo: &str, e: &CbError) -> String {
    format!(
        "note: cannot read {INITIATIVE_FILE} from {repo} ({e}); the local copy of the board config stays in force\n"
    )
}

/// Compare a fetched file with the copy and write the change. Pure apart from the file write.
pub fn apply(ac: &AgentCtx, repo: &str, f: &FetchedBoard) -> CbResult<String> {
    let copy = &ac.agent.board;
    if &f.cfg == copy {
        return Ok(String::new());
    }
    let ident = copy.identity_changes(&f.cfg);
    if !ident.is_empty() {
        return Err(CbError::refused(
            "board-identity-changed",
            format!(
                "{INITIATIVE_FILE} in {repo}@{} changes {}; an agent never follows a moved board. The local copy stays as it is. If the change is right, join again: crewboard join --repo {repo}",
                f.short_sha(),
                ident.join(", ")
            ),
        ));
    }
    let keys = copy.changed_keys(&f.cfg);
    ac.ctx
        .cd
        .update_board(&ac.ctx.cfg.initiative, &f.cfg, &f.sha)?;
    Ok(format!(
        "board config updated from {repo}@{}: {}\n",
        f.short_sha(),
        keys.join(", ")
    ))
}

/// The refresh of `start`: one read of the file.
pub fn refresh(ac: &AgentCtx) -> CbResult<Refresh> {
    let repo = ac.ctx.main_repo();
    match fetch_board_file(&ac.ctx.client, repo) {
        Ok(f) => Ok(Refresh {
            text: apply(ac, repo, &f)?,
            fresh: Some(f.cfg),
        }),
        Err(e) => Ok(Refresh {
            text: read_note(repo, &e),
            fresh: None,
        }),
    }
}

/// The line of `status` when the copy differs from the default branch; `Err` carries the note
/// of a failed read. One read. The stored blob SHA is the shortcut: the same SHA means the
/// same file.
pub fn differs(ac: &AgentCtx) -> Result<Option<String>, String> {
    let repo = ac.ctx.main_repo();
    let f = fetch_board_file(&ac.ctx.client, repo).map_err(|e| read_note(repo, &e))?;
    if ac.agent.board_sha.as_deref() == Some(f.sha.as_str()) || f.cfg == ac.agent.board {
        return Ok(None);
    }
    Ok(Some(format!(
        "board config differs from {repo} default branch; run crewboard resume ({})",
        ac.agent.board.changed_keys(&f.cfg).join(", ")
    )))
}
