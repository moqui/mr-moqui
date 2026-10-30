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

//! Commits, tags and people as the maps scripts see: the out parameters of the `get#Git*` services and the context of
//! gitECA rules.

use indexmap::IndexMap;
use moqui_lib::object::Value;

use super::ops::{CommitDetails, Signature, TagDetails};

pub(crate) fn map(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    )
}

pub(crate) fn opt_text(text: &str) -> Value {
    if text.is_empty() {
        Value::Null
    } else {
        Value::string(text)
    }
}

pub(crate) fn timestamp(seconds: i64) -> Value {
    jiff::Timestamp::from_second(seconds).map_or(Value::Null, Value::Timestamp)
}

fn signature_into(entries: &mut Vec<(&'static str, Value)>, names: [&'static str; 3], signature: &Signature) {
    entries.push((names[0], opt_text(&signature.name)));
    entries.push((names[1], opt_text(&signature.email)));
    entries.push((names[2], timestamp(signature.seconds)));
}

/// A commit as a map: ids, people and dates, the message, its summary and body, its trailers by name (each a list of
/// values), and the Moqui user of a commit made through `git://`.
pub(crate) fn commit_value(commit: &CommitDetails) -> Value {
    let id = commit.id.to_string();
    let summary = commit.summary().to_string();
    let body = commit
        .message
        .trim()
        .split_once('\n')
        .map_or("", |(_, rest)| rest)
        .trim()
        .to_string();
    let mut trailers: IndexMap<String, Value> = IndexMap::new();
    for (name, value) in commit.trailers() {
        if let Value::List(values) = trailers.entry(name).or_insert_with(|| Value::List(Vec::new())) {
            values.push(Value::string(value));
        }
    }
    let mut entries = vec![
        ("commitId", Value::string(&id)),
        ("shortId", Value::string(commit.id.to_hex_with_len(10).to_string())),
        ("treeId", Value::string(commit.tree.to_string())),
        (
            "parentIds",
            Value::List(
                commit
                    .parents
                    .iter()
                    .map(|parent| Value::string(parent.to_string()))
                    .collect(),
            ),
        ),
    ];
    signature_into(
        &mut entries,
        ["authorName", "authorEmail", "authorDate"],
        &commit.author,
    );
    signature_into(
        &mut entries,
        ["committerName", "committerEmail", "committerDate"],
        &commit.committer,
    );
    entries.extend([
        ("summary", Value::string(summary)),
        ("body", opt_text(&body)),
        ("message", Value::string(&commit.message)),
        ("trailers", Value::Map(trailers)),
        (
            "moquiUserId",
            crate::ops::user_of(&commit.message).map_or(Value::Null, Value::string),
        ),
    ]);
    map(entries)
}

pub(crate) fn tag_value(tag: &TagDetails) -> Value {
    let mut entries = vec![
        ("tagName", Value::string(&tag.name)),
        ("targetId", Value::string(tag.target.to_string())),
        ("targetType", Value::string(tag.target_kind.to_string())),
        ("message", Value::string(tag.message.trim_end())),
    ];
    if let Some(tagger) = &tag.tagger {
        signature_into(&mut entries, ["taggerName", "taggerEmail", "taggerDate"], tagger);
    }
    map(entries)
}
