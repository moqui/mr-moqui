//! Process-wide coordinator and reentrant serial application lane. The host owns HTTP and JSONL I/O.
use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::ThreadId;

use moqui_lib::error::Error;
use serde_json::{Value, json};

pub trait Provider: Send + Sync {
    /// Returns an object containing a nonempty operation_id and an integer time_epoch_ms.
    fn begin(&self, request: Value) -> Result<Value, Error>;
    fn complete(&self, request: Value) -> Result<(), Error>;
    fn random(&self, _domain: &str, _length: usize) -> Result<Vec<u8>, Error> {
        Err(Error::new("deterministic provider does not support random bytes"))
    }
    fn trace(&self, _request: Value) -> Result<(), Error> {
        Err(Error::new("deterministic provider does not support operation tracing"))
    }
}

static PROVIDER: OnceLock<Arc<dyn Provider>> = OnceLock::new();
static SEQUENCE: AtomicU64 = AtomicU64::new(1);
static FAILED: AtomicBool = AtomicBool::new(false);
static REQUESTED: AtomicBool = AtomicBool::new(false);
static LANE: Mutex<(Option<ThreadId>, usize)> = Mutex::new((None, 0));
static WAKE: Condvar = Condvar::new();
thread_local! { static OPERATIONS: RefCell<Vec<(u64, i64, bool)>> = const { RefCell::new(Vec::new()) }; }

pub fn request() {
    REQUESTED.store(true, Ordering::SeqCst);
}

pub fn requested() -> bool {
    REQUESTED.load(Ordering::SeqCst) || std::env::var("MOQUI_DETERMINISTIC").is_ok_and(|flag| flag == "true")
}

pub fn enabled() -> bool {
    moqui_lib::clock::deterministic()
}

pub fn install(source: Arc<dyn Provider>) -> Result<(), Error> {
    PROVIDER
        .set(source)
        .map_err(|_| Error::new("deterministic provider already installed"))?;
    moqui_lib::random::install(Arc::new(|domain, length| {
        check()?;
        provider().random(domain, length).inspect_err(|_| fail())
    }))?;
    moqui_lib::clock::enable_deterministic();
    Ok(())
}

pub fn check() -> Result<(), Error> {
    if FAILED.load(Ordering::SeqCst) {
        Err(Error::new(
            "deterministic coordinator previously failed; restart required",
        ))
    } else {
        Ok(())
    }
}

fn fail() {
    FAILED.store(true, Ordering::SeqCst);
    moqui_lib::clock::fail();
}

pub fn validate_startup() -> Result<(), Error> {
    if requested() && !enabled() {
        return Err(Error::new(
            "deterministic mode requires the host coordinator provider before boot",
        ));
    }
    Ok(())
}

struct Lane;
impl Lane {
    fn enter() -> Self {
        let thread = std::thread::current().id();
        let mut lane = LANE.lock().unwrap();
        while lane.0.is_some_and(|owner| owner != thread) {
            lane = WAKE.wait(lane).unwrap();
        }
        lane.0 = Some(thread);
        lane.1 += 1;
        Self
    }
}
impl Drop for Lane {
    fn drop(&mut self) {
        let mut lane = LANE.lock().unwrap();
        lane.1 -= 1;
        if lane.1 == 0 {
            lane.0 = None;
            WAKE.notify_all();
        }
    }
}

/// Not Send: assigned time and lane ownership belong to the calling thread.
pub struct Operation {
    sequence: u64,
    operation_id: String,
    ended: bool,
    synthetic: bool,
    _lane: Lane,
    _thread: PhantomData<*const ()>,
}

pub fn begin() -> Result<Option<Operation>, Error> {
    if !enabled() {
        return Ok(None);
    }
    let lane = Lane::enter();
    check()?;
    let sequence = SEQUENCE.fetch_add(1, Ordering::SeqCst);
    let parent = OPERATIONS.with(|ops| ops.borrow().last().map(|op| op.0));
    let mut stack =
        crate::context::ExecutionContext::with_current(|ec| ec.deterministic_artifacts()).unwrap_or_default();
    if stack.is_empty() {
        stack.push(json!({"type":"AT_OTHER", "sub_type":"transaction", "name":"bootstrap"}));
    }
    let artifact = stack[0].clone();
    let request = json!({"sequence": sequence, "event":"begin", "parent_sequence": parent,
        "artifact": artifact, "artifact_stack": stack});
    let response = provider().begin(request).inspect_err(|_| fail())?;
    let operation_id = response
        .get("operation_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty());
    let millis = response.get("time_epoch_ms").and_then(Value::as_i64);
    let (Some(operation_id), Some(millis)) = (operation_id, millis) else {
        fail();
        return Err(Error::new("invalid deterministic begin response"));
    };
    if jiff::Timestamp::from_millisecond(millis).is_err() {
        fail();
        return Err(Error::new("deterministic time is out of range"));
    }
    OPERATIONS.with(|ops| ops.borrow_mut().push((sequence, millis, false)));
    sync_clock();
    Ok(Some(Operation {
        sequence,
        operation_id: operation_id.to_string(),
        ended: false,
        synthetic: false,
        _lane: lane,
        _thread: PhantomData,
    }))
}

/// An explicit synthetic bootstrap/nontransactional operation, only if no operation is assigned.
pub fn bootstrap() -> Result<Option<Operation>, Error> {
    check()?;
    if OPERATIONS.with(|ops| ops.borrow().last().is_some_and(|op| !op.2)) {
        return Ok(None);
    }
    let mut op = begin()?;
    if let Some(op) = &mut op {
        op.synthetic = true;
    }
    Ok(op)
}

/// Local evidence only: observations never acknowledge or skip a paired barrier.
pub fn trace(kind: &str, action: &str, name: &str, outcome: &str) -> Result<(), Error> {
    if !enabled() {
        return Ok(());
    }
    let (parent, time) = OPERATIONS.with(|ops| {
        let ops = ops.borrow();
        (
            ops.last().map(|op| op.0),
            ops.iter().rev().find(|op| !op.2).map(|op| op.1),
        )
    });
    provider()
        .trace(json!({"event":"trace", "kind":kind, "action":action, "name":name,
        "outcome":outcome, "parent_sequence":parent, "time_epoch_ms":time}))
        .inspect_err(|_| fail())
}

/// Record both attempts and actual outcomes, including early errors in the closure.
pub fn traced<T>(kind: &str, action: &str, name: &str, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    trace(kind, action, name, "requested")?;
    let result = body();
    trace(kind, action, name, if result.is_ok() { "succeeded" } else { "failed" })?;
    result
}

fn provider() -> &'static dyn Provider {
    PROVIDER.get().expect("installed deterministic provider").as_ref()
}

fn sync_clock() {
    let time = OPERATIONS.with(|ops| ops.borrow().last().filter(|op| !op.2).map(|op| op.1));
    moqui_lib::clock::assign(time);
}

impl Operation {
    pub fn suspend(&self, suspended: bool) {
        OPERATIONS.with(|ops| {
            let mut ops = ops.borrow_mut();
            let op = ops.last_mut().expect("operation stack");
            assert_eq!(op.0, self.sequence);
            op.2 = suspended;
        });
        sync_clock();
    }

    /// A failed database commit/rollback has no provable outcome. Leave the begin uncompleted
    /// for the coordinator to fail, rather than manufacturing a rolled_back acknowledgement.
    pub(crate) fn uncertain(mut self) -> Error {
        self.ended = true;
        fail();
        OPERATIONS.with(|ops| {
            assert_eq!(ops.borrow_mut().pop().map(|op| op.0), Some(self.sequence));
        });
        sync_clock();
        Error::new("deterministic database outcome is uncertain; restart required (operation left incomplete)")
    }

    pub fn finish(mut self, outcome: &str) -> Result<(), Error> {
        if !matches!(outcome, "committed" | "rolled_back" | "begin_failed") {
            return Err(Error::new("invalid deterministic completion outcome"));
        }
        self.end(outcome)
    }

    fn end(&mut self, outcome: &str) -> Result<(), Error> {
        self.ended = true;
        let result = provider().complete(json!({"sequence":self.sequence, "operation_id":self.operation_id,
            "event":"complete", "outcome":outcome}));
        OPERATIONS.with(|ops| {
            assert_eq!(ops.borrow_mut().pop().map(|op| op.0), Some(self.sequence));
        });
        sync_clock();
        if result.is_err() {
            fail();
        }
        result
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        if !self.ended {
            if check().is_err() {
                // A child or transport failed. Do not invent successful completion for its
                // enclosing operation; leave the actual begin unmatched for the coordinator.
                self.ended = true;
                OPERATIONS.with(|ops| {
                    assert_eq!(ops.borrow_mut().pop().map(|op| op.0), Some(self.sequence));
                });
                sync_clock();
                return;
            }
            let outcome = if self.synthetic && !std::thread::panicking() {
                "committed"
            } else {
                "rolled_back"
            };
            if let Err(err) = self.end(outcome) {
                // There is no Result in a destructor. Poisoning prevents any subsequent work.
                tracing::error!(error = %err, "deterministic completion failed; process requires restart");
            }
        }
    }
}
