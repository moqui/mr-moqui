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

use super::*;

fn stamp(text: &str) -> Timestamp {
    parse_stamp(text).unwrap()
}

fn counts(daily: u32, weekly: u32, monthly: u32, yearly: Option<u32>) -> BackupRequest {
    BackupRequest {
        mode: BackupMode::Dump,
        backup_dir: None,
        daily,
        weekly,
        monthly,
        yearly,
        full_every: 7,
        fast_checkpoint: false,
        timeout: Duration::from_secs(60),
    }
}

fn named(name: &str) -> ArchiveIn {
    ArchiveIn {
        name: name.to_string(),
        parent: None,
    }
}

#[test]
fn a_previous_week_promotes_its_newest_daily_and_the_cap_drops_the_older_one() {
    let now = stamp("20260318T020000Z");
    let archives = [
        named("moqui-20260309T020000Z-d.dump"),
        named("moqui-20260311T020000Z-d.dump"),
        named("moqui-20260318T020000Z-d.dump"),
    ];
    let plan = retain(&archives, now, &TimeZone::UTC, &counts(1, 5, 12, None));
    // 2026-03-11 is the newer daily of that ISO week, so it becomes the weekly.
    assert_eq!(
        plan.renames,
        vec![(
            "moqui-20260311T020000Z-d.dump".to_string(),
            "moqui-20260311T020000Z-w.dump".to_string()
        )]
    );
    assert_eq!(plan.deletes, vec!["moqui-20260309T020000Z-d.dump".to_string()]);
    assert!(plan.parent_updates.is_empty());
}

#[test]
fn the_same_week_does_not_promote() {
    let now = stamp("20260318T030000Z");
    let archives = [
        named("moqui-20260316T020000Z-d.dump"),
        named("moqui-20260318T030000Z-d.dump"),
    ];
    let plan = retain(&archives, now, &TimeZone::UTC, &counts(1, 5, 12, None));
    assert!(plan.renames.is_empty());
    assert_eq!(plan.deletes, vec!["moqui-20260316T020000Z-d.dump".to_string()]);
}

#[test]
fn a_daily_in_a_finished_month_climbs_to_monthly_in_one_pass() {
    let now = stamp("20260318T020000Z");
    let archives = [named("moqui-20260115T020000Z-d.dump")];
    let plan = retain(&archives, now, &TimeZone::UTC, &counts(7, 5, 12, None));
    assert_eq!(
        plan.renames,
        vec![(
            "moqui-20260115T020000Z-d.dump".to_string(),
            "moqui-20260115T020000Z-m.dump".to_string()
        )]
    );
    assert!(plan.deletes.is_empty());
}

#[test]
fn an_empty_yearly_count_deletes_no_yearly() {
    let now = stamp("20260318T020000Z");
    let mut archives: Vec<ArchiveIn> = (2010..2020)
        .map(|year| named(&format!("moqui-{year}0101T020000Z-y.dump")))
        .collect();
    archives.push(named("moqui-20260318T020000Z-d.dump"));
    let plan = retain(&archives, now, &TimeZone::UTC, &counts(7, 5, 12, None));
    assert!(plan.deletes.iter().all(|name| !name.contains("-y.")));
}

#[test]
fn a_kept_incremental_keeps_a_parent_the_caps_would_drop() {
    let now = stamp("20260318T020000Z");
    let full = "moqui-20240102T020000Z-d";
    let incremental = "moqui-20260318T020000Z-d";
    let archives = [
        named(full),
        ArchiveIn {
            name: incremental.to_string(),
            parent: Some(full.to_string()),
        },
    ];
    let plan = retain(&archives, now, &TimeZone::UTC, &counts(1, 0, 0, Some(0)));
    assert!(plan.deletes.iter().all(|name| name != full), "parent deleted: {plan:?}");
    assert!(plan.renames.iter().any(|(old, new)| old == full && new.ends_with("-y")));
    let new_parent = plan
        .renames
        .iter()
        .find(|(old, _)| old == full)
        .map(|(_, new)| new.as_str())
        .unwrap();
    assert_eq!(
        plan.parent_updates,
        vec![(incremental.to_string(), new_parent.to_string())]
    );
}

#[test]
fn the_mirror_renames_then_uploads_then_drops_archives_it_does_not_own() {
    let keep = vec![
        "moqui-20260318T020000Z-d.dump".to_string(),
        "moqui-20260311T020000Z-w.dump".to_string(),
    ];
    let remote = vec![
        "moqui-20260311T020000Z-d.dump".to_string(),
        "moqui-20200101T000000Z-d.dump".to_string(),
        "notes.txt".to_string(),
    ];
    let renames = vec![(
        "moqui-20260311T020000Z-d.dump".to_string(),
        "moqui-20260311T020000Z-w.dump".to_string(),
    )];
    let plan = mirror_plan(&keep, &remote, &renames, is_archive_name);
    assert_eq!(plan.renames, renames);
    assert_eq!(plan.uploads, vec!["moqui-20260318T020000Z-d.dump".to_string()]);
    assert_eq!(plan.deletes, vec!["moqui-20200101T000000Z-d.dump".to_string()]);
}

#[test]
fn a_remote_name_already_promoted_is_not_renamed_again() {
    let keep = vec!["moqui-20260311T020000Z-w.dump".to_string()];
    let remote = vec![
        "moqui-20260311T020000Z-d.dump".to_string(),
        "moqui-20260311T020000Z-w.dump".to_string(),
    ];
    let renames = vec![(
        "moqui-20260311T020000Z-d.dump".to_string(),
        "moqui-20260311T020000Z-w.dump".to_string(),
    )];
    let plan = mirror_plan(&keep, &remote, &renames, is_archive_name);
    assert!(plan.renames.is_empty());
    assert!(plan.uploads.is_empty());
    assert_eq!(plan.deletes, vec!["moqui-20260311T020000Z-d.dump".to_string()]);
}

#[test]
fn archive_names_parse_their_prefix_stamp_and_tier_and_reject_the_rest() {
    let parsed = parse_archive_name("search-20260311T020000Z-w").unwrap();
    assert_eq!(parsed.prefix, "search");
    assert_eq!(parsed.stamp, stamp("20260311T020000Z"));
    assert_eq!(parsed.tier, Tier::Weekly);
    assert!(!parsed.dump);
    assert!(parse_archive_name("moqui-20260311T020000Z-d.dump").unwrap().dump);
    for name in [
        "other-20260311T020000Z-d.dump",
        "moqui-20260311T020000Z-dw.dump",
        "moqui-20260311T020000Z-.dump",
        "moqui-2026031T020000Z-d.dump",
        "moqui-20261311T020000Z-d.dump",
        // 16 bytes, with a two-byte character across a field boundary
        "moqui-202\u{e9}010T123456Z-d.dump",
        "moqui-20260311T1234\u{e9}Z-d.dump",
    ] {
        assert!(parse_archive_name(name).is_none(), "{name}");
    }
}
