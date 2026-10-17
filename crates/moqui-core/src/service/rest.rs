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

//! REST API. `*.rest.xml` is walked here. There is no HTTP route. JSON-RPC is not included.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, RwLock};

use indexmap::IndexMap;

use moqui_lib::node::MNode;
use moqui_lib::object::{to_plain_string, Value};

use super::call;
use super::define::{ParamDef, Required};
use crate::context::{Error, ErrorKind, ExecutionContext, ExecutionContextFactory};
use crate::entity::{sensitive, EntityFacade};

/// The REST resources of a factory, read once from its components' `service/*.rest.xml` files.
struct Roots {
    resources: BTreeMap<String, Node>,
}

type Shared = RwLock<Arc<Roots>>;

/// Walk `path` through `*.rest.xml` and run the `<entity>` or `<service>` operation.
///
/// The first element is the root resource `name`. `method` is the HTTP method, in any case, and
/// comes from the request, never from the parameters. The errors have kinds the web layer maps to
/// status codes: not found, method not allowed, and authentication required.
///
/// # Errors
///
/// Returns an error when the path or method is not served, or the operation fails.
pub fn run(path: &[&str], method: &str, parameters: &BTreeMap<String, Value>) -> Result<Value, Error> {
    if path.is_empty() {
        return Err(Error::of_kind(
            ErrorKind::NotFound,
            "Cannot run REST service with no path",
        ));
    }
    let method = method.trim().to_ascii_lowercase();
    let mut params = parameters.clone();
    let mut roots = current_roots()?;
    // As Java's `getRootResourceNode`: a root not loaded reads the REST files again, for one added since. Here at
    // most once in ten seconds, so requests for unknown roots do not read the files each time.
    if !roots.resources.contains_key(path[0]) && miss_reload_due() {
        roots = reload_current_roots()?;
    }
    let root = roots.resources.get(path[0]).ok_or_else(|| {
        Error::of_kind(
            ErrorKind::NotFound,
            format!("Service REST API root resource not found with name {}", path[0]),
        )
    })?;
    root.visit(path, 0, &method, &mut params)
}

/// `RestApi.getFreshRootResources` for the tools dashboard: each root resource with its `name`, `displayName`,
/// `description`, the counts of paths and methods under it (`childPaths`, `childMethods`), and its child resources by
/// name (`resourceMap`). Java's `methodMap` of handler objects, read only by its LLM browse tool, is not here.
///
/// # Errors
///
/// Returns an error when no context is entered.
pub fn root_resource_infos() -> Result<Vec<Value>, Error> {
    // Java's `getFreshRootResources` reads the files again.
    let roots = reload_current_roots()?;
    Ok(roots.resources.values().map(|node| resource_info(node, true)).collect())
}

/// Resolve a resource path (without `/rest/s1`) and HTTP method to its service and path parameter names.
/// Uses dispatch's named-resource-before-id rule. This neither executes nor authorizes a call;
/// consumers must still enforce authorization when executing the returned service.
pub fn resolve_service(path: &str, method: &str) -> Result<Value, Error> {
    let parts: Vec<_> = path
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let roots = current_roots()?;
    let mut node = parts
        .first()
        .and_then(|root| roots.resources.get(*root))
        .ok_or_else(|| Error::new("REST root resource not found"))?;
    let mut path_parameters = Vec::new();
    for part in &parts[1..] {
        node = node
            .resources
            .get(*part)
            .or(node.id.as_deref())
            .ok_or_else(|| Error::new("REST resource not found"))?;
        if node.is_id {
            path_parameters.push(Value::string(&node.name));
        }
        if node.is_id && node.allow_extra_path {
            break;
        }
    }
    if !node.methods.contains_key(&method.to_ascii_lowercase()) {
        if let Some(id) = node.id.as_deref().filter(|id| id.allow_extra_path) {
            node = id;
            path_parameters.push(Value::string(&node.name));
        }
    }
    let operation = node
        .methods
        .get(&method.to_ascii_lowercase())
        .ok_or_else(|| Error::new("REST method not found"))?;
    let Operation::Service { name, .. } = operation else {
        return Err(Error::new("REST method is not backed by a service"));
    };
    Ok(Value::Map(indexmap::indexmap! {
        "serviceName".to_string() => Value::string(name),
        "path".to_string() => Value::string(format!("/rest/s1/{}", parts.join("/"))),
        "pathParameters".to_string() => Value::List(path_parameters),
        "method".to_string() => Value::string(method.to_ascii_lowercase()),
    }))
}

fn resource_info(node: &Node, with_children: bool) -> Value {
    let text = |value: &Option<String>| value.as_deref().map_or(Value::Null, Value::string);
    let mut map = IndexMap::new();
    map.insert("name".to_string(), Value::string(&node.name));
    map.insert("displayName".to_string(), text(&node.display_name));
    map.insert("description".to_string(), text(&node.description));
    map.insert(
        "childPaths".to_string(),
        Value::I64(i64::try_from(node.path_count()).unwrap_or(i64::MAX)),
    );
    map.insert(
        "childMethods".to_string(),
        Value::I64(i64::try_from(node.method_count()).unwrap_or(i64::MAX)),
    );
    if with_children {
        map.insert(
            "resourceMap".to_string(),
            Value::Map(
                node.resources
                    .iter()
                    .map(|(name, child)| (name.clone(), resource_info(child, false)))
                    .collect(),
            ),
        );
    }
    Value::Map(map)
}

impl Node {
    /// Java's `childPaths`: each child resource and id, and the paths under them.
    fn path_count(&self) -> usize {
        self.resources
            .values()
            .map(|child| 1 + child.path_count())
            .sum::<usize>()
            + self.id.as_ref().map_or(0, |id| 1 + id.path_count())
    }

    fn method_count(&self) -> usize {
        self.methods.len()
            + self.resources.values().map(Node::method_count).sum::<usize>()
            + self.id.as_ref().map_or(0, |id| id.method_count())
    }
}

fn current_roots() -> Result<Arc<Roots>, Error> {
    let factory = ExecutionContext::with_current(|ctx| Arc::clone(ctx.factory()))
        .ok_or_else(|| Error::new("no execution context is entered"))?;
    Ok(roots_of(&factory))
}

/// True at most once in ten seconds, process wide.
fn miss_reload_due() -> bool {
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|err| err.into_inner());
    let due = last.is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(10));
    if due {
        *last = Some(std::time::Instant::now());
    }
    due
}

/// Read the REST files again and keep what they define now.
fn reload_current_roots() -> Result<Arc<Roots>, Error> {
    let factory = ExecutionContext::with_current(|ctx| Arc::clone(ctx.factory()))
        .ok_or_else(|| Error::new("no execution context is entered"))?;
    let shared = factory.extension::<Shared>(|factory| RwLock::new(Arc::new(load(factory))));
    let fresh = Arc::new(load(&factory));
    *shared.write().unwrap_or_else(|err| err.into_inner()) = Arc::clone(&fresh);
    Ok(fresh)
}

fn roots_of(factory: &Arc<ExecutionContextFactory>) -> Arc<Roots> {
    let shared = factory.extension::<Shared>(|factory| RwLock::new(Arc::new(load(factory))));
    let guard = shared.read().unwrap_or_else(|err| err.into_inner());
    Arc::clone(&guard)
}

/// Read every component's REST files. A file that does not load is logged and left out, so one bad
/// file never takes the other resources down. A later file with the same root name replaces an earlier one.
fn load(factory: &ExecutionContextFactory) -> Roots {
    let mut resources = BTreeMap::new();
    for component in factory.components() {
        let dir = component.location.join("service");
        load_dir(&mut resources, &dir);
    }
    tracing::info!(roots = resources.len(), "loaded REST API resources");
    Roots { resources }
}

fn load_dir(resources: &mut BTreeMap<String, Node>, dir: &Path) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = read
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".rest.xml"))
        })
        .collect();
    paths.sort();
    for path in paths {
        if let Err(err) = insert_file(resources, &path) {
            tracing::error!(file = %path.display(), error = %err, "REST file did not load");
        }
    }
}

fn insert_file(roots: &mut BTreeMap<String, Node>, path: &Path) -> Result<(), Error> {
    let root = MNode::parse_file(path)?;
    if root.name() != "resource" {
        return Err(Error::new(format!(
            "{} root is {} and not resource",
            path.display(),
            root.name()
        )));
    }
    let node = node_from(&root, "true", "", false)?;
    if roots.contains_key(&node.name) {
        tracing::warn!(resource = %node.name, file = %path.display(), "REST root resource replaced by a later file");
    }
    roots.insert(node.name.clone(), node);
    Ok(())
}

/// Replace a factory's resources with those in `paths`, for tests that bring their own files.
#[cfg(test)]
pub(crate) fn install(factory: &Arc<ExecutionContextFactory>, paths: &[PathBuf]) {
    let mut resources = BTreeMap::new();
    for path in paths {
        insert_file(&mut resources, path).expect("rest file");
    }
    let shared = factory.extension::<Shared>(|_| {
        RwLock::new(Arc::new(Roots {
            resources: BTreeMap::new(),
        }))
    });
    *shared.write().unwrap_or_else(|err| err.into_inner()) = Arc::new(Roots { resources });
}

struct Node {
    name: String,
    /// `displayName` and `description` of a resource, for the tools REST API list.
    display_name: Option<String>,
    description: Option<String>,
    is_id: bool,
    allow_extra_path: bool,
    /// The `AT_REST_PATH` artifact name: `/` and each name from the root, an id as `{name}`, as Java `getFullPathName`.
    full_path: String,
    /// `require-authentication` of the resource, else of its parent, else `true`.
    require_authentication: String,
    methods: BTreeMap<String, Operation>,
    resources: BTreeMap<String, Node>,
    id: Option<Box<Node>>,
}

enum Operation {
    Service {
        name: String,
        require_authentication: String,
    },
    Entity {
        name: String,
        master_name: Option<String>,
        operation: String,
        require_authentication: String,
    },
}

fn node_from(node: &MNode, parent_auth: &str, parent_path: &str, is_id: bool) -> Result<Node, Error> {
    let name = node.attribute("name").unwrap_or_default();
    if name.is_empty() {
        return Err(Error::new("REST resource is missing a name"));
    }
    // An empty value inherits, as Java's `?:`.
    let auth = node
        .attribute("require-authentication")
        .filter(|mode| !mode.is_empty())
        .unwrap_or_else(|| parent_auth.to_string());
    let full_path = if is_id {
        format!("{parent_path}/{{{name}}}")
    } else {
        format!("{parent_path}/{name}")
    };
    let mut methods = BTreeMap::new();
    let mut resources = BTreeMap::new();
    let mut id = None;
    for child in node.children() {
        match child.name().as_str() {
            "method" => {
                // Keyed by `type` as written, as Java's `methodMap`, so a `type="GET"` is never matched. A method
                // that does not load is logged and left out and the rest of the file served (Java skips an unknown
                // operation, keys a missing type by null, keeps a missing name, fails the file with no operation).
                let verb = child.attribute("type").unwrap_or_default();
                let operation = if verb.is_empty() {
                    Err(Error::new("REST method is missing a type"))
                } else {
                    operation_from(&child, &auth)
                };
                match operation {
                    Ok(operation) => {
                        methods.insert(verb, operation);
                    }
                    Err(err) => {
                        tracing::error!(resource = %full_path, method = %verb, error = %err, "REST method left out")
                    }
                }
            }
            "resource" => {
                let child_node = node_from(&child, &auth, &full_path, false)?;
                resources.insert(child_node.name.clone(), child_node);
            }
            "id" => id = Some(Box::new(node_from(&child, &auth, &full_path, true)?)),
            _ => {}
        }
    }
    Ok(Node {
        name,
        display_name: node.attribute("displayName").filter(|text| !text.is_empty()),
        description: node.attribute("description").filter(|text| !text.is_empty()),
        is_id,
        allow_extra_path: is_id && node.attribute("allow-extra-path").as_deref() == Some("true"),
        full_path,
        require_authentication: auth,
        methods,
        resources,
        id,
    })
}

fn operation_from(method: &MNode, parent_auth: &str) -> Result<Operation, Error> {
    let auth = method
        .attribute("require-authentication")
        .filter(|mode| !mode.is_empty())
        .unwrap_or_else(|| parent_auth.to_string());
    let body = method
        .children()
        .into_iter()
        .next()
        .ok_or_else(|| Error::new("REST method has no operation"))?;
    match body.name().as_str() {
        "service" => {
            let name = body.attribute("name").unwrap_or_default();
            if name.is_empty() {
                return Err(Error::new("REST service operation is missing a name"));
            }
            Ok(Operation::Service {
                name,
                require_authentication: auth,
            })
        }
        "entity" => {
            let name = body.attribute("name").unwrap_or_default();
            let operation = body.attribute("operation").unwrap_or_default();
            if name.is_empty() || operation.is_empty() {
                return Err(Error::new("REST entity operation is missing a name or operation"));
            }
            Ok(Operation::Entity {
                name,
                master_name: body.attribute("masterName").filter(|text| !text.is_empty()),
                operation,
                require_authentication: auth,
            })
        }
        other => Err(Error::new(format!("REST method operation {other} is not supported"))),
    }
}

impl Node {
    fn visit(
        &self,
        path: &[&str],
        index: usize,
        method: &str,
        params: &mut BTreeMap<String, Value>,
    ) -> Result<Value, Error> {
        let index = if self.is_id {
            self.bind_id(path, index, params);
            if self.allow_extra_path {
                path.len().saturating_sub(1)
            } else {
                index
            }
        } else {
            index
        };
        self.finish(path, index, method, params)
    }

    fn bind_id(&self, path: &[&str], index: usize, params: &mut BTreeMap<String, Value>) {
        if self.allow_extra_path {
            let items = path
                .get(index..)
                .unwrap_or(&[])
                .iter()
                .copied()
                .map(Value::string)
                .collect();
            params.insert(self.name.clone(), Value::List(items));
            return;
        }
        let value = path.get(index).copied().unwrap_or("");
        params.insert(self.name.clone(), Value::string(value));
    }

    fn finish(
        &self,
        path: &[&str],
        index: usize,
        method: &str,
        params: &mut BTreeMap<String, Value>,
    ) -> Result<Value, Error> {
        let next = index + 1;
        let more = path.len() > next;
        // The frame for this path, checked only when it is the last and requires authentication, as Java
        // `visitChildOrRun`. It is popped, and an anonymous login undone, when the guard drops.
        let mut frame = PathFrame::default();
        frame.push(self, method, !more && auth_required(&self.require_authentication))?;
        frame.apply_anonymous(&self.require_authentication)?;
        if more {
            let segment = path[next];
            if let Some(child) = self.resources.get(segment) {
                return child.visit(path, next, method, params);
            }
            if let Some(id) = &self.id {
                return id.visit(path, next, method, params);
            }
            let names = self.resources.keys().map(String::as_str).collect::<Vec<_>>().join(", ");
            return Err(Error::of_kind(
                ErrorKind::NotFound,
                format!(
                    "Resource {segment} not valid, index {index} in path {}; resources available are {names}",
                    path.join("/")
                ),
            ));
        }
        if !self.methods.contains_key(method) {
            if let Some(id) = &self.id {
                if id.allow_extra_path {
                    return id.visit(path, next, method, params);
                }
            }
            return Err(Error::of_kind(
                ErrorKind::MethodNotAllowed,
                format!("Method {method} not supported at {}", path.join("/")),
            ));
        }
        let operation = &self.methods[method];
        match operation {
            Operation::Service {
                name,
                require_authentication,
            } => run_service(name, require_authentication, params),
            Operation::Entity {
                name,
                master_name,
                operation,
                require_authentication,
            } => run_entity(name, master_name.as_deref(), operation, require_authentication, params),
        }
    }
}

/// `true` or empty `require-authentication`.
fn auth_required(mode: &str) -> bool {
    mode.is_empty() || mode == "true"
}

/// Java `actionByMethodMap`. A method not in it, which has no operation anyway, is checked as view.
fn action_from_method(method: &str) -> &'static str {
    match method {
        "patch" | "put" => "update",
        "post" => "create",
        "delete" => "delete",
        _ => "view",
    }
}

/// The `AT_REST_PATH` frame of one path node, and an anonymous login it made.
#[derive(Default)]
struct PathFrame {
    pushed: bool,
    anonymous: bool,
}

impl PathFrame {
    /// Push the frame, not counted as an artifact hit. A denied push is an authentication error when no one is
    /// logged in, so the client is asked to log in: the Java sent 403 there.
    fn push(&mut self, node: &Node, method: &str, requires_authz: bool) -> Result<(), Error> {
        let outcome = ExecutionContext::with_current(|ec| {
            ec.push_checked(
                "AT_REST_PATH",
                &node.full_path,
                action_from_method(method),
                requires_authz,
                true,
                None,
            )?;
            ec.untrack_hit();
            Ok::<(), Error>(())
        });
        match outcome {
            Some(Ok(())) => {
                self.pushed = true;
                Ok(())
            }
            Some(Err(err)) => {
                if ErrorKind::of(&err) == ErrorKind::Unauthorized && crate::user::current_username()?.is_none() {
                    return Err(Error::of_kind(
                        ErrorKind::AuthenticationRequired,
                        format!("User must be logged in for REST path {}", node.full_path),
                    ));
                }
                Err(err)
            }
            None => Ok(()),
        }
    }

    /// `anonymous-all` and `anonymous-view` mark the top frame inheritable, and log in the anonymous user when no
    /// one is logged in.
    fn apply_anonymous(&mut self, mode: &str) -> Result<(), Error> {
        let all = match mode {
            "anonymous-all" => true,
            "anonymous-view" => false,
            _ => return Ok(()),
        };
        ExecutionContext::with_current(|ec| ec.set_anonymous_authorized(all, ""));
        if crate::user::login_anonymous()? {
            self.anonymous = true;
        }
        Ok(())
    }
}

impl Drop for PathFrame {
    fn drop(&mut self) {
        if self.anonymous {
            crate::user::logout_anonymous_only();
        }
        if self.pushed {
            ExecutionContext::with_current(|ec| {
                let _ = ec.pop();
            });
        }
    }
}

fn run_service(name: &str, auth: &str, params: &BTreeMap<String, Value>) -> Result<Value, Error> {
    require_user(auth, &format!("call service {name}"))?;
    // The method's own `anonymous-*` marks the last path frame, as Java `MethodService.run`.
    let mut anonymous = PathFrame::default();
    anonymous.apply_anonymous(auth)?;
    let mut result = call::sync(name, params)?;
    remove_result_nulls(&mut result);
    // As Java's `RestResult`, the password fields of a `UserAccount` are left out wherever they are in the result.
    Ok(sensitive::strip_user_account_secrets(value_map(result)))
}

/// Java's `nestedRemoveNullsFromResultMap`: the null out-parameters are left out, and the null fields of an entity
/// value and of the maps that lead a list. Entity values are maps here, so a plain map out-parameter loses its null
/// entries too, where Java keeps them.
fn remove_result_nulls(result: &mut BTreeMap<String, Value>) {
    result.retain(|_, value| !value.is_null());
    for value in result.values_mut() {
        match value {
            Value::Map(map) => map.retain(|_, field| !field.is_null()),
            Value::List(items) => {
                for item in items {
                    let Value::Map(map) = item else {
                        break;
                    };
                    map.retain(|_, field| !field.is_null());
                }
            }
            _ => {}
        }
    }
}

fn run_entity(
    name: &str,
    master_name: Option<&str>,
    operation: &str,
    auth: &str,
    params: &BTreeMap<String, Value>,
) -> Result<Value, Error> {
    require_user(auth, &format!("operation {operation} on entity {name}"))?;
    let mut anonymous = PathFrame::default();
    anonymous.apply_anonymous(auth)?;
    let facade = current_facade()?;
    // As Java's `RestApi`, identity-admin entities are never readable here; secret-config reads are allowed on a
    // declared resource and writes need ADMIN. `UserAccount` writes lose the password fields, and the identity
    // fields unless the caller is ADMIN. Results never carry the password fields.
    let full_name = facade
        .with_entity(name, |entity, _, _| Ok(entity.full_name.clone()))
        .unwrap_or_else(|_| name.to_string());
    let refused = |what: &str| {
        Err(Error::of_kind(
            ErrorKind::Unauthorized,
            format!("Entity {full_name} is not {what} through this REST resource"),
        ))
    };
    let result = match operation {
        "list" | "one" | "count" if sensitive::is_identity_admin_entity(&full_name) => return refused("available"),
        "list" | "one" => find_entity(&facade, name, master_name, operation, params),
        "count" => count_entity(&facade, name, params),
        "create" | "update" | "store" | "delete" => {
            if sensitive::is_identity_admin_entity(&full_name)
                || (sensitive::is_secret_config_entity(&full_name) && !crate::user::is_in_group("ADMIN")?)
            {
                return refused("writable");
            }
            let mut parms = params.clone();
            if operation != "delete" && sensitive::is_user_account_entity(&full_name) {
                sensitive::remove_user_account_secrets(&mut parms);
                if !crate::user::is_in_group("ADMIN")? {
                    sensitive::remove_user_account_identity(&mut parms);
                }
            }
            let service = format!("{operation}#{name}");
            Ok(value_map(call::sync(&service, &parms)?))
        }
        other => Err(Error::new(format!(
            "Entity operation {other} not supported, must be one of: one, list, count, create, update, store, delete"
        ))),
    }?;
    Ok(sensitive::strip_user_account_secrets(result))
}

/// Java's `MethodEntity` one and list: the search form conditions of the parameters. `one` is the first row found,
/// without its null fields, or null; a list is at most 100 rows unless `pageNoLimit` is `true`, with its page numbers
/// for the response headers. A master name gives the master graph of each row.
fn find_entity(
    facade: &EntityFacade,
    name: &str,
    master_name: Option<&str>,
    operation: &str,
    params: &BTreeMap<String, Value>,
) -> Result<Value, Error> {
    let find = facade
        .find(name)?
        .search_form_map(params, &BTreeMap::new(), "", "", false)?
        .find;
    let master = master_name.filter(|master| !master.is_empty());
    let as_value = |row: &crate::entity::EntityValue| -> Result<Value, Error> {
        match master {
            Some(master) => Ok(crate::entity::query_rest::master_value(facade, row, master)?),
            None => Ok(Value::Map(
                row.fields()
                    .iter()
                    .filter(|(_, value)| !value.is_null())
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
            )),
        }
    };
    if operation == "one" {
        return match find.one()? {
            Some(row) => as_value(&row),
            None => Ok(Value::Null),
        };
    }
    let no_limit = params
        .get("pageNoLimit")
        .is_some_and(|value| to_plain_string(value) == "true");
    let find = if find.limit_value().is_none() && !no_limit {
        find.limit(100)
    } else {
        find
    };
    let (limit, offset) = (find.limit_value(), find.offset_value());
    let total = find.clone().count()?;
    crate::entity::query_rest::note_page_info(limit, offset, params, total, false);
    let rows = find.list()?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows.iter() {
        items.push(as_value(row)?);
    }
    Ok(Value::List(items))
}

/// Java's `MethodEntity` count: the search form conditions of the parameters, the total also in `X-Total-Count`.
fn count_entity(facade: &EntityFacade, name: &str, params: &BTreeMap<String, Value>) -> Result<Value, Error> {
    let count = facade
        .find(name)?
        .search_form_map(params, &BTreeMap::new(), "", "", false)?
        .find
        .count()?;
    crate::entity::query_rest::note_page_info(None, None, params, count, true);
    let mut entries = BTreeMap::new();
    entries.insert("count".to_string(), Value::I64(count));
    Ok(value_map(entries))
}

fn require_user(mode: &str, what: &str) -> Result<(), Error> {
    match mode {
        "false" | "anonymous-all" | "anonymous-view" => Ok(()),
        _ => {
            if crate::user::current_username()?.is_none() {
                Err(Error::of_kind(
                    ErrorKind::AuthenticationRequired,
                    format!("User must be logged in to {what}"),
                ))
            } else {
                Ok(())
            }
        }
    }
}

fn current_facade() -> Result<EntityFacade, Error> {
    ExecutionContext::with_current(|ctx| ctx.factory().entity())
        .flatten()
        .ok_or_else(|| Error::new("no entity facade is installed"))
}

fn value_map(entries: BTreeMap<String, Value>) -> Value {
    let stack = moqui_lib::context_stack::ContextStack::new();
    let frame = stack.shared_map();
    {
        let mut map = frame.borrow_mut();
        for (key, value) in entries {
            map.insert(key, value);
        }
    }
    drop(stack);
    unwrap_frame(frame)
}

fn unwrap_frame(frame: moqui_lib::context_stack::Frame) -> Value {
    Value::Map(match Rc::try_unwrap(frame) {
        Ok(cell) => cell.into_inner(),
        Err(frame) => frame.borrow().clone(),
    })
}

/// OpenAPI 3.1 for one service REST root. The caller strips `.json` and rejects YAML.
///
/// As Java's Swagger for a service method, the path parameters, then the other in-parameters as query parameters for
/// a method with no body (not those that are objects or arrays) or the in-parameters as the JSON body of a `post`,
/// `put` or `patch`, and the out-parameters as the response. A parameter with `entity-name` and `field-name` lists the
/// ids of that field's `Enumeration` or `StatusItem` type in `enum`, as `RestSchemaUtil.addParameterEnums`.
///
/// # Errors
///
/// Returns an error when the root name is missing or that root is not loaded.
pub(crate) fn openapi_document(segments: &[String]) -> Result<Value, Error> {
    if segments.is_empty() {
        return Err(Error::of_kind(
            ErrorKind::BadRequest,
            "No root resource name specified in path",
        ));
    }
    let roots = current_roots()?;
    let root = roots.resources.get(&segments[0]).ok_or_else(|| {
        Error::of_kind(
            ErrorKind::NotFound,
            format!("Service REST API root resource not found with name {}", segments[0]),
        )
    })?;
    let facade = current_facade().ok();
    let mut schemas = Schemas {
        facade: facade.as_ref(),
        enums: facade.as_ref().map(crate::entity::FieldEnums::new),
    };
    let mut paths = IndexMap::new();
    collect_paths(root, "", &segments[1..], &[], &mut schemas, &mut paths);
    let mut info = IndexMap::new();
    info.insert("title".to_string(), Value::string(&segments[0]));
    info.insert("version".to_string(), Value::string("1.0"));
    let mut server = IndexMap::new();
    server.insert("url".to_string(), Value::string("/rest/s1"));
    let mut doc = IndexMap::new();
    doc.insert("openapi".to_string(), Value::string("3.1.0"));
    doc.insert("info".to_string(), Value::Map(info));
    doc.insert("servers".to_string(), Value::List(vec![Value::Map(server)]));
    doc.insert("paths".to_string(), Value::Map(paths));
    Ok(Value::Map(doc))
}

/// What the parameter schemas of one document read: the entity model and the enumeration ids.
struct Schemas<'a> {
    facade: Option<&'a EntityFacade>,
    enums: Option<crate::entity::FieldEnums<'a>>,
}

fn collect_paths(
    node: &Node,
    prefix: &str,
    descend: &[String],
    path_params: &[String],
    schemas: &mut Schemas<'_>,
    paths: &mut IndexMap<String, Value>,
) {
    let here = join_path(prefix, node);
    let mut path_params = path_params.to_vec();
    if node.is_id {
        path_params.push(node.name.clone());
    }
    if let Some((next, rest)) = descend.split_first() {
        if let Some(child) = node.resources.get(next) {
            collect_paths(child, &here, rest, &path_params, schemas, paths);
            return;
        }
        if let Some(id) = &node.id {
            collect_paths(id, &here, rest, &path_params, schemas, paths);
        }
        return;
    }
    if !node.methods.is_empty() {
        paths.insert(here.clone(), method_object(&node.methods, &path_params, schemas));
    }
    if let Some(id) = &node.id {
        collect_paths(id, &here, &[], &path_params, schemas, paths);
    }
    for child in node.resources.values() {
        collect_paths(child, &here, &[], &path_params, schemas, paths);
    }
}

fn join_path(prefix: &str, node: &Node) -> String {
    let segment = if node.is_id {
        format!("{{{}}}", node.name)
    } else {
        node.name.clone()
    };
    if prefix.is_empty() {
        format!("/{segment}")
    } else {
        format!("{prefix}/{segment}")
    }
}

fn method_object(methods: &BTreeMap<String, Operation>, path_params: &[String], schemas: &mut Schemas<'_>) -> Value {
    let mut map = IndexMap::new();
    for (verb, operation) in methods {
        map.insert(verb.clone(), operation_object(verb, operation, path_params, schemas));
    }
    Value::Map(map)
}

fn operation_object(verb: &str, operation: &Operation, path_params: &[String], schemas: &mut Schemas<'_>) -> Value {
    let summary = match operation {
        Operation::Service { name, .. } => name.clone(),
        Operation::Entity { name, operation, .. } => format!("{operation} {name}"),
    };
    let mut success = IndexMap::new();
    success.insert("description".to_string(), Value::string("Success"));
    let mut denied = IndexMap::new();
    denied.insert("description".to_string(), Value::string("Authentication required"));
    let mut map = IndexMap::new();
    map.insert("summary".to_string(), Value::string(summary));
    if let Operation::Service { name, .. } = operation {
        service_parameters(name, verb, path_params, schemas, &mut map, &mut success);
    }
    let mut responses = IndexMap::new();
    responses.insert("200".to_string(), Value::Map(success));
    responses.insert("401".to_string(), Value::Map(denied));
    map.insert("responses".to_string(), Value::Map(responses));
    Value::Map(map)
}

/// The `parameters`, `requestBody` and response `content` of a service method. A service that is not defined, or
/// whose parameters do not expand, is logged and documented without them; Java's Swagger fails for it.
fn service_parameters(
    name: &str,
    verb: &str,
    path_params: &[String],
    schemas: &mut Schemas<'_>,
    operation: &mut IndexMap<String, Value>,
    success: &mut IndexMap<String, Value>,
) {
    let Some(def) = super::define::find(name) else {
        tracing::error!(
            service = name,
            "REST service not found, its parameters are left out of the OpenAPI"
        );
        return;
    };
    let in_params = match super::validate::declared_in(&def, schemas.facade) {
        Ok(params) => params,
        Err(err) => {
            tracing::error!(service = name, error = %err, "REST service parameters left out of the OpenAPI");
            return;
        }
    };
    let in_params: Vec<&ParamDef> = in_params
        .iter()
        .filter(|param| !param.name.is_empty() && param.required != Required::Disabled)
        .collect();
    let mut parameters = Vec::new();
    for path_param in path_params {
        let found = in_params.iter().find(|param| &param.name == path_param);
        let mut schema = IndexMap::new();
        schema.insert(
            "type".to_string(),
            Value::string(json_type(found.map_or("", |param| param.java_type.as_str()))),
        );
        if let Some(description) = found.and_then(|param| param.description.as_deref()) {
            schema.insert("description".to_string(), Value::string(description));
        }
        parameters.push(parameter_object(path_param, "path", true, schema));
    }
    let remaining: Vec<&ParamDef> = in_params
        .iter()
        .copied()
        .filter(|param| !path_params.contains(&param.name))
        .collect();
    if !remaining.is_empty() {
        if matches!(verb, "post" | "put" | "patch") {
            let mut json = IndexMap::new();
            json.insert("schema".to_string(), params_schema(&in_params, schemas));
            let mut content = IndexMap::new();
            content.insert("application/json".to_string(), Value::Map(json));
            let mut body = IndexMap::new();
            body.insert("required".to_string(), Value::Bool(true));
            body.insert("content".to_string(), Value::Map(content));
            operation.insert("requestBody".to_string(), Value::Map(body));
        } else {
            for param in remaining {
                // Query parameters, so no objects or arrays, as Java.
                if matches!(json_type(&param.java_type), "object" | "array") {
                    continue;
                }
                let schema = param_schema(param, schemas);
                parameters.push(parameter_object(&param.name, "query", false, schema));
            }
        }
    }
    if !parameters.is_empty() {
        operation.insert("parameters".to_string(), Value::List(parameters));
    }
    let out_params: Vec<&ParamDef> = def
        .out_params
        .iter()
        .filter(|param| !param.name.is_empty() && param.required != Required::Disabled)
        .collect();
    if !out_params.is_empty() {
        let mut json = IndexMap::new();
        json.insert("schema".to_string(), params_schema(&out_params, schemas));
        let mut content = IndexMap::new();
        content.insert("application/json".to_string(), Value::Map(json));
        success.insert("content".to_string(), Value::Map(content));
    }
}

fn parameter_object(name: &str, location: &str, required: bool, schema: IndexMap<String, Value>) -> Value {
    let mut map = IndexMap::new();
    map.insert("name".to_string(), Value::string(name));
    map.insert("in".to_string(), Value::string(location));
    map.insert("required".to_string(), Value::Bool(required));
    map.insert("schema".to_string(), Value::Map(schema));
    Value::Map(map)
}

/// Java's `getJsonSchemaMapIn` and `getJsonSchemaMapOut`.
fn params_schema(params: &[&ParamDef], schemas: &mut Schemas<'_>) -> Value {
    let mut properties = IndexMap::new();
    let mut required = Vec::new();
    for param in params {
        if param.required == Required::True {
            required.push(Value::string(&param.name));
        }
        properties.insert(param.name.clone(), Value::Map(param_schema(param, schemas)));
    }
    let mut map = IndexMap::new();
    map.insert("type".to_string(), Value::string("object"));
    map.insert("properties".to_string(), Value::Map(properties));
    if !required.is_empty() {
        map.insert("required".to_string(), Value::List(required));
    }
    Value::Map(map)
}

/// Java's `getJsonSchemaPropMap`: the type and format, the description, the default, the first nested parameter as an
/// array's items or the nested parameters as an object's properties, and otherwise the enumeration ids.
fn param_schema(param: &ParamDef, schemas: &mut Schemas<'_>) -> IndexMap<String, Value> {
    let json_type = json_type(&param.java_type);
    let mut map = IndexMap::new();
    map.insert("type".to_string(), Value::string(json_type));
    if let Some(format) = json_format(&param.java_type) {
        map.insert("format".to_string(), Value::string(format));
    }
    if let Some(description) = param.description.as_deref() {
        map.insert("description".to_string(), Value::string(description));
    }
    if let Some(default) = param.default_value.as_deref().filter(|text| !text.is_empty()) {
        map.insert("default".to_string(), Value::string(default));
    }
    if let Some(default) = param.default_expr.as_deref().filter(|text| !text.is_empty()) {
        map.insert("default".to_string(), Value::string(format!("{{{default}}}")));
    }
    match json_type {
        "array" => {
            if let Some(child) = param.children.first() {
                map.insert("items".to_string(), Value::Map(param_schema(child, schemas)));
            }
        }
        "object" => {
            if !param.children.is_empty() {
                let properties = param
                    .children
                    .iter()
                    .map(|child| (child.name.clone(), Value::Map(param_schema(child, schemas))))
                    .collect();
                map.insert("properties".to_string(), Value::Map(properties));
            }
        }
        _ => {
            if let Some(ids) = parameter_enums(param, schemas) {
                map.insert("enum".to_string(), Value::List(ids));
            }
        }
    }
    map
}

/// Java's `addParameterEnums`. An entity or field that is not defined is logged and the parameter has no `enum`; Java
/// fails the document.
fn parameter_enums(param: &ParamDef, schemas: &mut Schemas<'_>) -> Option<Vec<Value>> {
    let entity_name = param.entity_name.as_deref().filter(|name| !name.is_empty())?;
    let field_name = param.field_name.as_deref().filter(|name| !name.is_empty())?;
    let (Some(facade), Some(enums)) = (schemas.facade, schemas.enums.as_mut()) else {
        return None;
    };
    let found = facade.with_entity(entity_name, |entity, _, _| {
        Ok(entity.has_field(field_name).then(|| enums.of(entity, field_name)))
    });
    match found {
        Ok(Some(ids)) => ids,
        _ => {
            tracing::error!(
                parameter = %param.name,
                "Field {field_name} of entity {entity_name} not found, the parameter has no enum in the OpenAPI"
            );
            None
        }
    }
}

/// Java's `RestApi.getJsonType`, by the class name after the last `.`.
fn json_type(java_type: &str) -> &'static str {
    match java_type.rsplit('.').next().unwrap_or("") {
        "Integer" | "Long" | "Short" | "BigInteger" => "integer",
        "Float" | "Double" | "BigDecimal" => "number",
        "Boolean" => "boolean",
        "List" | "Set" | "Collection" | "EntityList" => "array",
        "Map" | "EntityValue" => "object",
        _ => "string",
    }
}

/// Java's `RestApi.getJsonFormat`.
fn json_format(java_type: &str) -> Option<&'static str> {
    match java_type.rsplit('.').next().unwrap_or("") {
        "Integer" | "Short" => Some("int32"),
        "Long" | "BigInteger" => Some("int64"),
        "Float" => Some("float"),
        "Double" => Some("double"),
        "Date" => Some("date"),
        "Timestamp" => Some("date-time"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use moqui_lib::object::Value;

    use super::{install, run};
    use crate::context::ExecutionContextFactory;
    use crate::entity::EntityFacade;

    #[test]
    fn basic_enum_types_returns_fixture_rows_and_a_service_runs() {
        let dir = std::env::temp_dir().join(format!("moqui-rest-{}", std::process::id()));
        let _guard = DirGuard(dir.clone());
        std::fs::create_dir_all(dir.join("service")).unwrap();
        std::fs::write(dir.join("service/moqui.rest.xml"), REST_XML).unwrap();
        let service_path = dir.join("service/echo.xml");
        std::fs::write(&service_path, SERVICE_XML).unwrap();
        let facade = crate::testing::open();
        let ctx = ExecutionContextFactory::test_context();
        install(ctx.factory(), &[dir.join("service/moqui.rest.xml")]);
        ctx.factory().set_entity(facade.clone());
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&service_path)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        insert_type(&facade, "DayOfWeek", "Day of Week");
        insert_type(&facade, "GeoType", "Geo Type");

        let listed = run(&["moqui", "basic", "enumTypes"], "get", &BTreeMap::new()).unwrap();
        assert_eq!(row_count(&listed), 2, "{listed:?}");
        assert_eq!(field(&listed, "DayOfWeek"), Some("Day of Week"));
        assert_eq!(field(&listed, "GeoType"), Some("Geo Type"));

        let one = run(&["moqui", "basic", "enumTypes", "GeoType"], "GET", &BTreeMap::new()).unwrap();
        let Value::Map(one_map) = one else {
            panic!("one result {one:?}");
        };
        assert_eq!(one_map.get("enumTypeId").and_then(Value::as_str), Some("GeoType"));
        assert_eq!(one_map.get("description").and_then(Value::as_str), Some("Geo Type"));

        let mut params = BTreeMap::new();
        params.insert("note".to_string(), Value::string("hello"));
        let echoed = run(&["moqui", "basic", "echo"], "post", &params).unwrap();
        let Value::Map(echoed_map) = echoed else {
            panic!("service result {echoed:?}");
        };
        assert_eq!(echoed_map.get("echoed").and_then(Value::as_str), Some("hello"));

        // The method comes from the request, so a parameter named _method changes nothing.
        let mut sneaky = BTreeMap::new();
        sneaky.insert("_method".to_string(), Value::string("post"));
        let err = run(&["moqui", "basic", "echo"], "get", &sneaky).unwrap_err();
        assert_eq!(
            crate::context::ErrorKind::of(&err),
            crate::context::ErrorKind::MethodNotAllowed,
            "{err}"
        );
        let err = run(&["moqui", "basic", "nothing"], "get", &BTreeMap::new()).unwrap_err();
        assert_eq!(
            crate::context::ErrorKind::of(&err),
            crate::context::ErrorKind::NotFound,
            "{err}"
        );
        let err = run(&["nope"], "get", &BTreeMap::new()).unwrap_err();
        assert_eq!(
            crate::context::ErrorKind::of(&err),
            crate::context::ErrorKind::NotFound,
            "{err}"
        );
    }

    /// As Java's `RestApi`: identity-admin entities are refused, a `UserAccount` write loses the password fields and,
    /// for a caller who is not ADMIN, the identity fields, secret-config writes need ADMIN, and results never carry
    /// the password fields.
    #[test]
    fn entity_resources_keep_sensitive_entities_and_fields_out() {
        let dir = std::env::temp_dir().join(format!("moqui-rest-sensitive-{}", std::process::id()));
        let _guard = DirGuard(dir.clone());
        std::fs::create_dir_all(dir.join("service")).unwrap();
        std::fs::write(
            dir.join("service/sec.rest.xml"),
            r#"<resource name="sec" require-authentication="false">
                <resource name="keys"><method type="get"><entity name="moqui.security.UserLoginKey" operation="list"/></method></resource>
                <resource name="users">
                    <method type="get"><entity name="moqui.security.UserAccount" operation="list"/></method>
                    <method type="post"><entity name="moqui.security.UserAccount" operation="create"/></method>
                </resource>
                <resource name="servers"><method type="post"><entity name="moqui.basic.email.EmailServer" operation="create"/></method></resource>
            </resource>"#,
        )
        .unwrap();
        let facade = crate::testing::open();
        let ctx = ExecutionContextFactory::test_context();
        install(ctx.factory(), &[dir.join("service/sec.rest.xml")]);
        ctx.factory().set_entity(facade.clone());
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        create(
            &facade,
            "moqui.security.UserAccount",
            &[("userId", "U1"), ("username", "u1"), ("currentPassword", "hash")],
        );
        let unauthorized =
            |err: &crate::context::Error| crate::context::ErrorKind::of(err) == crate::context::ErrorKind::Unauthorized;

        let err = run(&["sec", "keys"], "get", &BTreeMap::new()).unwrap_err();
        assert!(unauthorized(&err), "{err}");

        let listed = run(&["sec", "users"], "get", &BTreeMap::new()).unwrap();
        let Value::List(rows) = &listed else {
            panic!("list {listed:?}")
        };
        let Value::Map(row) = &rows[0] else {
            panic!("row {listed:?}")
        };
        assert_eq!(row.get("userId").and_then(Value::as_str), Some("U1"));
        assert!(!row.contains_key("currentPassword"), "{row:?}");

        let mut params = BTreeMap::new();
        params.insert("userId".to_string(), Value::string("U2"));
        params.insert("username".to_string(), Value::string("u2"));
        params.insert("currentPassword".to_string(), Value::string("chosen"));
        run(&["sec", "users"], "post", &params).unwrap();
        let stored = facade
            .find("moqui.security.UserAccount")
            .unwrap()
            .eq("userId", Value::string("U2"))
            .one()
            .unwrap()
            .expect("created");
        assert!(stored.get("currentPassword").is_none_or(Value::is_null));
        assert!(
            stored.get("username").is_none_or(Value::is_null),
            "identity fields need ADMIN"
        );

        let mut server = BTreeMap::new();
        server.insert("emailServerId".to_string(), Value::string("E1"));
        let err = run(&["sec", "servers"], "post", &server).unwrap_err();
        assert!(unauthorized(&err), "{err}");
    }

    #[test]
    fn service_metadata_resolves_named_resources_ids_and_extra_paths_without_executing() {
        let dir = std::env::temp_dir().join(format!("moqui-rest-metadata-{}", std::process::id()));
        let _guard = DirGuard(dir.clone());
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("api.rest.xml");
        std::fs::write(&path, r#"<resource name="api"><resource name="items">
            <resource name="named"><method type="get"><service name="literal#Named"/></method></resource>
            <id name="itemId"><resource name="detail"><method type="post"><service name="literal#Detail"/></method></resource></id>
            <method type="get"><entity name="moqui.basic.Enumeration" operation="list"/></method>
            </resource><resource name="browse"><id name="path" allow-extra-path="true">
            <method type="get"><service name="literal#Browse"/></method></id></resource></resource>"#).unwrap();
        let context = ExecutionContextFactory::test_context();
        install(context.factory(), &[path]);
        let _entered = context.enter();
        for (path, method, expected, parameters) in [
            ("api/items/named", "GET", "literal#Named", vec![]),
            (
                "/api/items/123/detail/",
                "post",
                "literal#Detail",
                vec![Value::string("itemId")],
            ),
            ("api/browse", "get", "literal#Browse", vec![Value::string("path")]),
            ("api/browse/a/b", "get", "literal#Browse", vec![Value::string("path")]),
        ] {
            let Value::Map(result) = super::resolve_service(path, method).unwrap() else {
                panic!("map");
            };
            assert_eq!(result["serviceName"].as_str(), Some(expected));
            assert_eq!(result["pathParameters"], Value::List(parameters));
            assert_eq!(result["method"], Value::string(method.to_ascii_lowercase()));
            assert_eq!(
                result["path"],
                Value::string(format!("/rest/s1/{}", path.trim_matches('/')))
            );
        }
        for path in ["", "missing", "api/items", "api/items/123", "api/items/named/missing"] {
            assert!(super::resolve_service(path, "get").is_err(), "{path}");
        }
        assert!(super::resolve_service("api/items/named", "post").is_err());
        assert!(crate::script::eval_condition(
            r#"ec.service.restApi.resolveService("api/items/123/detail", "post").pathParameters[0] === "itemId""#,
        )
        .unwrap());
    }

    #[test]
    fn a_login_is_required_unless_the_resource_waives_it() {
        let dir = std::env::temp_dir().join(format!("moqui-rest-auth-{}", std::process::id()));
        let _guard = DirGuard(dir.clone());
        std::fs::create_dir_all(dir.join("service")).unwrap();
        std::fs::write(
            dir.join("service/secure.rest.xml"),
            r#"<resource name="secure"><resource name="items">
                <method type="get"><entity name="moqui.basic.EnumerationType" operation="list"/></method>
            </resource></resource>"#,
        )
        .unwrap();
        std::fs::write(dir.join("service/broken.rest.xml"), "<resource name=").unwrap();
        let ctx = ExecutionContextFactory::test_context();
        ctx.factory().set_entity(crate::testing::open());
        // A broken file beside it does not stop the good one from being served.
        let mut loaded = BTreeMap::new();
        super::load_dir(&mut loaded, &dir.join("service"));
        assert_eq!(loaded.keys().collect::<Vec<_>>(), ["secure"]);
        install(ctx.factory(), &[dir.join("service/secure.rest.xml")]);
        let _entered = ctx.enter();
        let err = run(&["secure", "items"], "get", &BTreeMap::new()).unwrap_err();
        assert_eq!(
            crate::context::ErrorKind::of(&err),
            crate::context::ErrorKind::AuthenticationRequired,
            "{err}"
        );
    }

    /// The `AT_REST_PATH` frames: an ALWAYS grant on the root path flows to the service, a user without one is
    /// refused at the path, and a service that needs no login is still behind the path's authorization.
    #[test]
    fn rest_path_authz_gates_and_inherits() {
        let dir = std::env::temp_dir().join(format!("moqui-rest-path-{}", std::process::id()));
        let _guard = DirGuard(dir.clone());
        std::fs::create_dir_all(dir.join("service")).unwrap();
        std::fs::write(
            dir.join("service/moqui.rest.xml"),
            r#"<resource name="moqui"><resource name="basic">
                <resource name="echo"><method type="post"><service name="gate.ping#Rest"/></method></resource>
                <resource name="secured"><method type="post"><service name="gate.secure#Rest"/></method></resource>
            </resource></resource>"#,
        )
        .unwrap();
        let service_path = dir.join("service/gate.xml");
        std::fs::write(
            &service_path,
            r#"<services>
                <service verb="ping" noun="Rest" authenticate="false">
                    <out-parameters><parameter name="echoed"/></out-parameters>
                    <actions><script>echoed = "anonymous";</script></actions>
                </service>
                <service verb="secure" noun="Rest">
                    <out-parameters><parameter name="echoed"/></out-parameters>
                    <actions><script>echoed = "secured";</script></actions>
                </service>
            </services>"#,
        )
        .unwrap();
        let facade = crate::testing::open();
        let ctx = ExecutionContextFactory::test_context_with_conf(
            r#"<moqui-conf><artifact-execution-facade>
                <artifact-execution type="AT_REST_PATH" authz-enabled="true"/>
                <artifact-execution type="AT_SERVICE" authz-enabled="true"/>
                <artifact-execution type="AT_ENTITY" authz-enabled="true"/>
            </artifact-execution-facade></moqui-conf>"#,
        );
        install(ctx.factory(), &[dir.join("service/moqui.rest.xml")]);
        ctx.factory().set_entity(facade.clone());
        let _entered = ctx.enter();
        {
            let _authz = ctx.disable_authz();
            let failures = crate::service::lint(std::slice::from_ref(&service_path)).unwrap();
            assert!(failures.is_empty(), "{failures:?}");
            for (user, group) in [("pathadmin", Some("ADMIN")), ("pathplain", None)] {
                create(
                    &facade,
                    "moqui.security.UserAccount",
                    &[("userId", user), ("username", user)],
                );
                if let Some(group) = group {
                    let mut member = facade.make_value("moqui.security.UserGroupMember").unwrap();
                    member.set("userId", Value::string(user));
                    member.set("userGroupId", Value::string(group));
                    member.set(
                        "fromDate",
                        moqui_lib::object::basic_convert(&Value::I64(1_000), "java.sql.Timestamp").value,
                    );
                    member.create(&facade).unwrap();
                }
            }
            create(
                &facade,
                "moqui.security.ArtifactGroup",
                &[("artifactGroupId", "MOQUI_API")],
            );
            create(
                &facade,
                "moqui.security.ArtifactGroupMember",
                &[
                    ("artifactGroupId", "MOQUI_API"),
                    ("artifactName", "/moqui"),
                    ("artifactTypeEnumId", "AT_REST_PATH"),
                    ("nameIsPattern", "N"),
                    ("inheritAuthz", "Y"),
                ],
            );
            create(
                &facade,
                "moqui.security.ArtifactAuthz",
                &[
                    ("artifactAuthzId", "MOQUI_API_ADMIN"),
                    ("userGroupId", "ADMIN"),
                    ("artifactGroupId", "MOQUI_API"),
                    ("authzTypeEnumId", "AUTHZT_ALWAYS"),
                    ("authzActionEnumId", "AUTHZA_ALL"),
                ],
            );
        }
        let echoed = |value: Value| match value {
            Value::Map(map) => map.get("echoed").and_then(Value::as_str).unwrap_or("").to_string(),
            other => panic!("service result {other:?}"),
        };
        let kind = |err: &crate::context::Error| crate::context::ErrorKind::of(err);
        let none = BTreeMap::new();

        // No one logged in: the path asks for a login, also for the service that needs none.
        let err = run(&["moqui", "basic", "echo"], "post", &none).unwrap_err();
        assert_eq!(kind(&err), crate::context::ErrorKind::AuthenticationRequired, "{err}");

        // ADMIN has no grant on the service itself; the ALWAYS grant on /moqui is inherited.
        crate::user::internal_login_user("pathadmin").unwrap();
        let result = run(&["moqui", "basic", "secured"], "post", &none).unwrap();
        assert_eq!(echoed(result), "secured");
        let result = run(&["moqui", "basic", "echo"], "post", &none).unwrap();
        assert_eq!(echoed(result), "anonymous");
        crate::user::logout().unwrap();

        // A user outside ADMIN is refused at the path, before the service.
        crate::user::internal_login_user("pathplain").unwrap();
        for leaf in ["secured", "echo"] {
            let err = run(&["moqui", "basic", leaf], "post", &none).unwrap_err();
            assert_eq!(kind(&err), crate::context::ErrorKind::Unauthorized, "{leaf}: {err}");
            assert!(err.message().contains("AT_REST_PATH"), "{leaf}: {err}");
        }
        crate::user::logout().unwrap();
    }

    /// As Java's `RestResult` and `nestedRemoveNullsFromResultMap`: a service's out-parameters lose the `UserAccount`
    /// password fields at any depth, the null out-parameters, and the null fields one map down.
    #[test]
    fn service_results_lose_user_account_secrets_and_nulls() {
        let dir = std::env::temp_dir().join(format!("moqui-rest-secrets-{}", std::process::id()));
        let _guard = DirGuard(dir.clone());
        std::fs::create_dir_all(dir.join("service")).unwrap();
        std::fs::write(
            dir.join("service/acct.rest.xml"),
            r#"<resource name="acct" require-authentication="false">
                <resource name="accounts"><method type="get"><service name="acct.get#Accounts"/></method></resource>
            </resource>"#,
        )
        .unwrap();
        let service_path = dir.join("service/acct.xml");
        std::fs::write(
            &service_path,
            r#"<services>
                <service verb="get" noun="Accounts" authenticate="false">
                    <out-parameters>
                        <parameter name="account" type="Map"/>
                        <parameter name="accounts" type="List"/>
                        <parameter name="nested" type="Map"/>
                        <parameter name="nothing"/>
                    </out-parameters>
                    <actions><script>
                        account = {userId: "U1", currentPassword: "hash", passwordSalt: "salt", note: null};
                        accounts = [{userId: "U2", resetPassword: "reset", passwordHashType: "SHA", note: null}];
                        nested = {party: {users: [{userId: "U3", passwordBase64: "b64"}], kept: null}, gone: null};
                        nothing = null;
                    </script></actions>
                </service>
            </services>"#,
        )
        .unwrap();
        let ctx = ExecutionContextFactory::test_context();
        install(ctx.factory(), &[dir.join("service/acct.rest.xml")]);
        ctx.factory().set_entity(crate::testing::open());
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&service_path)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");

        let result = run(&["acct", "accounts"], "get", &BTreeMap::new()).unwrap();
        let Value::Map(result) = &result else {
            panic!("service result {result:?}")
        };
        assert!(!result.contains_key("nothing"), "{result:?}");
        let Some(Value::Map(account)) = result.get("account") else {
            panic!("account {result:?}")
        };
        assert_eq!(account.keys().collect::<Vec<_>>(), ["userId"], "{account:?}");
        let Some(Value::List(accounts)) = result.get("accounts") else {
            panic!("accounts {result:?}")
        };
        let Value::Map(listed) = &accounts[0] else {
            panic!("listed {accounts:?}")
        };
        assert_eq!(listed.keys().collect::<Vec<_>>(), ["userId"], "{listed:?}");
        let Some(Value::Map(nested)) = result.get("nested") else {
            panic!("nested {result:?}")
        };
        assert!(!nested.contains_key("gone"), "{nested:?}");
        let Some(Value::Map(party)) = nested.get("party") else {
            panic!("party {nested:?}")
        };
        // Only one map down, as Java: a null deeper in stays.
        assert_eq!(party.get("kept"), Some(&Value::Null), "{party:?}");
        let Some(Value::List(users)) = party.get("users") else {
            panic!("users {party:?}")
        };
        let Value::Map(user) = &users[0] else {
            panic!("user {users:?}")
        };
        assert_eq!(user.keys().collect::<Vec<_>>(), ["userId"], "{user:?}");
    }

    /// As Java's `PathNode`: `description` is an attribute, `childPaths` counts the resources and ids under a node, an
    /// empty `require-authentication` inherits, a method is keyed by `type` as written, and a method that does not load
    /// is left out of a file that is otherwise served.
    #[test]
    fn rest_files_read_as_java_reads_them() {
        let dir = std::env::temp_dir().join(format!("moqui-rest-read-{}", std::process::id()));
        let _guard = DirGuard(dir.clone());
        std::fs::create_dir_all(dir.join("service")).unwrap();
        std::fs::write(
            dir.join("service/read.rest.xml"),
            r#"<resource name="read" displayName="Read API" description="Reads things" require-authentication="false">
                <resource name="types" require-authentication="">
                    <method type="get"><entity name="moqui.basic.EnumerationType" operation="list"/></method>
                    <method type="post"><other name="nothing"/></method>
                    <method><entity name="moqui.basic.EnumerationType" operation="list"/></method>
                    <id name="enumTypeId"><resource name="detail"/></id>
                </resource>
                <resource name="shout"><method type="GET"><entity name="moqui.basic.EnumerationType" operation="list"/></method></resource>
            </resource>"#,
        )
        .unwrap();
        let ctx = ExecutionContextFactory::test_context();
        ctx.factory().set_entity(crate::testing::open());
        let mut loaded = BTreeMap::new();
        super::load_dir(&mut loaded, &dir.join("service"));
        let Value::Map(info) = super::resource_info(&loaded["read"], true) else {
            panic!("info")
        };
        assert_eq!(info["description"], Value::string("Reads things"));
        assert_eq!(info["displayName"], Value::string("Read API"));
        assert_eq!(info["childPaths"], Value::I64(4), "types, its id and detail, and shout");
        assert_eq!(info["childMethods"], Value::I64(2), "{info:?}");
        install(ctx.factory(), &[dir.join("service/read.rest.xml")]);
        let _entered = ctx.enter();
        let kind = |err: &crate::context::Error| crate::context::ErrorKind::of(err);
        // No login: the empty value took the root's `false`.
        assert!(run(&["read", "types"], "get", &BTreeMap::new()).is_ok());
        let err = run(&["read", "types"], "post", &BTreeMap::new()).unwrap_err();
        assert_eq!(kind(&err), crate::context::ErrorKind::MethodNotAllowed, "{err}");
        // The request's method is lower case, so `type="GET"` is never matched.
        let err = run(&["read", "shout"], "GET", &BTreeMap::new()).unwrap_err();
        assert_eq!(kind(&err), crate::context::ErrorKind::MethodNotAllowed, "{err}");
    }

    /// The OpenAPI of a service method: its path parameter, its other in-parameters as the query of a `get` or the
    /// JSON body of a `post`, its out-parameters as the response, and the `enum` of a parameter on an `Enumeration`
    /// field, as Java's Swagger.
    #[test]
    fn openapi_lists_service_parameters_with_their_descriptions_and_enumeration_ids() {
        let dir = std::env::temp_dir().join(format!("moqui-rest-openapi-{}", std::process::id()));
        let _guard = DirGuard(dir.clone());
        std::fs::create_dir_all(dir.join("service")).unwrap();
        std::fs::write(
            dir.join("service/feeds.rest.xml"),
            r#"<resource name="feeds"><id name="dataFeedId">
                <method type="get"><service name="feeds.get#Feed"/></method>
                <method type="post"><service name="feeds.get#Feed"/></method>
            </id></resource>"#,
        )
        .unwrap();
        let service_path = dir.join("service/feeds.xml");
        std::fs::write(
            &service_path,
            r#"<services>
                <service verb="get" noun="Feed" authenticate="false">
                    <in-parameters>
                        <parameter name="dataFeedId" required="true"><description> The feed </description></parameter>
                        <parameter name="dataFeedTypeEnumId" entity-name="moqui.entity.feed.DataFeed" field-name="dataFeedTypeEnumId"/>
                        <parameter name="count" type="Integer" default-value="10"><description>How many</description></parameter>
                        <parameter name="options" type="Map"/>
                    </in-parameters>
                    <out-parameters><parameter name="feed" type="Map"/></out-parameters>
                    <actions><script>feed = {};</script></actions>
                </service>
            </services>"#,
        )
        .unwrap();
        let facade = crate::testing::open();
        for (id, sequence) in [("DTFDTP_MAN_PULL", 2), ("DTFDTP_RT_PUSH", 1)] {
            let mut row = facade.make_value("moqui.basic.Enumeration").unwrap();
            row.set("enumId", Value::string(id));
            row.set("enumTypeId", Value::string("DataFeedType"));
            row.set("sequenceNum", Value::I64(sequence));
            row.create(&facade).unwrap();
        }
        let ctx = ExecutionContextFactory::test_context();
        install(ctx.factory(), &[dir.join("service/feeds.rest.xml")]);
        ctx.factory().set_entity(facade);
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&service_path)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");

        let doc = super::openapi_document(&["feeds".to_string()]).unwrap();
        let at = |value: &Value, path: &[&str]| -> Value {
            path.iter().fold(value.clone(), |current, key| match current {
                Value::Map(map) => map.get(*key).cloned().unwrap_or_else(|| panic!("no {key} in {map:?}")),
                other => panic!("not a map at {key}: {other:?}"),
            })
        };
        let ids = Value::List(vec![Value::string("DTFDTP_RT_PUSH"), Value::string("DTFDTP_MAN_PULL")]);
        let get = at(&doc, &["paths", "/feeds/{dataFeedId}", "get"]);
        let Value::List(parameters) = at(&get, &["parameters"]) else {
            panic!("parameters {get:?}")
        };
        let named = |name: &str| {
            parameters
                .iter()
                .find(|item| matches!(item, Value::Map(map) if map["name"] == Value::string(name)))
                .cloned()
        };
        let path = named("dataFeedId").expect("path parameter");
        assert_eq!(at(&path, &["in"]), Value::string("path"));
        assert_eq!(at(&path, &["required"]), Value::Bool(true));
        assert_eq!(at(&path, &["schema", "description"]), Value::string("The feed"));
        let feed_type = named("dataFeedTypeEnumId").expect("query parameter");
        assert_eq!(at(&feed_type, &["in"]), Value::string("query"));
        assert_eq!(at(&feed_type, &["schema", "enum"]), ids);
        let count = named("count").expect("count");
        assert_eq!(at(&count, &["schema", "type"]), Value::string("integer"));
        assert_eq!(at(&count, &["schema", "format"]), Value::string("int32"));
        assert_eq!(at(&count, &["schema", "default"]), Value::string("10"));
        assert_eq!(at(&count, &["schema", "description"]), Value::string("How many"));
        assert!(
            !matches!(at(&feed_type, &["schema"]), Value::Map(map) if map.contains_key("description")),
            "no description, none in the schema"
        );
        assert!(named("options").is_none(), "an object is no query parameter");
        assert_eq!(
            at(
                &get,
                &[
                    "responses",
                    "200",
                    "content",
                    "application/json",
                    "schema",
                    "properties",
                    "feed",
                    "type"
                ]
            ),
            Value::string("object")
        );

        let post = at(&doc, &["paths", "/feeds/{dataFeedId}", "post"]);
        let body = at(&post, &["requestBody", "content", "application/json", "schema"]);
        assert_eq!(at(&body, &["properties", "dataFeedTypeEnumId", "enum"]), ids);
        assert_eq!(
            at(&body, &["properties", "count", "description"]),
            Value::string("How many")
        );
        assert_eq!(at(&body, &["required"]), Value::List(vec![Value::string("dataFeedId")]));
    }

    fn create(facade: &EntityFacade, entity: &str, pairs: &[(&str, &str)]) {
        let mut row = facade.make_value(entity).unwrap();
        for (name, value) in pairs {
            row.set(*name, Value::string(*value));
        }
        row.create(facade).unwrap();
    }

    fn insert_type(facade: &EntityFacade, id: &str, description: &str) {
        let mut row = facade.make_value("moqui.basic.EnumerationType").unwrap();
        row.set("enumTypeId", Value::string(id));
        row.set("description", Value::string(description));
        row.create(facade).unwrap();
    }

    fn row_count(value: &Value) -> usize {
        let Value::List(items) = value else {
            panic!("list result {value:?}");
        };
        items.len()
    }

    fn field<'a>(value: &'a Value, id: &str) -> Option<&'a str> {
        let Value::List(items) = value else {
            return None;
        };
        items.iter().find_map(|item| {
            let Value::Map(row) = item else {
                return None;
            };
            let found = row.get("enumTypeId").and_then(Value::as_str)?;
            if found == id {
                row.get("description").and_then(Value::as_str)
            } else {
                None
            }
        })
    }

    struct DirGuard(PathBuf);
    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const REST_XML: &str = r#"<resource name="moqui" require-authentication="false">
        <resource name="basic">
            <resource name="enumTypes">
                <method type="get"><entity name="moqui.basic.EnumerationType" operation="list"/></method>
                <id name="enumTypeId">
                    <method type="get"><entity name="moqui.basic.EnumerationType" operation="one"/></method>
                </id>
            </resource>
            <resource name="echo">
                <method type="post"><service name="echo.ping#Rest"/></method>
            </resource>
        </resource>
    </resource>"#;

    const SERVICE_XML: &str = r#"<services>
        <service verb="ping" noun="Rest" authenticate="false">
            <in-parameters><parameter name="note"/></in-parameters>
            <out-parameters><parameter name="echoed"/></out-parameters>
            <actions><script>echoed = note;</script></actions>
        </service>
    </services>"#;
}
