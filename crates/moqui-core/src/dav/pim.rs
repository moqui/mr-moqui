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

//! CalDAV and CardDAV. The framework keeps no calendar or contact of its own: an application serves them through four
//! adapter services in the class the conf's `dav-server` names (`calendar-services`, `contact-services`), each called as
//! the logged-in user, which check access themselves (Mantle has `mantle.work.CalendarDavServices` and
//! `mantle.party.ContactDavServices`). With no class named, that half of the server is not there.
//!
//! | Service | In | Out |
//! |---|---|---|
//! | `get#DavCollections` | `collectionId` (only that one) | `collectionList`: `collectionId`, `name`, `description`, `color`, `order`, `readOnly`, `ctag` (changes whenever an object does), `components` (`VEVENT`, `VTODO`, comma separated; calendars) |
//! | `get#DavObjects` | `collectionId`, `objectNameList` (all when empty), `withData`, `componentType`, `start` and `end` (epoch milliseconds, a calendar query's time range) | `objectList`: `name` (`<id>.ics` or `.vcf`, the name a client PUT it by), `etag`, `data` (with `withData`), `componentType` |
//! | `put#DavObject` | `collectionId`, `objectName`, `data` (iCalendar or vCard text), `ifMatch`, `ifNoneMatch` | `etag`, `created`, `status` (a refusal: 403, 409, 412, 415) |
//! | `delete#DavObject` | `collectionId`, `objectName`, `ifMatch` | `status` |
//!
//! A collection that is not the user's to see is not in `collectionList`, and `get#DavObjects` adds an error for it.
//! Collections are made and removed in the application's screens (MKCALENDAR and a DELETE of a collection are refused).
//! A client keeps in step by the collection's `getctag` and each object's `getetag` (sync-collection is not served).

use indexmap::IndexMap;
use moqui_lib::object::Value;

use super::{
    call, encode, flag_of, maps, multistatus, multistatus_of, prop_response, proppatch, status_of_error,
    status_response, text_of, xml, Context, DavResponse, PropRequest, Resource, BASE,
};
use crate::context::ExecutionContextFactory;
use xml::{Element, CALDAV, CARDDAV, CS, DAV, ICAL};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Calendar,
    Contacts,
}

impl Kind {
    /// The adapter class the conf names, when there is one.
    pub(crate) fn services(self, factory: &ExecutionContextFactory) -> Option<String> {
        let attribute = match self {
            Self::Calendar => "calendar-services",
            Self::Contacts => "contact-services",
        };
        let name = factory.conf_setting("dav-server", attribute);
        let name = name.trim();
        (!name.is_empty()).then(|| name.to_string())
    }

    fn area(self) -> &'static str {
        match self {
            Self::Calendar => "calendars",
            Self::Contacts => "addressbooks",
        }
    }

    fn data_prop(self) -> (&'static str, &'static str) {
        match self {
            Self::Calendar => (CALDAV, "calendar-data"),
            Self::Contacts => (CARDDAV, "address-data"),
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Calendar => ".ics",
            Self::Contacts => ".vcf",
        }
    }
}

struct Adapter {
    kind: Kind,
    services: String,
}

impl Adapter {
    fn call(
        &self,
        verb: &str,
        parameters: &[(&str, Value)],
    ) -> Result<std::collections::BTreeMap<String, Value>, String> {
        call(&format!("{}.{verb}", self.services), parameters)
    }

    /// The user's collections, or only `collection_id`'s (an empty list when the user may not see it).
    fn collections(&self, collection_id: Option<&str>) -> Result<Vec<IndexMap<String, Value>>, String> {
        let parameters: Vec<(&str, Value)> = collection_id
            .map(|id| ("collectionId", Value::string(id)))
            .into_iter()
            .collect();
        Ok(maps(
            self.call("get#DavCollections", &parameters)?.get("collectionList"),
        ))
    }

    fn objects(
        &self,
        collection_id: &str,
        names: Option<&[String]>,
        with_data: bool,
        query: Option<&Query>,
    ) -> Result<Vec<IndexMap<String, Value>>, String> {
        let mut parameters = vec![
            ("collectionId", Value::string(collection_id)),
            ("withData", Value::Bool(with_data)),
        ];
        if let Some(names) = names {
            parameters.push(("objectNameList", Value::List(names.iter().map(Value::string).collect())));
        }
        if let Some(query) = query {
            if let Some(component) = &query.component {
                parameters.push(("componentType", Value::string(component)));
            }
            if let Some(start) = query.start {
                parameters.push(("start", Value::I64(start)));
            }
            if let Some(end) = query.end {
                parameters.push(("end", Value::I64(end)));
            }
        }
        Ok(maps(self.call("get#DavObjects", &parameters)?.get("objectList")))
    }
}

/// A calendar query's component and time range.
struct Query {
    component: Option<String>,
    start: Option<i64>,
    end: Option<i64>,
}

fn home_href(context: &Context<'_>, kind: Kind) -> String {
    format!("{BASE}/{}/{}/", kind.area(), encode(&context.username))
}

fn collection_href(context: &Context<'_>, kind: Kind, collection_id: &str) -> String {
    format!("{}{}/", home_href(context, kind), encode(collection_id))
}

pub(crate) fn handle(
    factory: &ExecutionContextFactory,
    context: &Context<'_>,
    method: &str,
    kind: Kind,
) -> DavResponse {
    let Some(services) = kind.services(factory) else {
        return DavResponse::status(404);
    };
    let adapter = Adapter { kind, services };
    match context.rest.as_slice() {
        [] => home(context, &adapter, method),
        [collection_id] => collection(context, &adapter, method, collection_id),
        [collection_id, name] => object(context, &adapter, method, collection_id, name),
        _ => DavResponse::status(404),
    }
}

fn failed(err: &str) -> DavResponse {
    let status = status_of_error(err);
    if status == 409 {
        tracing::warn!("dav: {err}");
    }
    DavResponse::text(status, err)
}

fn collection_resource(context: &Context<'_>, adapter: &Adapter, row: &IndexMap<String, Value>) -> Resource {
    let collection_id = text_of(row, "collectionId");
    let mut resource = Resource::new(collection_href(context, adapter.kind, &collection_id));
    let name = text_of(row, "name");
    resource.prop(
        DAV,
        "displayname",
        &xml::escape(if name.is_empty() { &collection_id } else { &name }),
    );
    let ctag = xml::escape(&text_of(row, "ctag"));
    resource.prop(CS, "getctag", &ctag);
    resource.prop(DAV, "getetag", &format!("&quot;{ctag}&quot;"));
    let writable = !flag_of(row, "readOnly");
    resource.privileges(writable);
    let description = xml::escape(&text_of(row, "description"));
    match adapter.kind {
        Kind::Calendar => {
            resource.prop(DAV, "resourcetype", "<D:collection/><C:calendar/>");
            resource.prop(CALDAV, "calendar-description", &description);
            let color = text_of(row, "color");
            if !color.is_empty() {
                resource.prop(ICAL, "calendar-color", &xml::escape(&color));
            }
            let order = text_of(row, "order");
            if !order.is_empty() {
                resource.prop(ICAL, "calendar-order", &xml::escape(&order));
            }
            let components = text_of(row, "components");
            let components = if components.is_empty() {
                "VEVENT".to_string()
            } else {
                components
            };
            let set: String = components
                .split(',')
                .map(str::trim)
                .filter(|component| !component.is_empty())
                .map(|component| format!(r#"<C:comp name="{}"/>"#, xml::escape(component)))
                .collect();
            resource.prop(CALDAV, "supported-calendar-component-set", &set);
            resource.prop(
                CALDAV,
                "supported-calendar-data",
                r#"<C:calendar-data content-type="text/calendar" version="2.0"/>"#,
            );
            resource.prop(
                DAV,
                "supported-report-set",
                "<D:supported-report><D:report><C:calendar-multiget/></D:report></D:supported-report><D:supported-report><D:report><C:calendar-query/></D:report></D:supported-report>",
            );
        }
        Kind::Contacts => {
            resource.prop(DAV, "resourcetype", "<D:collection/><CR:addressbook/>");
            resource.prop(CARDDAV, "addressbook-description", &description);
            resource.prop(
                CARDDAV,
                "supported-address-data",
                r#"<CR:address-data-type content-type="text/vcard" version="3.0"/><CR:address-data-type content-type="text/vcard" version="4.0"/>"#,
            );
            resource.prop(
                DAV,
                "supported-report-set",
                "<D:supported-report><D:report><CR:addressbook-multiget/></D:report></D:supported-report><D:supported-report><D:report><CR:addressbook-query/></D:report></D:supported-report>",
            );
        }
    }
    resource.add_user_props(&context.username);
    resource
}

fn content_type(kind: Kind, component: &str) -> String {
    match kind {
        Kind::Calendar if component.is_empty() => "text/calendar; charset=utf-8".to_string(),
        Kind::Calendar => format!(
            "text/calendar; charset=utf-8; component={}",
            component.to_ascii_lowercase()
        ),
        Kind::Contacts => "text/vcard; charset=utf-8".to_string(),
    }
}

fn object_resource(
    context: &Context<'_>,
    adapter: &Adapter,
    collection_id: &str,
    row: &IndexMap<String, Value>,
    writable: bool,
) -> Resource {
    let name = text_of(row, "name");
    let mut resource = Resource::new(format!(
        "{}{}",
        collection_href(context, adapter.kind, collection_id),
        encode(&name)
    ));
    resource.prop(DAV, "resourcetype", "");
    resource.prop(DAV, "getetag", &xml::escape(&quoted(&text_of(row, "etag"))));
    resource.prop(
        DAV,
        "getcontenttype",
        &xml::escape(&content_type(adapter.kind, &text_of(row, "componentType"))),
    );
    resource.prop(DAV, "displayname", &xml::escape(&name));
    if let Some(data) = row.get("data").filter(|value| !moqui_lib::object::is_empty(value)) {
        let data = moqui_lib::object::to_plain_string(data);
        resource.prop(DAV, "getcontentlength", &data.len().to_string());
        let (ns, prop) = adapter.kind.data_prop();
        resource.heavy_prop(ns, prop, &xml::escape(&data));
    }
    resource.privileges(writable);
    resource.add_user_props(&context.username);
    resource
}

/// An entity tag in quotes, as the adapter may give it bare.
fn quoted(etag: &str) -> String {
    if etag.starts_with('"') {
        etag.to_string()
    } else {
        format!("\"{etag}\"")
    }
}

fn home(context: &Context<'_>, adapter: &Adapter, method: &str) -> DavResponse {
    match method {
        "PROPFIND" => {}
        "MKCALENDAR" | "MKCOL" => {
            return DavResponse::text(403, "calendars and address books are made in the app's settings")
        }
        "REPORT" => return DavResponse::condition(403, DAV, "supported-report"),
        _ => return DavResponse::status(405),
    }
    let wanted = PropRequest::from_propfind(context.body.as_ref());
    let title = match adapter.kind {
        Kind::Calendar => "Calendars",
        Kind::Contacts => "Address books",
    };
    let mut home = Resource::collection(home_href(context, adapter.kind), title);
    home.privileges(false);
    home.add_user_props(&context.username);
    let mut resources = vec![home];
    if super::depth(context.request) > 0 {
        match adapter.collections(None) {
            Ok(rows) => resources.extend(rows.iter().map(|row| collection_resource(context, adapter, row))),
            Err(err) => return failed(&err),
        }
    }
    multistatus(&resources, &wanted)
}

fn find_collection(adapter: &Adapter, collection_id: &str) -> Result<Option<IndexMap<String, Value>>, String> {
    Ok(adapter
        .collections(Some(collection_id))?
        .into_iter()
        .find(|row| text_of(row, "collectionId") == collection_id))
}

/// Whether `wanted` asks for the object data by name (allprop does not include it).
fn wants_data(kind: Kind, wanted: &PropRequest) -> bool {
    let (ns, name) = kind.data_prop();
    matches!(wanted, PropRequest::Some(_)) && wanted.wants(ns, name)
}

fn collection(context: &Context<'_>, adapter: &Adapter, method: &str, collection_id: &str) -> DavResponse {
    let row = match find_collection(adapter, collection_id) {
        Ok(Some(row)) => row,
        Ok(None) if matches!(method, "MKCALENDAR" | "MKCOL") => {
            return DavResponse::text(403, "calendars and address books are made in the app's settings")
        }
        Ok(None) => return DavResponse::status(404),
        Err(err) => return failed(&err),
    };
    let writable = !flag_of(&row, "readOnly");
    match method {
        "PROPFIND" => {
            let wanted = PropRequest::from_propfind(context.body.as_ref());
            let mut resources = vec![collection_resource(context, adapter, &row)];
            if super::depth(context.request) > 0 {
                match adapter.objects(collection_id, None, wants_data(adapter.kind, &wanted), None) {
                    Ok(objects) => resources.extend(
                        objects
                            .iter()
                            .map(|object| object_resource(context, adapter, collection_id, object, writable)),
                    ),
                    Err(err) => return failed(&err),
                }
            }
            multistatus(&resources, &wanted)
        }
        "REPORT" => report(context, adapter, collection_id, writable),
        "PROPPATCH" => proppatch(
            &collection_href(context, adapter.kind, collection_id),
            context.body.as_ref(),
        ),
        "MKCALENDAR" | "MKCOL" => DavResponse::status(405),
        "DELETE" => DavResponse::text(403, "a calendar or address book is removed in the app's settings"),
        "GET" | "HEAD" => DavResponse::text(200, &text_of(&row, "name")),
        _ => DavResponse::status(405),
    }
}

fn report(context: &Context<'_>, adapter: &Adapter, collection_id: &str, writable: bool) -> DavResponse {
    let Some(body) = context.body.as_ref() else {
        return DavResponse::status(400);
    };
    let wanted = PropRequest::from_parent(body);
    let with_data = wanted.wants(adapter.kind.data_prop().0, adapter.kind.data_prop().1);
    let base = collection_href(context, adapter.kind, collection_id);
    let multiget = match adapter.kind {
        Kind::Calendar => body.is(CALDAV, "calendar-multiget"),
        Kind::Contacts => body.is(CARDDAV, "addressbook-multiget"),
    };
    let query = match adapter.kind {
        Kind::Calendar => body.is(CALDAV, "calendar-query"),
        Kind::Contacts => body.is(CARDDAV, "addressbook-query"),
    };
    if multiget {
        let mut asked: Vec<(String, String)> = Vec::new();
        for href in body.children_named(DAV, "href") {
            let text = href.text.trim();
            let path = text.find("://").map_or(text, |scheme| {
                let after = &text[scheme + 3..];
                after.find('/').map_or("/", |slash| &after[slash..])
            });
            let name = path.rsplit('/').next().map(super::decode_segment).unwrap_or_default();
            asked.push((path.to_string(), name));
        }
        let names: Vec<String> = asked.iter().map(|(_, name)| name.clone()).collect();
        let objects = match adapter.objects(collection_id, Some(&names), with_data, None) {
            Ok(objects) => objects,
            Err(err) => return failed(&err),
        };
        let responses: Vec<String> = asked
            .iter()
            .map(
                |(path, name)| match objects.iter().find(|object| &text_of(object, "name") == name) {
                    Some(object) if super::decode_segment(path).starts_with(&super::decode_segment(&base)) => {
                        prop_response(
                            &object_resource(context, adapter, collection_id, object, writable),
                            &wanted,
                        )
                    }
                    _ => status_response(path, 404),
                },
            )
            .collect();
        return multistatus_of(&responses);
    }
    if query {
        let filter = match adapter.kind {
            Kind::Calendar => calendar_query(body),
            Kind::Contacts => None,
        };
        let limit = body
            .child(CARDDAV, "limit")
            .and_then(|limit| limit.child(CARDDAV, "nresults"))
            .and_then(|count| count.text.trim().parse::<usize>().ok());
        let objects = match adapter.objects(collection_id, None, with_data, filter.as_ref()) {
            Ok(objects) => objects,
            Err(err) => return failed(&err),
        };
        let resources: Vec<Resource> = objects
            .iter()
            .take(limit.unwrap_or(usize::MAX))
            .map(|object| object_resource(context, adapter, collection_id, object, writable))
            .collect();
        return multistatus(&resources, &wanted);
    }
    DavResponse::condition(403, DAV, "supported-report")
}

/// The component and time range of a `calendar-query` filter: `VCALENDAR` > `VEVENT` or `VTODO` > `time-range`.
fn calendar_query(body: &Element) -> Option<Query> {
    let calendar = body.child(CALDAV, "filter")?.child(CALDAV, "comp-filter")?;
    let component = calendar.child(CALDAV, "comp-filter");
    let range = component.and_then(|component| component.child(CALDAV, "time-range"));
    Some(Query {
        component: component
            .and_then(|component| component.attribute("name"))
            .map(str::to_ascii_uppercase),
        start: range
            .and_then(|range| range.attribute("start"))
            .and_then(ical_utc_millis),
        end: range.and_then(|range| range.attribute("end")).and_then(ical_utc_millis),
    })
}

/// `20240131T080000Z` as epoch milliseconds.
pub(crate) fn ical_utc_millis(text: &str) -> Option<i64> {
    let text = text.trim().trim_end_matches('Z');
    let (date, time) = text.split_once('T').unwrap_or((text, "000000"));
    if date.len() != 8 || time.len() < 6 {
        return None;
    }
    let number = |from: &str| from.parse::<i16>().ok();
    let civil = jiff::civil::DateTime::new(
        number(&date[0..4])?,
        number(&date[4..6])? as i8,
        number(&date[6..8])? as i8,
        number(&time[0..2])? as i8,
        number(&time[2..4])? as i8,
        number(&time[4..6])? as i8,
        0,
    )
    .ok()?;
    civil
        .to_zoned(jiff::tz::TimeZone::UTC)
        .ok()
        .map(|zoned| zoned.timestamp().as_millisecond())
}

fn object(context: &Context<'_>, adapter: &Adapter, method: &str, collection_id: &str, name: &str) -> DavResponse {
    let collection = match find_collection(adapter, collection_id) {
        Ok(Some(row)) => row,
        Ok(None) => return DavResponse::status(404),
        Err(err) => return failed(&err),
    };
    let writable = !flag_of(&collection, "readOnly");
    let href = format!(
        "{}{}",
        collection_href(context, adapter.kind, collection_id),
        encode(name)
    );
    let one = |with_data: bool| -> Result<Option<IndexMap<String, Value>>, String> {
        Ok(adapter
            .objects(collection_id, Some(&[name.to_string()]), with_data, None)?
            .into_iter()
            .find(|object| text_of(object, "name") == name))
    };
    match method {
        "GET" | "HEAD" => {
            let object = match one(true) {
                Ok(Some(object)) => object,
                Ok(None) => return DavResponse::status(404),
                Err(err) => return failed(&err),
            };
            let etag = quoted(&text_of(&object, "etag"));
            if super::tag_list(context.request.header("if-none-match"))
                .iter()
                .any(|tag| tag == &etag || tag == "*")
            {
                return DavResponse::status(304).with_header("ETag", etag);
            }
            let data = text_of(&object, "data").into_bytes();
            DavResponse {
                status: 200,
                headers: vec![
                    (
                        "Content-Type".into(),
                        content_type(adapter.kind, &text_of(&object, "componentType")),
                    ),
                    ("ETag".into(), etag),
                    ("Content-Length".into(), data.len().to_string()),
                ],
                body: if method == "HEAD" { Vec::new() } else { data },
            }
        }
        "PROPFIND" => {
            let wanted = PropRequest::from_propfind(context.body.as_ref());
            match one(wants_data(adapter.kind, &wanted)) {
                Ok(Some(object)) => multistatus(
                    &[object_resource(context, adapter, collection_id, &object, writable)],
                    &wanted,
                ),
                Ok(None) => DavResponse::status(404),
                Err(err) => failed(&err),
            }
        }
        "PUT" => {
            if !writable {
                return DavResponse::condition(403, DAV, "need-privileges");
            }
            if !name.ends_with(adapter.kind.extension()) {
                return DavResponse::text(403, &format!("a name here ends with {}", adapter.kind.extension()));
            }
            let Ok(data) = String::from_utf8(context.request.body.clone()) else {
                return DavResponse::status(415);
            };
            let mut parameters = vec![
                ("collectionId", Value::string(collection_id)),
                ("objectName", Value::string(name)),
                ("data", Value::string(data)),
            ];
            if let Some(tag) = context.request.header("if-match") {
                parameters.push(("ifMatch", Value::string(tag.trim().trim_start_matches("W/"))));
            }
            if let Some(tag) = context.request.header("if-none-match") {
                parameters.push(("ifNoneMatch", Value::string(tag.trim())));
            }
            let out = match adapter.call("put#DavObject", &parameters) {
                Ok(out) => out,
                Err(err) => return failed(&err),
            };
            let status = out
                .get("status")
                .map(moqui_lib::object::to_plain_string)
                .and_then(|text| text.parse::<u16>().ok());
            if let Some(status) = status.filter(|status| *status >= 300) {
                return DavResponse::status(status);
            }
            let created = out
                .get("created")
                .is_some_and(|value| moqui_lib::object::to_plain_string(value) == "true");
            let mut answer = DavResponse::status(if created { 201 } else { 204 });
            let etag = out
                .get("etag")
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default();
            if !etag.is_empty() {
                answer = answer.with_header("ETag", quoted(&etag));
            }
            answer
        }
        "DELETE" => {
            if !writable {
                return DavResponse::condition(403, DAV, "need-privileges");
            }
            let mut parameters = vec![
                ("collectionId", Value::string(collection_id)),
                ("objectName", Value::string(name)),
            ];
            if let Some(tag) = context.request.header("if-match") {
                parameters.push(("ifMatch", Value::string(tag.trim().trim_start_matches("W/"))));
            }
            match adapter.call("delete#DavObject", &parameters) {
                Ok(out) => {
                    let status = out
                        .get("status")
                        .map(moqui_lib::object::to_plain_string)
                        .and_then(|text| text.parse::<u16>().ok());
                    DavResponse::status(status.filter(|status| *status >= 300).unwrap_or(204))
                }
                Err(err) => failed(&err),
            }
        }
        "PROPPATCH" => proppatch(&href, context.body.as_ref()),
        _ => DavResponse::status(405),
    }
}

#[cfg(test)]
mod tests {
    use super::ical_utc_millis;

    #[test]
    fn a_time_range_is_read_in_utc() {
        assert_eq!(ical_utc_millis("19700101T000001Z"), Some(1000));
        assert_eq!(ical_utc_millis("20240101T000000Z"), Some(1_704_067_200_000));
        assert_eq!(ical_utc_millis("2024"), None);
    }
}
