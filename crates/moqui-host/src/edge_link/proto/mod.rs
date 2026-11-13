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

//! The edge link's gRPC messages and services (`moqui.edge.v1`), generated from `crates/moqui-host/proto/edge.proto` by
//! tonic-prost-build on protox and checked in (plans/archive/Edge-Node.md §5.6). The test beside it fails when the checked-in
//! code is not what the proto generates; `MOQUI_REGENERATE_PROTO=1` rewrites it. Changes to the proto are additive only.

// generated code: its lints are tonic's and prost's to keep
#![allow(clippy::all, clippy::pedantic, missing_docs, unused_qualifications)]

include!("moqui.edge.v1.rs");

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
