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

//! `org.moqui.maps.GeoDataServices` (plans/Maps.md §6.4): `import#GeoData` (a run, done here: a ServiceJob runs it in
//! the background), `cancel#GeoDataImport` (the flag a run reads between batches) and `remove#GeoData`. They need the
//! MAPS_ADMIN authorization (the artifact group names the services), checked again here as the user with
//! authorization on, also in a ServiceJob, whose call is made with it off; the rows are then written as the
//! framework. A refusal before a run starts is a message-facade error; a run that fails is a `GdiFailed` run with its
//! `failureText`, not an error.
//!
//! A run's scope is its source key (every data set sharing it: a mirror, a later year) and its country: the check for a
//! run in progress and the edition in use are both of that scope. A source key's sets must share one format.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::geo::{self, Format, RowError};
use moqui_lib::object::{to_plain_string, Value};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::{Condition, EntityFacade, EntityValue};
use moqui_core::transaction::TransactionAttribute;

use super::fetch::{self, Conditional, Fetch};
use super::load::{self, Country, Counts, DataSet, Loader, Rejections, POINT_ASSOC, POSTAL_CODE};
use super::remove;
use super::{
    in_progress, CANCELLED, COMPLETED, DATA_SET, DOWNLOADING, ERROR_ROOT, FAILED, IMPORT, LOADING, MAX_ERRORS_OUT,
    MIN_ROW_SHARE, PENDING, STALE_AFTER, UNCHANGED, VALIDATING,
};
use crate::group_db::{add_error, flag, text, Out};

const CANCEL_SERVICE: &str = "org.moqui.maps.GeoDataServices.cancel#GeoDataImport";
const REMOVE_SERVICE: &str = "org.moqui.maps.GeoDataServices.remove#GeoData";
/// How often a step with no batches of its own (reading the file, a copy) touches its run row, so a live run is
/// never taken as abandoned.
const HEARTBEAT: Duration = Duration::from_secs(60);

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, import_geo_data)?;
    rust_service!(map, cancel_geo_data_import)?;
    rust_service!(map, remove_geo_data)?;
    Ok(())
}

fn current_user() -> String {
    moqui_core::user::current_user_id().ok().flatten().unwrap_or_default()
}

fn now() -> Value {
    Value::Timestamp(jiff::Timestamp::now())
}

fn text_of(row: &EntityValue, field: &str) -> String {
    row.get(field).map(to_plain_string).unwrap_or_default()
}

/// Whether the current user holds MAPS_ADMIN's authorization of `service`, asked with authorization on (also where
/// the caller turned it off: a job, or a script that disabled it); when not, an error naming `what`. With no context
/// entered (a unit test) nothing is checked.
fn authorized(service: &str, what: &str) -> bool {
    if !ExecutionContext::is_entered() {
        return true;
    }
    let _required = ExecutionContext::with_current(ExecutionContext::require_authz);
    match moqui_core::user::authorize_own("AT_SERVICE", service, "all") {
        Ok(()) => true,
        Err(err) => {
            tracing::info!(error = %err, service, "Refused a Geo data request");
            add_error(format!(
                "{what} needs the Maps configuration authorization (MAPS_ADMIN)"
            ));
            false
        }
    }
}

/// The identity before a job user's sign-in, put back when dropped.
#[must_use = "dropping the guard signs the job user out"]
struct JobSignIn(moqui_core::user::session::Identity);

impl Drop for JobSignIn {
    fn drop(&mut self) {
        moqui_core::user::session::restore_identity(&self.0);
    }
}

/// For a ServiceJob's run with no user (a scheduled refresh of `job_name`), sign in as `run_as` (the job's
/// `runAsUserId` parameter), who must be a `ServiceJobUser` of the job and have added themselves, so a job's writer
/// cannot run an import as someone who did not agree to it; that user's own authorization is then checked as for a
/// run of theirs.
///
/// # Errors
///
/// No `runAsUserId`, that user not a self-added user of the job, or not one who may sign in.
fn sign_in_job_user(facade: &EntityFacade, job_name: &str, run_as: &str) -> Result<JobSignIn, LibError> {
    if run_as.is_empty() {
        return Err(LibError::new(format!(
            "ServiceJob {job_name} runs import#GeoData with no user signed in: give it a runAsUserId parameter, a user \
             of the job who added themselves"
        )));
    }
    let (job_user, username) = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let job_user = facade
            .find("moqui.service.job.ServiceJobUser")?
            .eq("jobName", Value::string(job_name))
            .eq("userId", Value::string(run_as))
            .one()?;
        let username = facade
            .find("moqui.security.UserAccount")?
            .eq("userId", Value::string(run_as))
            .one()?
            .map(|account| text_of(&account, "username"))
            .unwrap_or_default();
        (job_user, username)
    };
    let consented = job_user.is_some_and(|row| text_of(&row, "addedByUserId") == run_as);
    if !consented {
        return Err(LibError::new(format!(
            "runAsUserId {run_as} is not a user of ServiceJob {job_name} who added themselves"
        )));
    }
    if username.is_empty() {
        return Err(LibError::new(format!("runAsUserId {run_as} is not a user")));
    }
    let restore = JobSignIn(moqui_core::user::session::identity());
    moqui_core::user::internal_login_user(&username)
        .map_err(|err| LibError::new(format!("runAsUserId {run_as} could not sign in: {err}")))?;
    Ok(restore)
}

/// Write `fields` onto run `run_id`, in a transaction of its own; a failure is logged.
fn write_run(facade: &EntityFacade, run_id: &str, fields: Vec<(&str, Value)>) {
    let written = facade.transactions_for(IMPORT).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            let Some(mut row) = facade
                .find(IMPORT)?
                .eq("geoDataImportId", Value::string(run_id))
                .one()?
            else {
                return Err(LibError::new(format!("no Geo data import {run_id}")));
            };
            for (field, value) in fields {
                row.set(field, value);
            }
            row.update(facade)
        })
    });
    if let Err(err) = written {
        tracing::error!(error = %err, run = %run_id, "Could not write the Geo data import row");
    }
}

fn read_run(facade: &EntityFacade, run_id: &str) -> Result<Option<EntityValue>, LibError> {
    facade.find(IMPORT)?.eq("geoDataImportId", Value::string(run_id)).one()
}

/// Whether run `run_id` should stop: a cancel was asked, or its row is no longer in progress (marked abandoned).
fn stop_requested(facade: &EntityFacade, run_id: &str) -> bool {
    matches!(read_run(facade, run_id), Ok(Some(row))
        if text_of(&row, "cancelRequested") == "Y" || !in_progress(&text_of(&row, "statusId")))
}

/// The data set `data_set_id` and its format. Refused when another set of its source key has another format: their
/// rows would be taken for each other's.
fn data_set(facade: &EntityFacade, data_set_id: &str) -> Result<(EntityValue, DataSet), String> {
    let row = facade
        .find(DATA_SET)
        .and_then(|find| find.eq("dataSetId", Value::string(data_set_id)).one())
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("There is no Geo data set {data_set_id}"))?;
    if text_of(&row, "disabled") == "Y" {
        return Err(format!("Geo data set {data_set_id} is disabled"));
    }
    let format = Format::of(&text_of(&row, "parserEnumId"), &text_of(&row, "yieldsEnumId"))
        .ok_or_else(|| format!("Geo data set {data_set_id} has a parser and yield no reader handles"))?;
    let source_key = text_of(&row, "sourceKey");
    if source_key.is_empty() || source_key.len() > 12 {
        return Err(format!(
            "Geo data set {data_set_id} needs a sourceKey of at most 12 characters"
        ));
    }
    let sharing = facade
        .find(DATA_SET)
        .and_then(|find| find.eq("sourceKey", Value::string(&source_key)).list())
        .map_err(|err| err.to_string())?;
    let mut owned_set_ids = Vec::new();
    for other in sharing.iter() {
        let other_id = text_of(other, "dataSetId");
        let other_format = Format::of(&text_of(other, "parserEnumId"), &text_of(other, "yieldsEnumId"));
        if other_format != Some(format) {
            return Err(format!(
                "Geo data set {other_id} shares sourceKey {source_key} with {data_set_id} but reads another format"
            ));
        }
        owned_set_ids.push(other_id);
    }
    let set = DataSet {
        data_set_id: data_set_id.to_string(),
        data_source_id: Some(text_of(&row, "dataSourceId")).filter(|id| !id.is_empty()),
        priority: text_of(&row, "priority").parse().unwrap_or(100),
        source_key,
        format,
        owned_set_ids,
    };
    Ok((row, set))
}

/// The country of a run of `row`'s data set: its coverage, else the one asked (needed for a set read a country at a
/// time and for an operator's file, which retires only within the country it names), else none.
fn run_country(
    facade: &EntityFacade,
    row: &EntityValue,
    format: Format,
    asked: &str,
) -> Result<Option<Country>, String> {
    let data_set_id = text_of(row, "dataSetId");
    let coverage = text_of(row, "coverageGeoId");
    let wanted = if coverage.is_empty() { asked } else { coverage.as_str() };
    if !coverage.is_empty() && !asked.is_empty() && asked != coverage {
        return Err(format!("Geo data set {data_set_id} covers {coverage} only"));
    }
    if wanted.is_empty() {
        if text_of(row, "perCountry") == "Y" {
            return Err(format!(
                "Geo data set {data_set_id} is read a country at a time: name the countryGeoId"
            ));
        }
        if format == Format::OperatorCsv {
            return Err(format!(
                "Geo data set {data_set_id} is an operator's file: name the countryGeoId its rows are for"
            ));
        }
        return Ok(None);
    }
    match load::country_by_id(facade, wanted) {
        Ok(Some(country)) => Ok(Some(country)),
        Ok(None) => Err(format!("{wanted} is not a country Geo")),
        Err(err) => Err(err.to_string()),
    }
}

/// The runs of `set`'s source key in `country`'s scope: the same country, or either of them with none.
fn scope_runs(facade: &EntityFacade, set: &DataSet, country: Option<&str>) -> Result<Vec<EntityValue>, LibError> {
    let runs = load::rows_in(facade, IMPORT, "dataSetId", &set.owned_set_ids, &[])?;
    Ok(runs
        .into_iter()
        .filter(|run| {
            let run_country = text_of(run, "countryGeoId");
            country.is_none_or(|country| run_country.is_empty() || run_country == country)
        })
        .collect())
}

/// Why a new run of `set` for `country` may not start: one of its scope is in progress. A run whose row has not changed
/// for [`STALE_AFTER`] is taken as abandoned and marked failed (and stops at its next check if it was alive after all).
fn running_refusal(facade: &EntityFacade, set: &DataSet, country: Option<&str>, own: &str) -> Option<String> {
    let runs = match scope_runs(facade, set, country) {
        Ok(runs) => runs,
        Err(err) => return Some(err.to_string()),
    };
    let stale_before = jiff::Timestamp::now()
        .checked_sub(jiff::SignedDuration::try_from(STALE_AFTER).unwrap_or_default())
        .unwrap_or_else(|_| jiff::Timestamp::now());
    for run in &runs {
        let run_id = text_of(run, "geoDataImportId");
        if run_id == own || !in_progress(&text_of(run, "statusId")) {
            continue;
        }
        let touched = match run.get("lastUpdatedStamp") {
            Some(Value::Timestamp(stamp)) => Some(*stamp),
            _ => None,
        };
        if touched.is_some_and(|stamp| stamp < stale_before) {
            tracing::warn!(run = %run_id, "Marking an abandoned Geo data import failed");
            write_run(
                facade,
                &run_id,
                vec![
                    ("statusId", Value::string(FAILED)),
                    ("failureText", Value::string("Abandoned: the run stopped with no word")),
                    ("thruDate", now()),
                ],
            );
            continue;
        }
        return Some(format!(
            "Geo data import {run_id} of source {} is in progress; cancel it or wait for it to end",
            set.source_key
        ));
    }
    None
}

/// The edition in use: the last completed run of the source key and country.
fn edition_in_use(facade: &EntityFacade, set: &DataSet, country: Option<&str>) -> Option<EntityValue> {
    let mut completed: Vec<EntityValue> = scope_runs(facade, set, country)
        .ok()?
        .into_iter()
        .filter(|run| text_of(run, "statusId") == COMPLETED)
        .filter(|run| text_of(run, "countryGeoId") == country.unwrap_or_default())
        .collect();
    completed.sort_by_key(|run| match run.get("thruDate") {
        Some(Value::Timestamp(stamp)) => Some(*stamp),
        _ => None,
    });
    completed.pop()
}

/// Whether the source key has rows in date in the run's scope. When it has none (a Remove, or a first run) the edition
/// in use is not: the run fetches whole and no row count holds it back.
fn owns_rows(facade: &EntityFacade, set: &DataSet, country: Option<&Country>) -> bool {
    let scoped = |find: moqui_core::entity::EntityFind| match country {
        Some(country) => find.condition(Condition::Like(
            "geoId".into(),
            Value::string(format!("{}_%", country.geo_id)),
        )),
        None => find,
    };
    let codes = facade.find(POSTAL_CODE).and_then(|find| {
        scoped(find.eq("sourceKey", Value::string(&set.source_key)))
            .condition(Condition::is_null("thruDate"))
            .limit(1)
            .count()
    });
    if matches!(codes, Ok(count) if count > 0) {
        return true;
    }
    let points = facade.find(POINT_ASSOC).and_then(|find| {
        scoped(find.condition(Condition::In(
            "dataSetId".into(),
            set.owned_set_ids.iter().map(Value::string).collect(),
        )))
        .condition(Condition::is_null("thruDate"))
        .limit(1)
        .count()
    });
    // a query that fails is taken as rows there: the edition in use still guards the run
    !matches!(points, Ok(0)) || codes.is_err()
}

/// `maps@import-max-bytes`.
fn max_bytes() -> Result<u64, String> {
    let value =
        ExecutionContext::with_current(|ec| ec.factory().conf_setting("maps", "import-max-bytes")).unwrap_or_default();
    if value.is_empty() {
        return Ok(geo::text::Limits::default().max_bytes);
    }
    value
        .parse()
        .map_err(|_| format!("maps@import-max-bytes is not a whole number: {value}"))
}

/// Whether `maps@external-requests` allows the server's own outside requests for maps.
fn external_requests() -> bool {
    ExecutionContext::with_current(|ec| ec.factory().conf_setting("maps", "external-requests"))
        .is_none_or(|value| !value.eq_ignore_ascii_case("false"))
}

/// What a run came to.
struct Ended {
    status: &'static str,
    failure: Option<String>,
    /// The step a failed run stopped in (`errorLocation`).
    step: Option<&'static str>,
    edition: Option<String>,
    counts: Counts,
    rejected: Rejections,
}

impl Ended {
    fn failed(step: &'static str, counts: Counts, rejected: Rejections, failure: String) -> Self {
        Self {
            status: FAILED,
            failure: Some(failure),
            step: Some(step),
            edition: None,
            counts,
            rejected,
        }
    }

    fn stopped(counts: Counts, rejected: Rejections) -> Self {
        Self {
            status: CANCELLED,
            failure: None,
            step: None,
            edition: None,
            counts,
            rejected,
        }
    }
}

/// What a run needs, settled by the preflight.
struct Plan {
    run_id: String,
    row: EntityValue,
    set: DataSet,
    country: Option<Country>,
    url: String,
    location: String,
    max_bytes: u64,
    force: bool,
    in_use: Option<EntityValue>,
}

/// `import#GeoData`: import a registered data set (`dataSetId`, with `countryGeoId` for a set read a country at a time
/// or an operator's file), or run a pending run (`geoDataImportId`, as a ServiceJob does). In: `location` (an
/// operator's file), `force` (fetch again whatever the edition in use, and accept a file with far fewer rows than it),
/// `runAsUserId` (a ServiceJob's user to run as). Out: `geoDataImportId`, `statusId`, `edition`, the counts,
/// `errorFileLocation`, `errors` (the first rejected rows: `line`, `field`, `message`) and `failureText`.
///
/// Preflight, refused with an error and no run: the set registered and enabled, outside requests allowed
/// (`maps@external-requests`) for a download, the operator's file at an allowed location, the license acknowledged, no
/// run of the same source key and country in progress. A run then downloads (or copies), validates the whole file
/// writing nothing, loads in batches (reading the cancel flag between them), and only after a whole load retires what
/// the new edition no longer has.
pub(crate) fn import_geo_data(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut _job_user = None;
    if let Some(job_name) = moqui_core::service::current_job_name().filter(|_| current_user().is_empty()) {
        match sign_in_job_user(facade, &job_name, &text(parameters, "runAsUserId")) {
            Ok(signed_in) => _job_user = Some(signed_in),
            Err(err) => {
                tracing::error!(job = %job_name, error = %err, "Refused a Geo data import with no user signed in");
                add_error(err.to_string());
                return Ok(Out::new());
            }
        }
    }
    if !authorized(super::IMPORT_SERVICE, "Importing Geo data") {
        return Ok(Out::new());
    }
    // the reference rows are the framework's to write
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let plan = match preflight(parameters, facade) {
        Ok(plan) => plan,
        Err((pending, message)) => {
            add_error(message.clone());
            let mut out = Out::new();
            if let Some(run_id) = pending {
                write_run(
                    facade,
                    &run_id,
                    vec![
                        ("statusId", Value::string(FAILED)),
                        ("failureText", Value::string(&message)),
                        ("errorLocation", Value::string("preflight")),
                        ("thruDate", now()),
                    ],
                );
                out.insert("geoDataImportId".to_string(), Value::string(run_id));
                out.insert("statusId".to_string(), Value::string(FAILED));
            }
            return Ok(out);
        }
    };
    let tmp = fetch::tmp_path(&plan.run_id);
    let ended = match &tmp {
        Ok(path) => run(facade, &plan, path),
        Err(err) => Ended::failed("download", Counts::default(), Rejections::default(), err.to_string()),
    };
    if let Ok(path) = &tmp {
        let _ = std::fs::remove_file(path);
    }
    Ok(finish(facade, &plan, ended))
}

/// The preflight: the plan of a run that may start (its row made or taken), or the refusal with the pending run it
/// was for.
fn preflight(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Result<Plan, (Option<String>, String)> {
    let pending_id = text(parameters, "geoDataImportId");
    let pending = Some(pending_id.clone()).filter(|id| !id.is_empty());
    let refuse = |message: String| (pending.clone(), message);
    let (data_set_id, asked_country) = match &pending {
        Some(run_id) => {
            let run = read_run(facade, run_id).map_err(|err| refuse(err.to_string()))?;
            let Some(run) = run else {
                return Err((None, format!("There is no Geo data import {run_id}")));
            };
            if text_of(&run, "statusId") != PENDING {
                return Err((None, format!("Geo data import {run_id} is not pending")));
            }
            if text_of(&run, "cancelRequested") == "Y" {
                write_run(
                    facade,
                    run_id,
                    vec![("statusId", Value::string(CANCELLED)), ("thruDate", now())],
                );
                return Err((
                    None,
                    format!("Geo data import {run_id} was cancelled before it started"),
                ));
            }
            (text_of(&run, "dataSetId"), text_of(&run, "countryGeoId"))
        }
        None => (text(parameters, "dataSetId"), text(parameters, "countryGeoId")),
    };
    if data_set_id.is_empty() {
        return Err(refuse("Importing Geo data needs a dataSetId".to_string()));
    }
    let (row, set) = data_set(facade, &data_set_id).map_err(refuse)?;
    let country = run_country(facade, &row, set.format, &asked_country).map_err(refuse)?;
    let country_id = country.as_ref().map(|country| country.geo_id.clone());
    let url = text_of(&row, "urlTemplate");
    let location = text(parameters, "location");
    if url.is_empty() {
        if location.is_empty() {
            return Err(refuse(format!(
                "Geo data set {data_set_id} is an operator's file: give its location"
            )));
        }
        if !fetch::location_allowed(&location) {
            tracing::info!(location = %location, "Refused a Geo data file location");
            return Err(refuse(crate::NOT_AVAILABLE.to_string()));
        }
    } else {
        if !location.is_empty() {
            return Err(refuse(format!(
                "Geo data set {data_set_id} is downloaded from its URL and takes no location"
            )));
        }
        if !external_requests() {
            return Err(refuse(format!(
                "Geo data set {data_set_id} is downloaded, and maps@external-requests is false: outside requests for \
                 maps are off"
            )));
        }
    }
    if text_of(&row, "licenseAckUserId").is_empty() {
        return Err(refuse(format!(
            "The license of Geo data set {data_set_id} ({}) has not been acknowledged by an administrator",
            text_of(&row, "licenseName")
        )));
    }
    let max_bytes = max_bytes().map_err(refuse)?;
    if let Some(refusal) = running_refusal(facade, &set, country_id.as_deref(), &pending_id) {
        return Err(refuse(refusal));
    }
    let source_url = if url.is_empty() {
        String::new()
    } else {
        let code = country.as_ref().map_or("", |country| country.alpha2.as_str());
        fetch::expand_template(&url, code, &text_of(&row, "sourceYear")).map_err(refuse)?
    };
    let force = flag(parameters, "force");
    let in_use = if owns_rows(facade, &set, country.as_ref()) {
        edition_in_use(facade, &set, country_id.as_deref())
    } else {
        None
    };
    let fields = vec![
        ("dataSetId", Value::string(&data_set_id)),
        ("countryGeoId", country_id.clone().map_or(Value::Null, Value::String)),
        ("statusId", Value::string(DOWNLOADING)),
        (
            "sourceUrl",
            Some(source_url.clone())
                .filter(|url| !url.is_empty())
                .map_or(Value::Null, Value::String),
        ),
        (
            "sourceLocation",
            Some(location.clone())
                .filter(|location| !location.is_empty())
                .map_or(Value::Null, Value::String),
        ),
        ("forced", Value::string(if force { "Y" } else { "N" })),
        (
            "userId",
            Some(current_user())
                .filter(|user| !user.is_empty())
                .map_or(Value::Null, Value::String),
        ),
        (
            "jobRunId",
            moqui_core::service::current_job_run_id().map_or(Value::Null, Value::String),
        ),
        ("fromDate", now()),
        ("rowsRead", Value::I64(0)),
        ("rowsCreated", Value::I64(0)),
        ("rowsUpdated", Value::I64(0)),
        ("rowsUnchanged", Value::I64(0)),
        ("rowsRetired", Value::I64(0)),
        ("rowsRejected", Value::I64(0)),
    ];
    let run_id = match &pending {
        Some(run_id) => {
            write_run(facade, run_id, fields);
            run_id.clone()
        }
        None => {
            create_run(facade, fields).map_err(|err| refuse(format!("Could not create the Geo data import: {err}")))?
        }
    };
    Ok(Plan {
        run_id,
        row,
        set,
        country,
        url: source_url,
        location,
        max_bytes,
        force,
        in_use,
    })
}

fn create_run(facade: &EntityFacade, fields: Vec<(&str, Value)>) -> Result<String, LibError> {
    let tx = facade.transactions_for(IMPORT)?;
    tx.run(TransactionAttribute::ForceNew, None, || {
        let mut row = facade.make_value(IMPORT)?;
        row.set_sequenced_id_primary(facade)?;
        for (field, value) in fields {
            row.set(field, value);
        }
        row.create(facade)?;
        Ok(text_of(&row, "geoDataImportId"))
    })
}

/// A heartbeat of run `run_id`: touches its row at most every [`HEARTBEAT`].
fn heartbeat<'a>(facade: &'a EntityFacade, run_id: &'a str, status: &'static str) -> impl FnMut() + 'a {
    let mut last = Instant::now();
    move || {
        if last.elapsed() >= HEARTBEAT {
            last = Instant::now();
            write_run(facade, run_id, vec![("statusId", Value::string(status))]);
        }
    }
}

/// The run itself, from the fetch to the retiring.
fn run(facade: &EntityFacade, plan: &Plan, path: &std::path::Path) -> Ended {
    let mut counts = Counts::default();
    let run_id = plan.run_id.as_str();
    // the validators are of the edition in use's own URL: a mirror's would mean nothing
    let conditional = match (&plan.in_use, plan.force) {
        (Some(in_use), false) if text_of(in_use, "dataSetId") == plan.set.data_set_id => Conditional {
            etag: Some(text_of(in_use, "sourceEtag")).filter(|etag| !etag.is_empty()),
            last_modified: Some(text_of(in_use, "sourceLastModified")).filter(|text| !text.is_empty()),
        },
        _ => Conditional::default(),
    };
    let mut progress = |bytes: u64| {
        write_run(facade, run_id, vec![("byteCount", Value::I64(bytes as i64))]);
    };
    let fetched = if plan.url.is_empty() {
        fetch::copy_location(&plan.location, plan.max_bytes, path, &mut progress).map(Fetch::Fetched)
    } else {
        fetch::download(&plan.url, &conditional, plan.max_bytes, path, &mut progress)
    };
    let fetched = match fetched {
        Ok(Fetch::Fetched(fetched)) => fetched,
        Ok(Fetch::Unchanged) => {
            return Ended {
                status: UNCHANGED,
                failure: None,
                step: None,
                edition: plan.in_use.as_ref().map(|in_use| text_of(in_use, "edition")),
                counts,
                rejected: Rejections::default(),
            }
        }
        Err(err) => return Ended::failed("download", counts, Rejections::default(), err.to_string()),
    };
    let year = text_of(&plan.row, "sourceYear");
    let edition = if !year.is_empty() {
        year
    } else if let Some(last_modified) = &fetched.last_modified {
        last_modified.clone()
    } else {
        jiff::Zoned::now().date().to_string()
    };
    write_run(
        facade,
        run_id,
        vec![
            ("statusId", Value::string(VALIDATING)),
            ("byteCount", Value::I64(fetched.bytes as i64)),
            ("checksum", Value::string(&fetched.checksum)),
            ("sourceEtag", fetched.etag.clone().map_or(Value::Null, Value::String)),
            (
                "sourceLastModified",
                fetched.last_modified.clone().map_or(Value::Null, Value::String),
            ),
            ("edition", Value::string(&edition)),
        ],
    );
    if stop_requested(facade, run_id) {
        return Ended::stopped(counts, Rejections::default());
    }
    // validate: the whole file, writing nothing
    let member_template = text_of(&plan.row, "archiveMember");
    let member = if member_template.is_empty() {
        None
    } else {
        let code = plan.country.as_ref().map_or("", |country| country.alpha2.as_str());
        match fetch::expand_template(&member_template, code, &text_of(&plan.row, "sourceYear")) {
            Ok(member) => Some(member),
            Err(message) => return Ended::failed("validate", counts, Rejections::default(), message),
        }
    };
    let limits = geo::text::Limits {
        max_bytes: plan.max_bytes,
        ..geo::text::Limits::default()
    };
    let read = match load::read_file(
        path,
        member.as_deref(),
        &limits,
        plan.set.format,
        &mut heartbeat(facade, run_id, VALIDATING),
    ) {
        Ok(read) => read,
        Err(err) => {
            return Ended::failed(
                "validate",
                counts,
                Rejections::default(),
                format!("The file does not read: {err}"),
            )
        }
    };
    let mut prepared = match load::prepare(facade, &plan.set, plan.country.as_ref(), read) {
        Ok(prepared) => prepared,
        Err(err) => return Ended::failed("validate", counts, Rejections::default(), err.to_string()),
    };
    counts.read = prepared.rows;
    counts.rejected = prepared.rejected.total;
    let mut rejected = std::mem::take(&mut prepared.rejected);
    if prepared.items.is_empty() {
        return Ended::failed("validate", counts, rejected, "The file has no row to load".to_string());
    }
    let in_use_rows = plan
        .in_use
        .as_ref()
        .and_then(|in_use| text_of(in_use, "rowsRead").parse::<f64>().ok());
    if let Some(in_use_rows) = in_use_rows.filter(|_| !plan.force) {
        if (prepared.rows as f64) < in_use_rows * MIN_ROW_SHARE {
            return Ended::failed(
                "validate",
                counts,
                rejected,
                format!(
                    "The file has {} rows, far fewer than the {in_use_rows} of the edition in use; import it with \
                     force to accept it",
                    prepared.rows
                ),
            );
        }
    }
    write_run(
        facade,
        run_id,
        vec![
            ("statusId", Value::string(LOADING)),
            ("rowsRead", Value::I64(counts.read as i64)),
            ("rowsRejected", Value::I64(counts.rejected as i64)),
        ],
    );
    if stop_requested(facade, run_id) {
        return Ended::stopped(counts, rejected);
    }
    // load, a batch to a transaction, the counts written and the cancel flag read between batches
    let mut loader = Loader::new(facade, &plan.set, plan.country.as_ref(), edition.clone());
    let batches: Vec<&[load::Item]> = prepared.items.chunks(super::BATCH).collect();
    let last = batches.len();
    for (index, batch) in batches.into_iter().enumerate() {
        let result = loader.load_batch(batch);
        counts.created += result.created;
        counts.updated += result.updated;
        counts.unchanged += result.unchanged;
        counts.rejected += result.rejected.len() as u64;
        rejected.extend(result.rejected);
        write_run(
            facade,
            run_id,
            vec![
                ("rowsCreated", Value::I64(counts.created as i64)),
                ("rowsUpdated", Value::I64(counts.updated as i64)),
                ("rowsUnchanged", Value::I64(counts.unchanged as i64)),
                ("rowsRejected", Value::I64(counts.rejected as i64)),
            ],
        );
        if index + 1 < last && stop_requested(facade, run_id) {
            return Ended::stopped(counts, rejected);
        }
    }
    // finish, only after a whole load (and not for a run taken as abandoned meanwhile)
    if matches!(read_run(facade, run_id), Ok(Some(row)) if !in_progress(&text_of(&row, "statusId"))) {
        return Ended::stopped(counts, rejected);
    }
    write_run(facade, run_id, vec![("statusId", Value::string(LOADING))]);
    match loader.retire(&prepared) {
        Ok(retired) => counts.retired = retired,
        Err(err) => {
            return Ended::failed(
                "retire",
                counts,
                rejected,
                format!("The rows loaded, but retiring what the edition no longer has failed: {err}"),
            )
        }
    }
    let cleared = ExecutionContext::with_current(|ec| ec.factory().cache().clear(super::POSTAL_LOOKUP_CACHE));
    if let Some(Err(err)) = cleared {
        tracing::warn!(error = %err, "Could not clear the postal lookup cache");
    }
    Ended {
        status: COMPLETED,
        failure: None,
        step: None,
        edition: Some(edition),
        counts,
        rejected,
    }
}

/// The rejected rows as CSV (line, field, message, row), with a last line saying how many more were not kept.
fn error_csv(rejected: &Rejections) -> Option<String> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(["line", "field", "message", "row"]).ok()?;
    for error in &rejected.kept {
        writer
            .write_record([
                error.line.to_string().as_str(),
                &error.field,
                &error.message,
                &error.text,
            ])
            .ok()?;
    }
    let more = rejected.total.saturating_sub(rejected.kept.len() as u64);
    if more > 0 {
        let note = format!("{more} more rejected rows are not listed");
        writer.write_record(["", "", note.as_str(), ""]).ok()?;
    }
    writer.into_inner().ok().and_then(|bytes| String::from_utf8(bytes).ok())
}

/// The error file of a run with rejected rows: `<ERROR_ROOT>/<userId>/<runId>/errors.csv`.
fn write_error_file(run_id: &str, rejected: &Rejections) -> Option<String> {
    if rejected.total == 0 {
        return None;
    }
    let csv = error_csv(rejected)?;
    let user = Some(current_user()).filter(|user| !user.is_empty() && !user.contains('/'));
    let location = format!("{ERROR_ROOT}/{}/{run_id}/errors.csv", user.as_deref().unwrap_or("_NA_"));
    let saved = ExecutionContext::with_current(|ec| {
        let _authz = ec.disable_authz();
        ec.factory().resource().write_bytes(&location, csv.as_bytes())
    });
    match saved {
        Some(Ok(())) => Some(location),
        Some(Err(err)) => {
            tracing::error!(error = %err, run = %run_id, "Could not write the Geo data import's error file");
            None
        }
        None => None,
    }
}

/// Write the run's end and its out-parameters. A run marked abandoned meanwhile keeps that status.
fn finish(facade: &EntityFacade, plan: &Plan, mut ended: Ended) -> BTreeMap<String, Value> {
    ended.rejected.kept.sort_by_key(|error| error.line);
    let error_location = write_error_file(&plan.run_id, &ended.rejected);
    let counts = ended.counts;
    let marked = match read_run(facade, &plan.run_id) {
        Ok(Some(row)) if !in_progress(&text_of(&row, "statusId")) => Some(text_of(&row, "statusId")),
        _ => None,
    };
    let mut fields = vec![
        ("rowsRead", Value::I64(counts.read as i64)),
        ("rowsCreated", Value::I64(counts.created as i64)),
        ("rowsUpdated", Value::I64(counts.updated as i64)),
        ("rowsUnchanged", Value::I64(counts.unchanged as i64)),
        ("rowsRetired", Value::I64(counts.retired as i64)),
        ("rowsRejected", Value::I64(counts.rejected as i64)),
        (
            "errorFileLocation",
            error_location.clone().map_or(Value::Null, Value::String),
        ),
    ];
    let status = match &marked {
        Some(status) => {
            tracing::warn!(run = %plan.run_id, status = %status, "A Geo data import ended after it was marked ended");
            status.clone()
        }
        None => {
            fields.push(("statusId", Value::string(ended.status)));
            fields.push(("thruDate", now()));
            fields.push(("failureText", ended.failure.clone().map_or(Value::Null, Value::String)));
            fields.push(("errorLocation", ended.step.map_or(Value::Null, Value::string)));
            if let Some(edition) = &ended.edition {
                fields.push(("edition", Value::string(edition)));
            }
            ended.status.to_string()
        }
    };
    write_run(facade, &plan.run_id, fields);
    if let Some(failure) = &ended.failure {
        tracing::warn!(run = %plan.run_id, data_set = %plan.set.data_set_id, failure = %failure, "A Geo data import failed");
    } else {
        tracing::info!(
            run = %plan.run_id,
            data_set = %plan.set.data_set_id,
            status = %status,
            read = counts.read,
            created = counts.created,
            updated = counts.updated,
            unchanged = counts.unchanged,
            retired = counts.retired,
            rejected = counts.rejected,
            "Geo data import ended"
        );
    }
    let mut out = Out::new();
    out.insert("geoDataImportId".to_string(), Value::string(&plan.run_id));
    out.insert("statusId".to_string(), Value::string(status));
    out.insert(
        "edition".to_string(),
        ended.edition.clone().map_or(Value::Null, Value::String),
    );
    for (name, count) in [
        ("rowsRead", counts.read),
        ("rowsCreated", counts.created),
        ("rowsUpdated", counts.updated),
        ("rowsUnchanged", counts.unchanged),
        ("rowsRetired", counts.retired),
        ("rowsRejected", counts.rejected),
    ] {
        out.insert(name.to_string(), Value::I64(count as i64));
    }
    out.insert(
        "errorFileLocation".to_string(),
        error_location.map_or(Value::Null, Value::String),
    );
    out.insert(
        "failureText".to_string(),
        ended.failure.map_or(Value::Null, Value::String),
    );
    out.insert(
        "errors".to_string(),
        Value::List(
            ended
                .rejected
                .kept
                .iter()
                .take(MAX_ERRORS_OUT)
                .map(|error: &RowError| {
                    let mut map = IndexMap::default();
                    map.insert("line".to_string(), Value::I64(error.line as i64));
                    map.insert("field".to_string(), Value::string(&error.field));
                    map.insert("message".to_string(), Value::string(&error.message));
                    Value::Map(map)
                })
                .collect(),
        ),
    );
    out
}

/// `cancel#GeoDataImport`: ask run `geoDataImportId` to stop. A pending run is cancelled at once; a running one reads
/// the request between its steps and batches and ends `GdiCancelled`, with nothing retired. Out: `statusId`.
pub(crate) fn cancel_geo_data_import(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !authorized(CANCEL_SERVICE, "Cancelling a Geo data import") {
        return Ok(Out::new());
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let run_id = text(parameters, "geoDataImportId");
    let Some(run) = read_run(facade, &run_id)? else {
        add_error(format!("There is no Geo data import {run_id}"));
        return Ok(Out::new());
    };
    let status = text_of(&run, "statusId");
    let mut out = Out::new();
    if status == PENDING {
        write_run(
            facade,
            &run_id,
            vec![
                ("cancelRequested", Value::string("Y")),
                ("statusId", Value::string(CANCELLED)),
                ("thruDate", now()),
            ],
        );
        out.insert("statusId".to_string(), Value::string(CANCELLED));
    } else if in_progress(&status) {
        write_run(facade, &run_id, vec![("cancelRequested", Value::string("Y"))]);
        out.insert("statusId".to_string(), Value::string(status));
    } else {
        add_error(format!("Geo data import {run_id} is not running ({status})"));
    }
    Ok(out)
}

/// `remove#GeoData`: remove what data set `dataSetId`'s source key owns (for `countryGeoId`, needed for a set read a
/// country at a time; for a set with no country, every country): rows nothing refers to are deleted, the rest
/// end-dated; for the ZCTA-to-county set its county links. Refused for a set of countries or divisions, which are
/// shared, and while an import of the source key and country is in progress. Out: `rowsDeleted`, `rowsRetired`.
pub(crate) fn remove_geo_data(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !authorized(REMOVE_SERVICE, "Removing Geo data") {
        return Ok(Out::new());
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let data_set_id = text(parameters, "dataSetId");
    let (row, set) = match data_set(facade, &data_set_id) {
        Ok(found) => found,
        Err(message) => {
            add_error(message);
            return Ok(Out::new());
        }
    };
    let country = match run_country(facade, &row, set.format, &text(parameters, "countryGeoId")) {
        Ok(country) => country,
        Err(message) => {
            add_error(message);
            return Ok(Out::new());
        }
    };
    let country_id = country.as_ref().map(|country| country.geo_id.clone());
    if let Some(refusal) = running_refusal(facade, &set, country_id.as_deref(), "") {
        add_error(refusal);
        return Ok(Out::new());
    }
    match remove::remove(facade, &set, country.as_ref()) {
        Ok(removed) => {
            tracing::info!(data_set = %data_set_id, deleted = removed.deleted, retired = removed.retired, "Removed Geo data");
            let mut out = Out::new();
            out.insert("rowsDeleted".to_string(), Value::I64(removed.deleted as i64));
            out.insert("rowsRetired".to_string(), Value::I64(removed.retired as i64));
            Ok(out)
        }
        Err(err) => {
            add_error(format!("Could not remove Geo data set {data_set_id}'s rows: {err}"));
            Ok(Out::new())
        }
    }
}
