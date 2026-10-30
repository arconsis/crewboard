//! The local state file: `<config-dir>/state/<initiative>.<agent>.json`.
//!
//! One JSON file, one [`StateCache`]. The struct is split into one struct per feature. Each one is a
//! `#[serde(flatten)]` field of [`StateCache`], so the JSON keeps its flat field names and a file
//! that an older build wrote still loads.
//!
//! Most of the file is only a cache: deleting it costs reads, nothing more. Two parts are
//! **state**, not cache, and a lost file loses them:
//!
//! - `selftest_used` ([`SelftestState`]) is a replay guard. A selftest nonce that suppressed one
//!   forged comment must not suppress a second one. Without the file, the second forgery is
//!   treated as new.
//! - `deferred` ([`LivenessState`]) holds events that a `poll --quiet` found. Nothing else
//!   holds them, so a lost file loses those events.
//!
//! A third part is memory that GitHub cannot give back: `posted_notes` ([`NoteState`]), the ids of
//! the signed notes that this agent posted on tickets. A lost file loses it: a deletion of an
//! earlier note is then not seen (spec/security-details#deleted-notes).
//!
//! Only a cache (rebuilt by the next `watch` or read from GitHub): `etags`, `registry`, `polls`,
//! `last_ok`, `last_cursor` (statistics and ETags), the ticket index and snapshots, the
//! late-work memory (`late_*`), `liveness` (the last finding per agent, only to avoid
//! repeated events), `tick_requested`, `merged_told` and `owner_route_judged` (a lost entry
//! costs one read). `local_cursor` is the event cursor of a headless config dir; a lost one
//! starts the cursor again.
//!
//! Saving is locked and merges the two state parts. A `monitor` loop and a model command of the
//! same agent can run at once. [`StateCache::save`] takes an advisory lock (`<file>.lock`), reads
//! the file again, merges `selftest_used` (union) and `deferred` (the events this copy added are
//! added, the ones it consumed are removed, the ones a concurrent writer added stay), and writes
//! the result atomically. `posted_notes` is merged too (`NoteState::merge_disk`): a note that
//! a concurrent writer recorded stays, a note that it forgot stays forgotten. So are
//! `notes_recheck` and `notes_too_large` (a union; a mark that this copy cleared stays cleared).
//! Every other key is cache: the copy that saves last wins, and a lost value costs one read.

pub use crate::events::TicketSnap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The whole state file.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateCache {
    #[serde(flatten)]
    pub poll: PollState,
    #[serde(flatten)]
    pub index: TicketIndex,
    #[serde(flatten)]
    pub selftest: SelftestState,
    #[serde(flatten)]
    pub liveness: LivenessState,
    #[serde(flatten)]
    pub late: LateState,
    #[serde(flatten)]
    pub tick: TickState,
    #[serde(flatten)]
    pub owner: OwnerState,
    #[serde(flatten)]
    pub notes: NoteState,
}

/// ETags, poll statistics and cursors of `poll` and `watch`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PollState {
    #[serde(default)]
    pub etags: BTreeMap<String, String>,
    #[serde(default)]
    pub polls: u64,
    #[serde(default)]
    pub last_ok: Option<String>,
    /// The cursor text of the last successful cycle (for error lines).
    #[serde(default)]
    pub last_cursor: Option<String>,
    /// The registry issue comments with the ETag of the answer (a 304 needs the body).
    #[serde(default)]
    pub registry: Option<CachedComments>,
    /// The own event cursor of a headless config dir (spec/agents#headless-coordinator): `poll` and
    /// `watch` read and move this one and never write the `Cursor` field of the shared agent item.
    #[serde(default)]
    pub local_cursor: Option<String>,
}

/// The ticket index and the last snapshot of each ticket.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketIndex {
    /// Ticket index (spec/board#item-ids-are-the-source-of-truth): `owner/repo#n` -> item id,
    /// learned from the signed info comments of the coordinator. A cache only: it can be rebuilt by
    /// `watch`.
    #[serde(default)]
    pub tickets: BTreeMap<String, String>,
    /// The one-time full scan of the open `crew:work` issues has run.
    #[serde(default)]
    pub tickets_scanned: bool,
    /// Status and agent of each known ticket at the last cycle (item id -> snapshot), to
    /// report changes. Ticket commands update it after their own writes.
    #[serde(default)]
    pub ticket_snaps: BTreeMap<String, TicketSnap>,
}

impl TicketIndex {
    /// Forget the tickets whose repo is not one of `repos`, and ask for a new full scan if any
    /// was forgotten. After a repo transfer or rename the index still holds keys under the old
    /// name; they would show as tickets that do not exist (a cache only, so nothing is lost).
    pub fn drop_foreign(&mut self, repos: &[String]) {
        let before = self.tickets.len();
        self.tickets.retain(|key, _| {
            key.rsplit_once('#')
                .is_some_and(|(repo, _)| repos.iter().any(|r| r.eq_ignore_ascii_case(repo)))
        });
        if self.tickets.len() != before {
            self.tickets_scanned = false;
        }
    }
}

/// The selftest replay guard. This is state, not cache.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelftestState {
    /// Selftest nonces that suppressed one forged comment already (spec/messages#selftest): a
    /// second forgery with the same nonce alerts. Pruned after one hour.
    #[serde(default)]
    pub selftest_used: BTreeMap<String, (u64, i64)>,
}

/// The coordinator's liveness findings and the events that wait for the next real poll.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LivenessState {
    /// The coordinator's last finding per agent (`stale` or `dead`), so that a change is
    /// reported once as an event. The board decides what is done; this only decides what is
    /// printed again.
    #[serde(default)]
    pub liveness: BTreeMap<String, String>,
    /// Events that the duties found during a `poll --quiet` (a peek prints nothing): the next
    /// real `poll` or `watch` prints them first. This is state, not cache.
    #[serde(default)]
    pub deferred: Vec<crate::events::Event>,
    /// The `deferred` events of the file at load time, so that a save can tell what this copy
    /// added and what it consumed. Not part of the file.
    #[serde(skip)]
    pub(crate) deferred_loaded: Vec<crate::events::Event>,
    /// Tickets (item ids) whose board Status the signed log did not support at the last check:
    /// the next cycle looks again before it tells the owner (`ops::log_check`).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub status_pending: BTreeSet<String>,
    /// Tickets (item ids) that another writer moved out of Done or Dropped and that the Status
    /// check has not read yet, with the time (Unix seconds) of the move that a cycle saw. A cycle
    /// reads at most `ops::log_check::SETBACKS_PER_CYCLE` of them, the oldest first.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub setback_queue: BTreeMap<String, i64>,
}

/// The memory of the late-work duty. The keys `stale_seen`, `stale_checked`,
/// `stale_tick_checked` and `stale_green` are read as aliases.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LateState {
    /// Late waits that were given as events (spec/events#stale-work): `<item id>|<case>` ->
    /// head SHA or `-`. A wait comes once per value.
    #[serde(default, alias = "stale_seen")]
    pub late_seen: BTreeMap<String, String>,
    /// When `poll` or `watch` read GitHub for the late waits last (Unix seconds).
    #[serde(default, alias = "stale_checked")]
    pub late_checked: Option<i64>,
    /// When `poll` or `watch` read GitHub for `ci-finished` last (Unix seconds). Apart from
    /// `late_checked`: its throttle is shorter (60 seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub late_ci_checked: Option<i64>,
    /// When the headless `tick` read GitHub for the late waits last (Unix seconds). Apart from
    /// `late_checked`, so neither starves the other.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "stale_tick_checked"
    )]
    pub late_tick_checked: Option<i64>,
    /// Head SHA -> Unix time since when its CI is green. CI that is green on a SHA stays green.
    #[serde(default, alias = "stale_green")]
    pub late_green: BTreeMap<String, i64>,
}

/// What the headless `tick` did already (spec/agents#headless-coordinator).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TickState {
    /// `<repo>#<issue>` -> id of the `done` comment for which a merge attempt already sent a
    /// request to the dev. The tick does not send it again.
    #[serde(default)]
    pub tick_requested: BTreeMap<String, u64>,
    /// `tick`: item id of an In Progress ticket -> the URL of the merged PR about which
    /// the dev was told already. A told PR is not read again. A cache only: the signed `info` on
    /// the ticket is the truth, and a lost cache costs one read, not a second `info`.
    #[serde(default)]
    pub merged_told: BTreeMap<String, String>,
}

/// The owner-route safety net.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnerState {
    /// Owner-route safety net (spec/tickets#workflow-files-the-owner-route): item id of a
    /// `Done` ticket -> the `updated` time of its board item when the net judged it. A judged
    /// ticket is not read again until the item changes.
    #[serde(default)]
    pub owner_route_judged: BTreeMap<String, String>,
    /// Unix time of the last run of the `needs-owner` label duty (`ops/needs_owner_sync.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_owner_checked: Option<i64>,
}

/// Per ticket, at most this many notes are remembered (the newest by comment id).
pub const MAX_NOTES_PER_TICKET: usize = 20;
/// At most this many tickets are remembered; above it, the ticket whose newest note is the oldest
/// goes first.
pub const MAX_NOTE_TICKETS: usize = 300;
/// A ticket whose newest note is older than this (90 days, in seconds) is forgotten. A closed
/// ticket gets no new note, so its memory ends 90 days after the close.
pub const NOTE_KEEP_SECS: i64 = 90 * 24 * 3600;

/// The key of a ticket in the note memory ([`NoteState`]): `owner/repo#n` with the repo in lower
/// case. GitHub owner and repo names are case-insensitive, so `Org/Repo#7` and `org/repo#7` are
/// one ticket. Every record and every lookup of the memory builds its key here.
pub fn note_key(repo: &str, issue: u64) -> String {
    format!("{}#{issue}", repo.to_ascii_lowercase())
}

/// What a remembered note is (`ops::own_notes::note_kind`). The state file holds the kebab-case
/// name (`close`, `verify-pass`, `verify-fail`, `assignment`, `reassign`, `creation`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteKind {
    /// A close note of every kind (Done or Dropped).
    Close,
    VerifyPass,
    VerifyFail,
    Assignment,
    Reassign,
    Creation,
    /// A name that this build does not know (a file of a later build): kept with its name, so
    /// that the file still loads and a save writes the name back as it was.
    Other(String),
}

impl NoteKind {
    /// The name in the state file and in the event text.
    pub fn as_str(&self) -> &str {
        match self {
            NoteKind::Close => "close",
            NoteKind::VerifyPass => "verify-pass",
            NoteKind::VerifyFail => "verify-fail",
            NoteKind::Assignment => "assignment",
            NoteKind::Reassign => "reassign",
            NoteKind::Creation => "creation",
            NoteKind::Other(name) => name,
        }
    }

    /// The kind of a name in the state file.
    fn of_name(name: &str) -> Self {
        match name {
            "close" => NoteKind::Close,
            "verify-pass" => NoteKind::VerifyPass,
            "verify-fail" => NoteKind::VerifyFail,
            "assignment" => NoteKind::Assignment,
            "reassign" => NoteKind::Reassign,
            "creation" => NoteKind::Creation,
            other => NoteKind::Other(other.to_string()),
        }
    }
}

impl Serialize for NoteKind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for NoteKind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(NoteKind::of_name(&String::deserialize(d)?))
    }
}

/// One signed note that this agent posted on a ticket.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PostedNote {
    /// The GitHub comment id.
    pub id: u64,
    /// What the note is.
    pub kind: NoteKind,
    /// When it was posted (Unix seconds).
    pub at: i64,
}

/// The signed notes that this agent posted on tickets (`owner/repo#n` -> notes, oldest first), so
/// that the coordinator sees when one of them is deleted (`ops::own_notes`,
/// spec/security-details#deleted-notes). GitHub cannot give this back: a lost file loses it.
///
/// Bound: per ticket the newest [`MAX_NOTES_PER_TICKET`] notes; at most [`MAX_NOTE_TICKETS`]
/// tickets; a ticket whose newest note is older than [`NOTE_KEEP_SECS`] is forgotten.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteState {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub posted_notes: BTreeMap<String, Vec<PostedNote>>,
    /// Tickets (`owner/repo#n`) whose comments a later cycle reads again (the last read failed,
    /// a gone note waits until its event is reported, or the board Status contradicts the signed
    /// log while a remembered close note is still there), with the time (Unix seconds) of the
    /// mark or of the last read that kept it. A cycle reads at most
    /// `ops::own_notes::RECHECKS_PER_CYCLE` of them, the oldest first, so the reads rotate.
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "recheck_any"
    )]
    pub notes_recheck: BTreeMap<String, i64>,
    /// The note ids of the file when this copy last read it, so that a merge can tell what a
    /// concurrent writer added or forgot. Not part of the file.
    #[serde(skip)]
    pub(crate) notes_loaded: BTreeSet<u64>,
    /// The recheck marks of the file when this copy last read it (see `notes_loaded`). Not part
    /// of the file.
    #[serde(skip)]
    pub(crate) recheck_loaded: BTreeMap<String, i64>,
    /// Tickets (`owner/repo#n`) whose comment list had more pages than crewboard reads at the
    /// last read, and whose `notes-not-checked` event was shown and reported, with the time
    /// (Unix seconds) of that read. Their notes stay. Such a ticket is read again when its board
    /// row changes, or [`TOO_LARGE_RECHECK_SECS`] after the last read, and not in every cycle
    /// (a read costs `MAX_PAGES` pages); a read that is complete again removes the entry
    /// (`ops::own_notes`). The Status check of the cycle (`ops::log_check`) uses it too, also for
    /// a ticket without notes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub notes_too_large: BTreeMap<String, i64>,
    /// The `notes_too_large` entries of the file when this copy last read it (see
    /// `notes_loaded`). Not part of the file.
    #[serde(skip)]
    pub(crate) too_large_loaded: BTreeMap<String, i64>,
}

/// A ticket of [`NoteState::notes_too_large`] is read again this long (6 hours, in seconds) after
/// its last read, also when its board row did not change.
pub const TOO_LARGE_RECHECK_SECS: i64 = 6 * 3600;

/// Merge one map of marks of this copy (`mine`) with the map of the file (`disk`). `loaded`: the
/// map of the file when this copy last read it. A mark of the file that this copy did not see
/// was set by a concurrent writer: it is added. A mark that this copy set or moved stays. A mark
/// that this copy cleared stays cleared, and a mark that a concurrent writer cleared goes when
/// this copy did not touch it.
fn merge_marks(
    mine: &mut BTreeMap<String, i64>,
    loaded: &BTreeMap<String, i64>,
    disk: &BTreeMap<String, i64>,
) {
    mine.retain(|k, at| disk.contains_key(k) || loaded.get(k) != Some(at));
    for (k, at) in disk {
        if !mine.contains_key(k) && loaded.get(k) != Some(at) {
            mine.insert(k.clone(), *at);
        }
    }
}

/// `notes_recheck` as a map, or as the list that an earlier build wrote (each mark then counts as
/// the oldest).
fn recheck_any<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, i64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Form {
        Map(BTreeMap<String, i64>),
        List(Vec<String>),
    }
    Ok(match Form::deserialize(d)? {
        Form::Map(m) => m,
        Form::List(l) => l.into_iter().map(|k| (k, 0)).collect(),
    })
}

impl NoteState {
    /// Mark a ticket for a recheck at `now` (a mark that is there already moves to `now`: it
    /// goes to the back of the rotation).
    pub fn mark(&mut self, ticket: &str, now: i64) {
        self.notes_recheck.insert(ticket.to_string(), now);
    }

    /// The remembered notes of one ticket, oldest first.
    pub fn of(&self, ticket: &str) -> &[PostedNote] {
        self.posted_notes.get(ticket).map_or(&[], Vec::as_slice)
    }

    /// Every remembered note id.
    pub fn ids(&self) -> BTreeSet<u64> {
        self.posted_notes.values().flatten().map(|n| n.id).collect()
    }

    /// Remember a note, then apply the bound.
    pub fn record(&mut self, ticket: &str, note: PostedNote, now: i64) {
        let list = self.posted_notes.entry(ticket.to_string()).or_default();
        if !list.iter().any(|n| n.id == note.id) {
            list.push(note);
        }
        self.bound(now);
    }

    /// Forget one note (it was reported, or it is gone for another reason).
    pub fn forget(&mut self, ticket: &str, id: u64) {
        if let Some(list) = self.posted_notes.get_mut(ticket) {
            list.retain(|n| n.id != id);
            if list.is_empty() {
                self.posted_notes.remove(ticket);
                self.notes_recheck.remove(ticket);
            }
        }
    }

    /// Forget the tickets whose repo is not one of `repos` (a transfer or a rename).
    pub fn drop_foreign(&mut self, repos: &[String]) {
        let ours = |key: &str| {
            key.rsplit_once('#')
                .is_some_and(|(repo, _)| repos.iter().any(|r| r.eq_ignore_ascii_case(repo)))
        };
        self.posted_notes.retain(|k, _| ours(k));
        self.notes_recheck.retain(|k, _| ours(k));
        self.notes_too_large.retain(|k, _| ours(k));
    }

    /// Put the keys of a file that an older build wrote ([`note_key`]: the repo in lower case)
    /// together. The notes of `Org/Repo#7` and `org/repo#7` are one list.
    pub(crate) fn normalize_keys(&mut self) {
        let odd: Vec<String> = self
            .posted_notes
            .keys()
            .filter(|k| k.to_ascii_lowercase() != **k)
            .cloned()
            .collect();
        for k in odd {
            if let Some(list) = self.posted_notes.remove(&k) {
                let to = self.posted_notes.entry(k.to_ascii_lowercase()).or_default();
                for n in list {
                    if !to.iter().any(|x| x.id == n.id) {
                        to.push(n);
                    }
                }
            }
        }
        self.notes_recheck = std::mem::take(&mut self.notes_recheck)
            .into_iter()
            .map(|(k, at)| (k.to_ascii_lowercase(), at))
            .collect();
        self.notes_too_large = std::mem::take(&mut self.notes_too_large)
            .into_iter()
            .map(|(k, at)| (k.to_ascii_lowercase(), at))
            .collect();
    }

    /// Apply the bound of the type text.
    pub fn bound(&mut self, now: i64) {
        self.normalize_keys();
        for list in self.posted_notes.values_mut() {
            list.sort_by_key(|n| n.id);
            let extra = list.len().saturating_sub(MAX_NOTES_PER_TICKET);
            list.drain(..extra);
        }
        let newest = |l: &Vec<PostedNote>| l.iter().map(|n| n.at).max().unwrap_or(i64::MIN);
        self.posted_notes
            .retain(|_, l| !l.is_empty() && newest(l) > now - NOTE_KEEP_SECS);
        if self.posted_notes.len() > MAX_NOTE_TICKETS {
            let mut by_age: Vec<(i64, String)> = self
                .posted_notes
                .iter()
                .map(|(k, l)| (newest(l), k.clone()))
                .collect();
            by_age.sort();
            for (_, k) in by_age
                .iter()
                .take(self.posted_notes.len() - MAX_NOTE_TICKETS)
            {
                self.posted_notes.remove(k);
            }
        }
        let kept: BTreeSet<String> = self.posted_notes.keys().cloned().collect();
        self.notes_recheck.retain(|k, _| kept.contains(k));
        // A too-large ticket may have no notes (the Status check): its entry has its own bound.
        self.notes_too_large
            .retain(|_, at| *at > now - NOTE_KEEP_SECS);
        if self.notes_too_large.len() > MAX_NOTE_TICKETS {
            let mut by_age: Vec<(i64, String)> = self
                .notes_too_large
                .iter()
                .map(|(k, at)| (*at, k.clone()))
                .collect();
            by_age.sort();
            for (_, k) in by_age
                .iter()
                .take(self.notes_too_large.len() - MAX_NOTE_TICKETS)
            {
                self.notes_too_large.remove(k);
            }
        }
    }

    /// What this copy read from the file: the keys in one spelling, and the ids and marks of the
    /// file as the base of the next merge.
    pub(crate) fn loaded(&mut self) {
        self.normalize_keys();
        self.notes_loaded = self.ids();
        self.recheck_loaded = self.notes_recheck.clone();
        self.too_large_loaded = self.notes_too_large.clone();
    }

    /// Merge the notes of the file (`disk`) into this copy. A note that the file has and this
    /// copy did not know at its last read was recorded by a concurrent writer: it is added. A
    /// note that this copy knew from the file and that the file does not have any more was
    /// forgotten by a concurrent writer: it goes. A note that this copy forgot stays forgotten.
    ///
    /// The recheck marks are a union: a mark of the file that this copy did not see at its last
    /// read was set by a concurrent writer and is added; a mark that this copy set or moved since
    /// then stays. A mark that this copy cleared on purpose (it read the mark from the file and
    /// does not have it now) stays cleared, and a mark that a concurrent writer cleared goes when
    /// this copy did not touch it. Afterwards this copy counts the file's ids and marks as read.
    pub(crate) fn merge_disk(&mut self, disk: &NoteState, now: i64) {
        let on_disk = disk.ids();
        let known = std::mem::take(&mut self.notes_loaded);
        for list in self.posted_notes.values_mut() {
            list.retain(|n| on_disk.contains(&n.id) || !known.contains(&n.id));
        }
        let here = self.ids();
        for (ticket, list) in &disk.posted_notes {
            for n in list
                .iter()
                .filter(|n| !known.contains(&n.id) && !here.contains(&n.id))
            {
                self.posted_notes
                    .entry(ticket.clone())
                    .or_default()
                    .push(n.clone());
            }
        }
        let marked = std::mem::take(&mut self.recheck_loaded);
        merge_marks(&mut self.notes_recheck, &marked, &disk.notes_recheck);
        let large = std::mem::take(&mut self.too_large_loaded);
        merge_marks(&mut self.notes_too_large, &large, &disk.notes_too_large);
        self.bound(now);
        self.notes_loaded = on_disk;
        self.recheck_loaded = disk.notes_recheck.clone();
        self.too_large_loaded = disk.notes_too_large.clone();
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedComments {
    pub etag: String,
    pub comments: Vec<crate::comments::Comment>,
}

impl StateCache {
    /// The cursor text to show when a cycle failed before it could read the real one.
    pub fn last_cursor_hint(&self) -> String {
        self.poll
            .last_cursor
            .clone()
            .unwrap_or_else(|| "unknown".into())
    }

    /// A missing file is the empty state (a first run, no warning). Any other failure to read it
    /// is tried once more after a short pause (an older binary may be writing in place, or a
    /// scanner may hold the file). If it still fails, the run goes on with empty state and one
    /// warning on stderr, which tells the two cases apart:
    /// - the content is not valid JSON: the file is COPIED to `<name>.corrupt-<unix secs>` (the
    ///   live path is never renamed, so a writer that finishes later loses nothing; the next save
    ///   replaces it);
    /// - the file cannot be read (permissions, I/O): nothing is copied, the file stays.
    pub fn load(path: &Path) -> Self {
        Self::load_with(
            path,
            &mut std::io::stderr(),
            &|p| std::fs::read_to_string(p),
            std::time::Duration::from_millis(200),
        )
    }

    pub(crate) fn load_with(
        path: &Path,
        warn: &mut dyn std::io::Write,
        read: &dyn Fn(&Path) -> std::io::Result<String>,
        pause: std::time::Duration,
    ) -> Self {
        use std::io::ErrorKind;
        let mut last = String::new();
        let mut parse_failed = false;
        for attempt in 0..2 {
            if attempt > 0 {
                crate::clock::pause(pause);
            }
            match read(path) {
                Ok(text) => match serde_json::from_str::<Self>(&text) {
                    Ok(mut s) => {
                        s.liveness.deferred_loaded = s.liveness.deferred.clone();
                        s.notes.loaded();
                        return s;
                    }
                    Err(e) => {
                        parse_failed = true;
                        last = format!("not valid JSON: {e}");
                    }
                },
                Err(e) if e.kind() == ErrorKind::NotFound => return Self::default(),
                // invalid UTF-8 is damaged content, not an I/O problem
                Err(e) if e.kind() == ErrorKind::InvalidData => {
                    parse_failed = true;
                    last = format!("not valid UTF-8: {e}");
                }
                Err(e) => {
                    parse_failed = false;
                    last = format!("cannot be read ({:?}: {e})", e.kind());
                }
            }
        }
        let note = if parse_failed {
            let secs = crate::clock::now_epoch();
            let mut name = path.file_name().unwrap_or_default().to_os_string();
            name.push(format!(".corrupt-{secs}"));
            let copy = path.with_file_name(name);
            match std::fs::copy(path, &copy) {
                Ok(_) => format!("a copy is in {}", copy.display()),
                Err(e) => format!("could not copy it to {}: {e}", copy.display()),
            }
        } else {
            "the file is left in place".to_string()
        };
        writeln!(
            warn,
            "warning: state file {} {last}; {note}; starting with empty state",
            path.display()
        )
        .ok(); // output is best effort; a closed stream has nobody to tell
        Self::default()
    }

    /// This copy, merged with the file as it is now (see the module text): the state parts of a
    /// concurrent writer are kept. A file that is missing or does not parse changes nothing.
    fn merged_with_disk(&self, path: &Path) -> Self {
        let Some(disk) = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Self>(&t).ok())
        else {
            return self.clone();
        };
        let mut out = self.clone();
        for (k, v) in disk.selftest.selftest_used {
            out.selftest.selftest_used.entry(k).or_insert(v);
        }
        let consumed: Vec<_> = self
            .liveness
            .deferred_loaded
            .iter()
            .filter(|e| !self.liveness.deferred.contains(e))
            .collect();
        let mut merged: Vec<_> = disk
            .liveness
            .deferred
            .into_iter()
            .filter(|e| !consumed.contains(&e))
            .collect();
        for e in &self.liveness.deferred {
            if !merged.contains(e) {
                merged.push(e.clone());
            }
        }
        out.liveness.deferred = merged;
        out.notes.merge_disk(&disk.notes, crate::clock::now_epoch());
        out
    }

    /// Take into this copy the notes that a concurrent writer recorded or forgot since this copy
    /// read the file (`NoteState::merge_disk`). A long `watch` calls it at each cycle, so a note
    /// that a ticket command posted meanwhile is compared too. A missing or unreadable file
    /// changes nothing.
    pub fn refresh_notes(&mut self, path: &Path) {
        if let Some(disk) = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Self>(&t).ok())
        {
            self.notes
                .merge_disk(&disk.notes, crate::clock::now_epoch());
        }
    }

    /// Write the state atomically (temp file, sync, rename), under the lock of the file and merged
    /// with what a concurrent writer saved since this copy was loaded. A lock file that cannot be
    /// opened is a warning: the save goes on without it. Afterwards the notes of this copy are
    /// the notes of the file, and the file counts as read: a later save of the same copy (a long
    /// `watch`) merges against what it wrote, so a mark that it moved does not undo a clear of a
    /// concurrent writer.
    pub fn save(&mut self, path: &Path) -> crate::error::CbResult<()> {
        let mut lock_name = path.as_os_str().to_os_string();
        lock_name.push(".lock");
        let _lock = crate::fsutil::lock_exclusive(Path::new(&lock_name))
            .inspect_err(|e| {
                eprintln!(
                    "warning: state lock {}: {e}",
                    Path::new(&lock_name).display()
                )
            })
            .ok();
        let merged = self.merged_with_disk(path);
        let text = serde_json::to_string_pretty(&merged)
            .map_err(|e| crate::error::CbError::config(format!("state {}: {e}", path.display())))?;
        crate::fsutil::write_atomic_private(path, text.as_bytes()).map_err(|e| {
            crate::error::CbError::config(format!("cannot save state {}: {e}", path.display()))
        })?;
        self.notes = merged.notes;
        self.notes.loaded();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventClass;
    use serde_json::Value;

    /// The flat format as the current build writes it (19 fields).
    const FIXTURE: &str = include_str!("../tests/fixtures/state/current-format.json");

    fn fixture_file() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state").join("demo.claude@t1.json");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, FIXTURE).unwrap();
        (dir, p)
    }

    #[test]
    fn a_state_file_of_the_flat_format_loads_with_every_field_equal() {
        let (_d, p) = fixture_file();
        let c = StateCache::load(&p);
        assert_eq!(c.poll.etags.len(), 2);
        assert_eq!(c.poll.etags["registry"], "W/\"abc123\"");
        assert_eq!(c.poll.polls, 42);
        assert_eq!(c.poll.last_ok.as_deref(), Some("2026-10-23T08:15:00Z"));
        assert_eq!(
            c.poll.last_cursor.as_deref(),
            Some("example-org/app#7:1001")
        );
        let reg = c.poll.registry.as_ref().unwrap();
        assert_eq!(reg.etag, "W/\"abc123\"");
        assert_eq!(reg.comments[0].id, 1001);
        assert_eq!(
            c.poll.local_cursor.as_deref(),
            Some("example-org/app#7:1002")
        );
        assert_eq!(c.index.tickets["example-org/app#7"], "PVTI_item7");
        assert!(c.index.tickets_scanned);
        assert_eq!(
            c.index.ticket_snaps["PVTI_item7"].agent.as_deref(),
            Some("claude@t1")
        );
        assert_eq!(c.index.ticket_snaps["PVTI_item8"], TicketSnap::default());
        assert_eq!(c.selftest.selftest_used["nonce-aaaa"], (7, 1790850000));
        assert_eq!(c.liveness.liveness["claude@t1"], "stale");
        assert_eq!(c.liveness.deferred.len(), 1);
        assert_eq!(c.liveness.deferred[0].class, EventClass::Liveness);
        assert_eq!(c.liveness.deferred[0].body, "claude@t1 is stale");
        assert_eq!(c.late.late_seen["PVTI_item7|no-branch"], "-");
        assert_eq!(c.late.late_checked, Some(1790850100));
        assert_eq!(c.late.late_tick_checked, Some(1790850200));
        assert_eq!(c.late.late_green["0123abc"], 1790849000);
        assert_eq!(c.tick.tick_requested["example-org/app#7"], 1003);
        assert_eq!(
            c.tick.merged_told["PVTI_item7"],
            "https://github.com/example-org/app/pull/9"
        );
        assert_eq!(
            c.owner.owner_route_judged["PVTI_item8"],
            "2026-10-22T09:00:00Z"
        );
        // control: nothing was lost, so the JSON of the loaded struct is the fixture
        let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(serde_json::to_value(&c).unwrap(), fixture);
        // and the control can fail: an empty cache is not the fixture
        assert_ne!(
            serde_json::to_value(StateCache::default()).unwrap(),
            fixture
        );
    }

    #[test]
    fn save_and_load_round_trip_keeps_the_same_keys_and_values() {
        let (d, p) = fixture_file();
        let mut c = StateCache::load(&p);
        let out = d.path().join("out").join("again.json");
        c.save(&out).unwrap();
        let saved = std::fs::read_to_string(&out).unwrap();
        // same JSON modulo key order: the flat field names did not change
        let a: Value = serde_json::from_str(FIXTURE).unwrap();
        let b: Value = serde_json::from_str(&saved).unwrap();
        assert_eq!(a, b);
        assert_eq!(StateCache::load(&out), c);
        // a second round is byte-identical to the first
        let out2 = d.path().join("again2.json");
        StateCache::load(&out).save(&out2).unwrap();
        assert_eq!(std::fs::read_to_string(&out2).unwrap(), saved);
        // the keys are flat at the top: no group name appears in the file
        for group in ["poll", "index", "selftest", "late", "tick", "owner"] {
            assert!(b.get(group).is_none(), "{group} must not be a key");
        }
        assert_eq!(b.as_object().unwrap().len(), 19);
    }

    /// Moving a type between modules must not change the file: what the code writes for the
    /// fixture is the committed golden file, byte for byte (key order included). The golden
    /// file was written by the code before the move.
    #[test]
    fn a_saved_state_file_is_byte_identical_to_the_fixture() {
        let (d, p) = fixture_file();
        let out = d.path().join("saved.json");
        StateCache::load(&p).save(&out).unwrap();
        assert_eq!(
            std::fs::read_to_string(&out).unwrap().trim_end(),
            // a Windows checkout may turn the line ends of the golden file into CRLF
            include_str!("../tests/fixtures/state/saved-format.json")
                .replace("\r\n", "\n")
                .trim_end()
        );
    }

    #[test]
    fn the_replay_guard_and_the_deferred_events_are_state_and_survive_a_round_trip() {
        let (d, p) = fixture_file();
        let mut c = StateCache::load(&p);
        c.selftest
            .selftest_used
            .insert("nonce-bbbb".into(), (9, 1790850500));
        let mut e = c.liveness.deferred[0].clone();
        e.body = "second event".into();
        c.liveness.deferred.push(e);
        let out = d.path().join("s.json");
        c.save(&out).unwrap();
        let back = StateCache::load(&out);
        assert_eq!(back.selftest.selftest_used.len(), 2);
        assert_eq!(back.selftest.selftest_used["nonce-bbbb"], (9, 1790850500));
        assert_eq!(back.liveness.deferred.len(), 2);
        assert_eq!(back.liveness.deferred[1].body, "second event");
    }

    #[test]
    fn an_old_file_with_few_fields_and_an_unknown_key_still_loads() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("old.json");
        std::fs::write(
            &p,
            r#"{"etags":{"registry":"x"},"polls":3,"removed_key":1}"#,
        )
        .unwrap();
        let c = StateCache::load(&p);
        assert_eq!(c.poll.polls, 3);
        assert_eq!(c.poll.etags["registry"], "x");
        assert_eq!(c.index, TicketIndex::default());
        assert_eq!(c.liveness, LivenessState::default());
    }

    const NO_PAUSE: std::time::Duration = std::time::Duration::ZERO;

    fn load_quiet(p: &Path) -> (StateCache, String) {
        let mut w = Vec::new();
        let c = StateCache::load_with(p, &mut w, &|p| std::fs::read_to_string(p), NO_PAUSE);
        (c, String::from_utf8(w).unwrap())
    }

    fn files(d: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn a_missing_file_is_empty_state_without_a_warning() {
        let d = tempfile::tempdir().unwrap();
        let (c, w) = load_quiet(&d.path().join("none.json"));
        assert_eq!(c, StateCache::default());
        assert!(w.is_empty());
        assert!(files(d.path()).is_empty());
    }

    #[test]
    fn a_corrupt_or_truncated_file_is_copied_aside_with_one_warning_and_stays_in_place() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.json");
        let mut good = StateCache::default();
        good.poll.polls = 7;
        good.save(&p).unwrap();
        let full = std::fs::read_to_string(&p).unwrap();
        for bad in ["{ not json".to_string(), full[..full.len() / 2].to_string()] {
            std::fs::write(&p, &bad).unwrap();
            let (c, w) = load_quiet(&p);
            assert_eq!(c, StateCache::default());
            assert_eq!(w.lines().count(), 1, "{w}");
            assert!(w.contains("not valid JSON"), "{w}");
            assert!(w.contains(&p.display().to_string()), "{w}");
            assert!(w.contains("s.json.corrupt-"), "{w}");
            assert_eq!(std::fs::read_to_string(&p).unwrap(), bad, "live file stays");
            let aside: Vec<_> = files(d.path())
                .into_iter()
                .filter(|f| f.contains(".corrupt-"))
                .collect();
            assert_eq!(aside.len(), 1);
            let ap = d.path().join(&aside[0]);
            assert_eq!(std::fs::read_to_string(&ap).unwrap(), bad);
            std::fs::remove_file(&ap).unwrap();
        }
    }

    #[test]
    fn a_file_that_is_invalid_at_first_and_valid_at_the_second_read_is_loaded() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.json");
        let mut good = StateCache::default();
        good.poll.polls = 9;
        let text = serde_json::to_string(&good).unwrap();
        let calls = std::cell::Cell::new(0);
        let mut w = Vec::new();
        let c = StateCache::load_with(
            &p,
            &mut w,
            &|_| {
                calls.set(calls.get() + 1);
                Ok(if calls.get() == 1 {
                    text[..text.len() / 2].to_string()
                } else {
                    text.clone()
                })
            },
            NO_PAUSE,
        );
        assert_eq!(c.poll.polls, 9);
        assert_eq!(calls.get(), 2);
        assert!(w.is_empty());
        assert!(files(d.path()).is_empty());
    }

    #[test]
    fn an_io_error_is_retried_then_warned_as_unreadable_and_nothing_is_copied() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.json");
        std::fs::write(&p, "{}").unwrap();
        let calls = std::cell::Cell::new(0);
        let mut w = Vec::new();
        let c = StateCache::load_with(
            &p,
            &mut w,
            &|_| {
                calls.set(calls.get() + 1);
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            },
            NO_PAUSE,
        );
        let w = String::from_utf8(w).unwrap();
        assert_eq!(c, StateCache::default());
        assert_eq!(calls.get(), 2);
        assert!(w.contains("cannot be read"), "{w}");
        assert!(w.contains("PermissionDenied"), "{w}");
        assert!(!w.contains("not valid JSON"), "{w}");
        assert_eq!(files(d.path()), ["s.json"]);
    }

    #[test]
    fn a_directory_at_the_state_path_is_an_unreadable_file_not_a_corrupt_one() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.json");
        std::fs::create_dir(&p).unwrap();
        let (c, w) = load_quiet(&p);
        assert_eq!(c, StateCache::default());
        assert!(w.contains("cannot be read"), "{w}");
        assert_eq!(files(d.path()), ["s.json"]);
    }

    #[cfg(unix)]
    #[test]
    fn the_state_file_is_private_and_keeps_a_mode_that_the_owner_set() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.json");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        StateCache::default().save(&p).unwrap();
        assert_eq!(mode(&p), 0o600);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
        StateCache::default().save(&p).unwrap();
        assert_eq!(mode(&p), 0o640);
    }

    #[test]
    fn a_save_that_fails_is_an_error_naming_the_path() {
        let d = tempfile::tempdir().unwrap();
        let blocker = d.path().join("file");
        std::fs::write(&blocker, "x").unwrap();
        // the parent "dir" is a file: create_dir_all fails
        let p = blocker.join("s.json");
        let e = StateCache::default().save(&p).unwrap_err();
        assert!(e.to_string().contains("s.json"), "{e}");
    }

    /// A state file with the key names `stale_seen`, `stale_checked`, `stale_tick_checked` and
    /// `stale_green`.
    const OLD_NAMES: &str = include_str!("../tests/fixtures/state/before-late-names.json");

    #[test]
    fn a_state_file_with_the_old_stale_names_loads_into_the_late_fields() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("old.json");
        std::fs::write(&p, OLD_NAMES).unwrap();
        // control: the file has the `stale_*` names and none of the `late_*` names
        let raw: Value = serde_json::from_str(OLD_NAMES).unwrap();
        for old in [
            "stale_seen",
            "stale_checked",
            "stale_tick_checked",
            "stale_green",
        ] {
            assert!(raw.get(old).is_some(), "{old}");
        }
        assert!(raw.get("late_seen").is_none() && raw.get("late_checked").is_none());
        let mut c = StateCache::load(&p);
        assert_eq!(c.late.late_seen["PVTI_item7|no-branch"], "-");
        assert_eq!(c.late.late_checked, Some(1790850100));
        assert_eq!(c.late.late_tick_checked, Some(1790850200));
        assert_eq!(c.late.late_green["0123abc"], 1790849000);
        // everything else is the same as in the current format
        let (_d2, q) = fixture_file();
        let cur = StateCache::load(&q);
        assert_eq!(c.poll, cur.poll);
        assert_eq!(c.index, cur.index);
        assert_eq!(c.liveness, cur.liveness);
        assert_eq!(c.tick, cur.tick);
        // saved, it has the new names only
        let out = d.path().join("new.json");
        c.save(&out).unwrap();
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert!(saved.get("late_seen").is_some() && saved.get("late_checked").is_some());
        for old in [
            "stale_seen",
            "stale_checked",
            "stale_tick_checked",
            "stale_green",
        ] {
            assert!(saved.get(old).is_none(), "{old} was written");
        }
        assert_eq!(StateCache::load(&out), c);
        // `stale_tick_checked` is `late_tick_checked`
        std::fs::write(
            &p,
            r#"{"stale_tick_checked":77,"stale_seen":{"a|take":"-"}}"#,
        )
        .unwrap();
        let t = StateCache::load(&p);
        assert_eq!(t.late.late_tick_checked, Some(77));
        assert_eq!(t.late.late_checked, None);
        assert_eq!(t.late.late_seen["a|take"], "-");
    }

    #[test]
    fn drop_foreign_forgets_tickets_of_a_former_repo_and_asks_for_a_new_scan() {
        let mut i = TicketIndex::default();
        i.tickets
            .insert("old-owner/old-repo#7".into(), "PVTI_7".into());
        i.tickets
            .insert("example-org/app#7".into(), "PVTI_7".into());
        i.tickets_scanned = true;
        // nothing foreign: untouched, the scan flag stays
        i.drop_foreign(&["old-owner/old-repo".into(), "example-org/app".into()]);
        assert_eq!(i.tickets.len(), 2);
        assert!(i.tickets_scanned);
        // the repo moved
        i.drop_foreign(&["EXAMPLE-ORG/app".into()]);
        assert_eq!(
            i.tickets.keys().collect::<Vec<_>>(),
            vec!["example-org/app#7"]
        );
        assert!(!i.tickets_scanned);
    }

    fn event(body: &str) -> crate::events::Event {
        let mut e = StateCache::load(&fixture_file().1).liveness.deferred[0].clone();
        e.body = body.to_string();
        e
    }

    #[test]
    fn two_writers_do_not_lose_each_others_state() {
        let (_d, p) = fixture_file();
        // both copies are loaded before either one saves
        let mut a = StateCache::load(&p);
        let mut b = StateCache::load(&p);
        a.liveness.deferred.push(event("from a"));
        a.selftest.selftest_used.insert("nonce-a".into(), (1, 1));
        b.liveness.deferred.push(event("from b"));
        b.selftest.selftest_used.insert("nonce-b".into(), (2, 2));
        a.save(&p).unwrap();
        b.save(&p).unwrap();
        let back = StateCache::load(&p);
        let bodies: Vec<&str> = back
            .liveness
            .deferred
            .iter()
            .map(|e| e.body.as_str())
            .collect();
        assert_eq!(bodies, ["claude@t1 is stale", "from a", "from b"]);
        assert!(back.selftest.selftest_used.contains_key("nonce-a"));
        assert!(back.selftest.selftest_used.contains_key("nonce-b"));
        // control: a plain overwrite would have lost "from a"
        assert!(!serde_json::to_string(&b).unwrap().contains("from a"));
    }

    #[test]
    fn a_consumed_event_stays_consumed_and_a_new_one_of_the_other_writer_stays() {
        let (_d, p) = fixture_file();
        let mut watch = StateCache::load(&p);
        let mut poll = StateCache::load(&p);
        // the real cycle takes what the peek kept
        let taken = std::mem::take(&mut watch.liveness.deferred);
        assert_eq!(taken.len(), 1);
        // the peek found one more
        poll.liveness.deferred.push(event("found by the peek"));
        poll.save(&p).unwrap();
        watch.save(&p).unwrap();
        let back = StateCache::load(&p);
        let bodies: Vec<&str> = back
            .liveness
            .deferred
            .iter()
            .map(|e| e.body.as_str())
            .collect();
        assert_eq!(bodies, ["found by the peek"]);
    }

    #[test]
    fn many_threads_that_load_change_and_save_keep_every_update() {
        let (_d, p) = fixture_file();
        let n = 8;
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let p = p.clone();
                std::thread::spawn(move || {
                    for j in 0..5 {
                        let mut c = StateCache::load(&p);
                        c.selftest
                            .selftest_used
                            .insert(format!("nonce-{i}-{j}"), (1, 1));
                        c.liveness.deferred.push(event(&format!("e-{i}-{j}")));
                        c.save(&p).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let back = StateCache::load(&p);
        // load-modify-save is not one locked step, so a concurrent writer may save between this
        // load and save; the merge keeps what each save added
        for i in 0..n {
            for j in 0..5 {
                assert!(
                    back.selftest
                        .selftest_used
                        .contains_key(&format!("nonce-{i}-{j}")),
                    "nonce {i}-{j}"
                );
                assert!(
                    back.liveness
                        .deferred
                        .iter()
                        .any(|e| e.body == format!("e-{i}-{j}")),
                    "event {i}-{j}"
                );
            }
        }
    }

    fn posted(id: u64, at: i64) -> PostedNote {
        PostedNote {
            id,
            kind: NoteKind::Close,
            at,
        }
    }

    #[test]
    fn the_note_kinds_keep_their_names_in_the_state_file() {
        let all = [
            NoteKind::Close,
            NoteKind::VerifyPass,
            NoteKind::VerifyFail,
            NoteKind::Assignment,
            NoteKind::Reassign,
            NoteKind::Creation,
        ];
        for k in all {
            let text = serde_json::to_string(&k).unwrap();
            assert_eq!(text, format!("\"{}\"", k.as_str()));
            assert_eq!(serde_json::from_str::<NoteKind>(&text).unwrap(), k);
        }
        // a name of a later build loads as Other with its name, so the file is not lost and a
        // save writes the name back as it was
        let pin = serde_json::from_str::<NoteKind>("\"pin\"").unwrap();
        assert_eq!(pin, NoteKind::Other("pin".into()));
        assert_eq!(serde_json::to_string(&pin).unwrap(), "\"pin\"");
    }

    #[test]
    fn the_note_memory_keeps_its_bound() {
        let now = 2_000_000_000;
        let mut n = NoteState::default();
        for id in 0..(MAX_NOTES_PER_TICKET as u64 + 5) {
            n.record("o/r#1", posted(id, now), now);
        }
        let ids: Vec<u64> = n.of("o/r#1").iter().map(|x| x.id).collect();
        assert_eq!(ids.len(), MAX_NOTES_PER_TICKET);
        assert_eq!(ids[0], 5, "the oldest notes go first");
        // a ticket whose newest note is too old is forgotten, with its recheck
        n.record(
            "o/r#2",
            posted(900, now - NOTE_KEEP_SECS - 1),
            now - NOTE_KEEP_SECS - 1,
        );
        n.mark("o/r#2", now);
        n.bound(now);
        assert!(n.of("o/r#2").is_empty() && !n.notes_recheck.contains_key("o/r#2"));
        // above the ticket cap, the ticket with the oldest newest note goes first
        let mut m = NoteState::default();
        for t in 0..(MAX_NOTE_TICKETS as i64 + 2) {
            m.record(
                &format!("o/r#{}", t + 10),
                posted(t as u64, now - 1000 + t),
                now,
            );
        }
        assert_eq!(m.posted_notes.len(), MAX_NOTE_TICKETS);
        assert!(m.of("o/r#10").is_empty() && m.of("o/r#11").is_empty());
        assert!(!m.of("o/r#12").is_empty());
        // control: forget removes one note, and the empty ticket with it
        m.forget("o/r#12", 2);
        assert!(!m.posted_notes.contains_key("o/r#12"));
    }

    #[test]
    fn a_save_merges_the_recheck_marks_and_a_mark_that_this_copy_cleared_stays_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        let now = crate::clock::now_epoch();
        let mut first = StateCache::default();
        for t in 1..=3 {
            first
                .notes
                .record(&format!("o/r#{t}"), posted(t * 10, now), now);
        }
        first.notes.mark("o/r#1", 100);
        first.notes.mark("o/r#3", 100);
        first.save(&p).unwrap();
        // a long `watch` and a ticket command load the file
        let mut watch = StateCache::load(&p);
        let mut cmd = StateCache::load(&p);
        // the command marks ticket 2 (its read failed) and clears ticket 3
        cmd.notes.mark("o/r#2", 200);
        cmd.notes.notes_recheck.remove("o/r#3");
        cmd.save(&p).unwrap();
        // the watch read ticket 1 and cleared its mark on purpose; it did not touch ticket 3
        watch.notes.notes_recheck.remove("o/r#1");
        watch.save(&p).unwrap();
        let back = StateCache::load(&p);
        assert_eq!(
            back.notes.notes_recheck,
            [("o/r#2".to_string(), 200)].into(),
            "the concurrent mark stays, both clears stay"
        );
        // a copy that moves a mark after its read keeps it, although the file cleared it
        let mut stale = StateCache::load(&p);
        let mut other = StateCache::load(&p);
        other.notes.notes_recheck.remove("o/r#2");
        other.save(&p).unwrap();
        stale.notes.mark("o/r#2", 300);
        stale.save(&p).unwrap();
        assert_eq!(
            StateCache::load(&p).notes.notes_recheck,
            [("o/r#2".to_string(), 300)].into()
        );
        // control: a refresh brings a concurrent mark into a copy in memory
        let mut mem = StateCache::load(&p);
        let mut w = StateCache::load(&p);
        w.notes.mark("o/r#1", 400);
        w.save(&p).unwrap();
        mem.refresh_notes(&p);
        assert_eq!(mem.notes.notes_recheck.get("o/r#1"), Some(&400));
    }

    #[test]
    fn a_mark_that_a_long_watch_moved_and_saved_does_not_undo_a_later_clear_of_another_writer() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        let now = crate::clock::now_epoch();
        let mut first = StateCache::default();
        first.notes.record("o/r#1", posted(10, now), now);
        first.notes.mark("o/r#1", 100);
        first.save(&p).unwrap();
        // a long `watch` moves the mark (a read that keeps it) and saves
        let mut watch = StateCache::load(&p);
        watch.notes.mark("o/r#1", 200);
        watch.save(&p).unwrap();
        // a ticket command clears the mark afterwards
        let mut cmd = StateCache::load(&p);
        cmd.notes.notes_recheck.remove("o/r#1");
        cmd.save(&p).unwrap();
        // the watch saves again without a change to the mark: the clear stays
        watch.save(&p).unwrap();
        assert!(
            StateCache::load(&p).notes.notes_recheck.is_empty(),
            "the moved mark came back"
        );
        // the same for the too-large memory
        let mut watch = StateCache::load(&p);
        watch.notes.notes_too_large.insert("o/r#1".into(), now);
        watch.save(&p).unwrap();
        let mut cmd = StateCache::load(&p);
        assert_eq!(cmd.notes.notes_too_large.get("o/r#1"), Some(&now));
        cmd.notes.notes_too_large.remove("o/r#1");
        cmd.save(&p).unwrap();
        watch.save(&p).unwrap();
        assert!(StateCache::load(&p).notes.notes_too_large.is_empty());
        // control: a mark that the watch moves after its save stays
        watch.notes.mark("o/r#1", 400);
        watch.save(&p).unwrap();
        assert_eq!(
            StateCache::load(&p).notes.notes_recheck.get("o/r#1"),
            Some(&400)
        );
    }

    #[test]
    fn the_recheck_marks_of_an_earlier_build_load_as_the_oldest_marks() {
        // an earlier build wrote the marks as a list; a parse error would empty the whole file
        let old = r#"{"posted_notes":{"o/r#1":[{"id":10,"kind":"close","at":5}]},"notes_recheck":["o/r#1"]}"#;
        let s: StateCache = serde_json::from_str(old).unwrap();
        assert_eq!(s.notes.notes_recheck, [("o/r#1".to_string(), 0)].into());
        assert_eq!(s.notes.ids(), [10].into());
        // the current form is a map with the time of the mark, and it round-trips
        let mut n = StateCache::default();
        n.notes.record("o/r#2", posted(20, 7), 7);
        n.notes.mark("o/r#2", 1234);
        let text = serde_json::to_string(&n).unwrap();
        assert!(text.contains(r#""notes_recheck":{"o/r#2":1234}"#), "{text}");
        let back: StateCache = serde_json::from_str(&text).unwrap();
        assert_eq!(back.notes.notes_recheck, n.notes.notes_recheck);
    }

    #[test]
    fn a_save_keeps_the_notes_of_a_concurrent_writer_and_what_this_copy_forgot_stays_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        let now = crate::clock::now_epoch();
        let mut first = StateCache::default();
        first.notes.record("o/r#1", posted(10, now), now);
        first.notes.record("o/r#1", posted(11, now), now);
        first.save(&p).unwrap();
        // a long `watch` loaded the file
        let mut watch = StateCache::load(&p);
        // a ticket command records a new note meanwhile
        let mut cmd = StateCache::load(&p);
        cmd.notes.record("o/r#2", posted(20, now), now);
        cmd.save(&p).unwrap();
        // the watch reported note 10 and forgets it, then saves its older copy
        watch.notes.forget("o/r#1", 10);
        watch.save(&p).unwrap();
        let back = StateCache::load(&p);
        assert_eq!(back.notes.ids(), [11, 20].into());
        // a refresh brings the concurrent note into the copy in memory
        let mut stale = StateCache::default();
        stale.refresh_notes(&p);
        assert_eq!(stale.notes.ids(), [11, 20].into());
        // a copy that forgot 11 after its refresh does not get it back from the file
        stale.notes.forget("o/r#1", 11);
        stale.refresh_notes(&p);
        assert_eq!(stale.notes.ids(), [20].into());
        stale.save(&p).unwrap();
        assert_eq!(StateCache::load(&p).notes.ids(), [20].into());
    }
}
