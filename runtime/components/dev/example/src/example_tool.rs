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

//! `ExampleTool`: a tool factory with each part of the lifecycle, and the tool scripts get from
//! `ec.factory.getTool("ExampleTool")`. `MoquiConf.xml` selects it by its class, [`CLASS`].
//!
//! ```js
//! const tool = ec.factory.getTool("ExampleTool")
//! tool.greet("Ada")             // "Hello, Ada!"
//! tool.greeting                 // "Hello", a property
//! tool.counter().increment(2)   // a second tool, shared by every instance of this factory
//! ec.factory.getTool("ExampleTool", "Hi").greet("Ada")   // "Hi, Ada!": getTool parameters reach get_instance()
//! ```

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, OnceLock};

use moqui_core::context::{Error, ExecutionContext, ExecutionContextFactory};
use moqui_core::native::{tool_value, NativeRegistry, Parameters, Tool, ToolFactory};
use moqui_lib::object::Value;

/// The identifier `<tool-factory class="..."/>` selects.
const CLASS: &str = "example::ExampleToolFactory";

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.tool(CLASS, || Arc::new(ExampleToolFactory::default()))
}

#[derive(Default)]
struct ExampleToolFactory {
    /// From `example_greeting`, read in `init`.
    greeting: OnceLock<String>,
    /// State the factory keeps for every instance it gives.
    counter: Arc<Counter>,
}

impl ToolFactory for ExampleToolFactory {
    fn name(&self) -> &str {
        "ExampleTool"
    }

    /// Before the entity facade is there. A service whose body needs this factory's state is bound here, as a closure,
    /// before any service definition is read: `example.ExampleServices.get#CounterValue`.
    fn pre_facade_init(&self, factory: &Arc<ExecutionContextFactory>) -> Result<(), Error> {
        let counter = Arc::clone(&self.counter);
        factory.register_service("example::counter_value", move |_ec, _service, _parameters| {
            let mut out = Parameters::new();
            out.insert("count".to_string(), Value::I64(counter.value.load(Ordering::Relaxed)));
            Ok(out)
        })
    }

    /// The entity facade is attached here, before the data load and the first request.
    fn init(&self, _factory: &Arc<ExecutionContextFactory>) -> Result<(), Error> {
        let configured = moqui_lib::env::get_prop_or_env("example_greeting");
        let greeting = if configured.is_empty() {
            "Hello".to_string()
        } else {
            configured
        };
        tracing::info!("ExampleTool init, greeting {greeting}");
        let _ = self.greeting.set(greeting);
        Ok(())
    }

    /// A first `getTool` parameter replaces the greeting for that instance.
    fn get_instance(&self, parameters: &[Value]) -> Result<Arc<dyn Tool>, Error> {
        let greeting = match parameters.first() {
            Some(Value::String(text)) if !text.is_empty() => text.clone(),
            Some(Value::Null) | None => self.greeting.get().cloned().unwrap_or_else(|| "Hello".to_string()),
            Some(other) => {
                return Err(Error::new(format!(
                    "ExampleTool takes a greeting string, not {other:?}"
                )))
            }
        };
        Ok(Arc::new(ExampleTool {
            greeting,
            counter: Arc::clone(&self.counter),
        }))
    }

    fn type_declaration(&self) -> Option<&'static str> {
        Some(
            "{ greet(name: string): string; counter(): { increment(by?: number): number; value(): number }; \
             readonly greeting: string }",
        )
    }

    fn destroy(&self) {
        tracing::info!(
            "ExampleTool destroy, counter at {}",
            self.counter.value.load(Ordering::Relaxed)
        );
    }
}

struct ExampleTool {
    greeting: String,
    counter: Arc<Counter>,
}

impl Tool for ExampleTool {
    fn methods(&self) -> &'static [&'static str] {
        &["greet", "counter"]
    }

    fn call(&self, _ec: &ExecutionContext, method: &str, args: &[Value]) -> Result<Value, Error> {
        match method {
            "greet" => {
                let name = match args.first() {
                    Some(Value::Null) | None => "World".to_string(),
                    Some(value) => moqui_lib::object::to_plain_string(value),
                };
                Ok(Value::String(format!("{}, {name}!", self.greeting)))
            }
            // A tool can give another tool: a script calls methods on the result too.
            "counter" => Ok(tool_value(Arc::clone(&self.counter) as Arc<dyn Tool>)),
            _ => Err(Error::new(format!("ExampleTool has no method {method}"))),
        }
    }

    fn property(&self, _ec: &ExecutionContext, name: &str) -> Result<Option<Value>, Error> {
        Ok((name == "greeting").then(|| Value::String(self.greeting.clone())))
    }
}

#[derive(Default)]
struct Counter {
    value: AtomicI64,
}

impl Tool for Counter {
    fn methods(&self) -> &'static [&'static str] {
        &["increment", "value"]
    }

    fn call(&self, _ec: &ExecutionContext, method: &str, args: &[Value]) -> Result<Value, Error> {
        match method {
            "increment" => {
                let by = match args.first() {
                    Some(Value::Null) | None => 1,
                    Some(value) => moqui_lib::object::to_plain_string(value)
                        .parse::<i64>()
                        .map_err(|_| Error::new("increment takes a whole number"))?,
                };
                Ok(Value::I64(self.value.fetch_add(by, Ordering::Relaxed) + by))
            }
            "value" => Ok(Value::I64(self.value.load(Ordering::Relaxed))),
            _ => Err(Error::new(format!("counter has no method {method}"))),
        }
    }
}
