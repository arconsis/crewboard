//! Read AUTHORITY.md of the default branch of the main repo. Every command that checks what the
//! authority allows reads it through here; the commands do not import each other for it.

use super::Ctx;
use crate::authority::Authority;
use crate::error::CbResult;

/// The text of AUTHORITY.md on the default branch of the main repo; `None` if it is not there.
/// The context keeps the text it read when it loaded, so a command reads the file once; after
/// [`Ctx::forget_authority`] (or a failed first read) it reads again.
pub fn authority_text(ctx: &Ctx) -> CbResult<Option<String>> {
    if let Some(t) = ctx.authority_text_cached() {
        return Ok(t);
    }
    let t = super::fetch_authority_text(&ctx.client, ctx.main_repo(), &ctx.cfg.authority)?;
    ctx.keep_authority_text(t.clone());
    Ok(t)
}

/// The authority that applies now: AUTHORITY.md of the default branch, read fresh. A missing
/// file or one without front matter means the defaults that `init` writes; a front matter that
/// does not parse is a refusal (`authority-bad-front-matter`), never a silent default.
pub fn load_authority(ctx: &Ctx) -> CbResult<Authority> {
    let a = match authority_text(ctx)? {
        Some(t) => crate::authority::parse(&t)?.unwrap_or_default(),
        None => Authority::default(),
    };
    warn_contradictions_once(&a);
    Ok(a)
}

/// The authority for a warning only: a missing file, a read error or a bad front matter give
/// `None` (`start` reports those itself).
pub fn load_authority_quiet(ctx: &Ctx) -> Option<Authority> {
    let t = authority_text(ctx).ok()??;
    crate::authority::parse(&t).ok()?
}

/// Print the contradictions of AUTHORITY.md to stderr, once per process (every command that
/// re-reads the file calls [`load_authority`]).
fn warn_contradictions_once(a: &Authority) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let w = a.contradiction_warnings();
    if !w.is_empty() && !DONE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        eprint!("{w}");
    }
}
