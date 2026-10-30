/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! Local archive names and the retention ladder for the embedded-database backup. The host takes the
//! backup; this module decides which names to rename and which to delete, and what a remote mirror
//! should upload. Neither function talks to Postgres or to a network.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use jiff::civil::{Date, Time};
use jiff::tz::TimeZone;
use jiff::Timestamp;
use moqui_lib::error::Error;

/// `dump` is one `pg_dump` file. `incremental` is a `pg_basebackup`, a full whenever the generation
/// or `summarize_wal` says so, otherwise a block incremental.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupMode {
    Dump,
    Incremental,
}

/// What one run of the backup service asks the host to do. Counts are how many archives of that tier
/// to keep. `yearly: None` does not cap yearlies.
#[derive(Clone, Debug)]
pub struct BackupRequest {
    pub mode: BackupMode,
    /// Empty uses `runtime/db/backup`.
    pub backup_dir: Option<PathBuf>,
    pub daily: u32,
    pub weekly: u32,
    pub monthly: u32,
    pub yearly: Option<u32>,
    /// Incrementals in one generation before the next one is a full. The full counts as one.
    pub full_every: u32,
    /// `pg_basebackup --checkpoint=fast`: the backup starts after an immediate checkpoint, not the server's spread one,
    /// which can take minutes on an idle server. Off by default, so a scheduled backup does not add an I/O burst.
    pub fast_checkpoint: bool,
    pub timeout: Duration,
}

/// One archive that should exist locally after the run, name and path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeptArchive {
    pub name: String,
    pub path: PathBuf,
}

/// What the host produced. `errors` are clusters that failed; the others still published.
#[derive(Clone, Debug)]
pub struct BackupOutcome {
    pub skipped: bool,
    pub backup_dir: PathBuf,
    pub archive_path: PathBuf,
    pub backup_kind: String,
    pub bytes_written: u64,
    pub promoted: Vec<String>,
    pub removed: Vec<String>,
    pub keep: Vec<KeptArchive>,
    /// Net renames from this run, old name to new name, for the remote mirror.
    pub renames: Vec<(String, String)>,
    pub errors: Vec<String>,
}

/// The host's backup. Core calls it and does not start a postmaster itself.
pub type BackupRunner = Arc<dyn Fn(&BackupRequest) -> Result<BackupOutcome, Error> + Send + Sync>;

/// The facade's slot of the installed runner.
#[derive(Default)]
struct RunnerSlot(Option<BackupRunner>);

/// Install the host's embedded-database backup. Absent when this process has no embedded cluster, and the backup
/// service then reports `skipped`.
pub fn set_runner(facade: &crate::entity::EntityFacade, runner: Option<BackupRunner>) {
    facade.with_slot(RunnerSlot::default, |slot| slot.0 = runner);
}

/// Clone of the installed runner. The slot is not held across the backup.
#[must_use]
pub fn runner(facade: &crate::entity::EntityFacade) -> Option<BackupRunner> {
    facade.with_slot_read(RunnerSlot::default, |slot| slot.0.clone())
}

/// Tier letter in the archive name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Tier {
    fn letter(self) -> char {
        match self {
            Self::Daily => 'd',
            Self::Weekly => 'w',
            Self::Monthly => 'm',
            Self::Yearly => 'y',
        }
    }

    fn next(self) -> Option<Self> {
        match self {
            Self::Daily => Some(Self::Weekly),
            Self::Weekly => Some(Self::Monthly),
            Self::Monthly => Some(Self::Yearly),
            Self::Yearly => None,
        }
    }

    fn parse(byte: u8) -> Option<Self> {
        match byte {
            b'd' => Some(Self::Daily),
            b'w' => Some(Self::Weekly),
            b'm' => Some(Self::Monthly),
            b'y' => Some(Self::Yearly),
            _ => None,
        }
    }
}

/// An archive name the ladder understands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedName {
    pub prefix: String,
    pub stamp: Timestamp,
    pub tier: Tier,
    pub dump: bool,
}

/// One archive on disk. `parent` is the archive name an incremental was taken from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveIn {
    pub name: String,
    pub parent: Option<String>,
}

/// Renames of archives that stay, deletes of the names still on disk, and sidecar parent rewrites
/// for kept incrementals whose parent was renamed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetainPlan {
    pub renames: Vec<(String, String)>,
    pub deletes: Vec<String>,
    pub parent_updates: Vec<(String, String)>,
}

/// What the mirror should do, in that order: rename, upload, then delete.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MirrorPlan {
    pub renames: Vec<(String, String)>,
    pub uploads: Vec<String>,
    pub deletes: Vec<String>,
}

/// `moqui-YYYYMMDDTHHMMSSZ-d.dump` or the same stem without `.dump` for a physical backup directory.
/// The prefix is `moqui` or `search`.
#[must_use]
pub fn parse_archive_name(name: &str) -> Option<ParsedName> {
    let (prefix, rest) = name.split_once('-')?;
    if prefix != "moqui" && prefix != "search" {
        return None;
    }
    let (stamp_text, rest) = rest.split_once('-')?;
    let stamp = parse_stamp(stamp_text)?;
    let (tier_text, dump) = match rest.strip_suffix(".dump") {
        Some(tier) => (tier, true),
        None => (rest, false),
    };
    let bytes = tier_text.as_bytes();
    if bytes.len() != 1 {
        return None;
    }
    Some(ParsedName {
        prefix: prefix.to_string(),
        stamp,
        tier: Tier::parse(bytes[0])?,
        dump,
    })
}

/// The file or directory name for one archive.
#[must_use]
pub fn archive_name(prefix: &str, stamp: Timestamp, tier: Tier, dump: bool) -> String {
    let ext = if dump { ".dump" } else { "" };
    format!("{prefix}-{}-{}{ext}", format_stamp(stamp), tier.letter())
}

#[must_use]
pub fn is_archive_name(name: &str) -> bool {
    parse_archive_name(name).is_some()
}

/// Promote completed periods, then cap each tier. `now` is the backup just taken: its week, month,
/// and year are not promoted. `zone` is the zone the cron schedule uses. A kept incremental keeps
/// its parent chain, and those parents do not have to fit the cap.
#[must_use]
pub fn retain(archives: &[ArchiveIn], now: Timestamp, zone: &TimeZone, counts: &BackupRequest) -> RetainPlan {
    let mut items: Vec<Item> = archives.iter().filter_map(Item::from_archive).collect();
    let prefixes: Vec<String> = {
        let mut seen = Vec::new();
        for item in &items {
            if !seen.contains(&item.prefix) {
                seen.push(item.prefix.clone());
            }
        }
        seen
    };
    for prefix in prefixes {
        let indexes: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.prefix == prefix)
            .map(|(index, _)| index)
            .collect();
        promote(&mut items, &indexes, now, zone);
    }
    let mut delete = HashSet::new();
    for tier in [Tier::Daily, Tier::Weekly, Tier::Monthly] {
        let count = match tier {
            Tier::Daily => counts.daily,
            Tier::Weekly => counts.weekly,
            Tier::Monthly => counts.monthly,
            Tier::Yearly => 0,
        };
        cap_into(&items, tier, count, &mut delete);
    }
    if let Some(count) = counts.yearly {
        cap_into(&items, Tier::Yearly, count, &mut delete);
    }
    rescue(&items, &mut delete);
    let mut plan = RetainPlan::default();
    for item in &items {
        if delete.contains(&item.name) {
            plan.deletes.push(item.original_name.clone());
            continue;
        }
        if item.original_name != item.name {
            plan.renames.push((item.original_name.clone(), item.name.clone()));
        }
        if item.parent != item.original_parent {
            if let Some(parent) = &item.parent {
                plan.parent_updates.push((item.name.clone(), parent.clone()));
            }
        }
    }
    plan.renames.sort();
    plan.deletes.sort();
    plan.parent_updates.sort();
    plan
}

/// Remote names against the local keep set. `renames` are this run's promotions. A remote name that
/// is not an archive is left in place.
#[must_use]
pub fn mirror_plan(
    keep: &[String],
    remote: &[String],
    renames: &[(String, String)],
    is_archive: impl Fn(&str) -> bool,
) -> MirrorPlan {
    let keep_set: HashSet<&str> = keep.iter().map(String::as_str).collect();
    let mut remote_set: HashSet<String> = remote.iter().cloned().collect();
    let mut plan = MirrorPlan::default();
    for (old, new) in renames {
        if remote_set.contains(old) && keep_set.contains(new.as_str()) && !remote_set.contains(new) {
            plan.renames.push((old.clone(), new.clone()));
            remote_set.remove(old);
            remote_set.insert(new.clone());
        }
    }
    let mut uploads: Vec<String> = keep
        .iter()
        .filter(|name| !remote_set.contains(*name))
        .cloned()
        .collect();
    uploads.sort();
    plan.uploads = uploads;
    let mut deletes: Vec<String> = remote_set
        .into_iter()
        .filter(|name| is_archive(name) && !keep_set.contains(name.as_str()))
        .collect();
    deletes.sort();
    plan.deletes = deletes;
    plan
}

struct Item {
    original_name: String,
    name: String,
    prefix: String,
    stamp: Timestamp,
    tier: Tier,
    dump: bool,
    parent: Option<String>,
    original_parent: Option<String>,
}

impl Item {
    fn from_archive(archive: &ArchiveIn) -> Option<Self> {
        let parsed = parse_archive_name(&archive.name)?;
        Some(Self {
            original_name: archive.name.clone(),
            name: archive.name.clone(),
            prefix: parsed.prefix,
            stamp: parsed.stamp,
            tier: parsed.tier,
            dump: parsed.dump,
            parent: archive.parent.clone(),
            original_parent: archive.parent.clone(),
        })
    }
}

#[derive(Clone, Copy)]
enum Period {
    Week,
    Month,
    Year,
}

fn promote(items: &mut [Item], indexes: &[usize], now: Timestamp, zone: &TimeZone) {
    promote_tier(items, indexes, now, zone, Period::Week, Tier::Daily);
    promote_tier(items, indexes, now, zone, Period::Month, Tier::Weekly);
    promote_tier(items, indexes, now, zone, Period::Year, Tier::Monthly);
}

fn promote_tier(items: &mut [Item], indexes: &[usize], now: Timestamp, zone: &TimeZone, period: Period, from: Tier) {
    let now_key = period_key(now, zone, period);
    let mut periods = Vec::new();
    for index in indexes {
        let item = &items[*index];
        if item.tier != from {
            continue;
        }
        let key = period_key(item.stamp, zone, period);
        if key < now_key && !periods.contains(&key) {
            periods.push(key);
        }
    }
    for key in periods {
        let occupied = indexes.iter().any(|index| {
            let item = &items[*index];
            item.tier > from && period_key(item.stamp, zone, period) == key
        });
        if occupied {
            continue;
        }
        let mut best: Option<usize> = None;
        for index in indexes {
            let stamp = items[*index].stamp;
            if items[*index].tier == from && period_key(stamp, zone, period) == key {
                let newer = best.is_none_or(|known| stamp > items[known].stamp);
                if newer {
                    best = Some(*index);
                }
            }
        }
        if let Some(index) = best {
            bump(items, index);
        }
    }
}

fn bump(items: &mut [Item], index: usize) {
    let Some(next) = items[index].tier.next() else {
        return;
    };
    let old = items[index].name.clone();
    items[index].tier = next;
    items[index].name = archive_name(&items[index].prefix, items[index].stamp, next, items[index].dump);
    let new = items[index].name.clone();
    for item in items.iter_mut() {
        if item.parent.as_deref() == Some(old.as_str()) {
            item.parent = Some(new.clone());
        }
    }
}

fn cap_into(items: &[Item], tier: Tier, count: u32, delete: &mut HashSet<String>) {
    let mut group: Vec<&Item> = items.iter().filter(|item| item.tier == tier).collect();
    group.sort_by(|left, right| right.stamp.cmp(&left.stamp).then(right.name.cmp(&left.name)));
    for item in group.into_iter().skip(count as usize) {
        delete.insert(item.name.clone());
    }
}

fn rescue(items: &[Item], delete: &mut HashSet<String>) {
    let mut seen = HashSet::new();
    let roots: Vec<String> = items
        .iter()
        .filter(|item| !delete.contains(&item.name))
        .map(|item| item.name.clone())
        .collect();
    for name in roots {
        rescue_one(items, delete, &name, &mut seen);
    }
}

fn rescue_one(items: &[Item], delete: &mut HashSet<String>, name: &str, seen: &mut HashSet<String>) {
    if !seen.insert(name.to_string()) {
        return;
    }
    let Some(parent) = items
        .iter()
        .find(|item| item.name == name)
        .and_then(|item| item.parent.clone())
    else {
        return;
    };
    delete.remove(&parent);
    rescue_one(items, delete, &parent, seen);
}

fn period_key(stamp: Timestamp, zone: &TimeZone, period: Period) -> (i16, i16) {
    let date = stamp.to_zoned(zone.clone()).date();
    match period {
        Period::Week => {
            let week = date.iso_week_date();
            (week.year(), i16::from(week.week()))
        }
        Period::Month => (date.year(), i16::from(date.month())),
        Period::Year => (date.year(), 0),
    }
}

fn format_stamp(stamp: Timestamp) -> String {
    let zoned = stamp.to_zoned(TimeZone::UTC);
    let date = zoned.date();
    let time = zoned.time();
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        date.year(),
        date.month(),
        date.day(),
        time.hour(),
        time.minute(),
        time.second()
    )
}

fn parse_stamp(text: &str) -> Option<Timestamp> {
    let bytes = text.as_bytes();
    if bytes.len() != 16 || bytes[8] != b'T' || bytes[15] != b'Z' {
        return None;
    }
    let year: i16 = text[0..4].parse().ok()?;
    let month: i8 = text[4..6].parse().ok()?;
    let day: i8 = text[6..8].parse().ok()?;
    let hour: i8 = text[9..11].parse().ok()?;
    let minute: i8 = text[11..13].parse().ok()?;
    let second: i8 = text[13..15].parse().ok()?;
    let date = Date::new(year, month, day).ok()?;
    let time = Time::new(hour, minute, second, 0).ok()?;
    Some(date.to_datetime(time).to_zoned(TimeZone::UTC).ok()?.timestamp())
}

#[cfg(test)]
#[path = "backup_tests.rs"]
mod tests;
