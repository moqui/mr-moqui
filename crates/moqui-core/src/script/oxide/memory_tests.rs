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

//! The thread's oxide runtime under many texts and when it runs out of heap: its caches stay bounded on its heap, and
//! a script that runs out fails alone.

use std::sync::Arc;

use moqui_lib::object::Value;

use crate::context::Error;
use crate::screen::sri::Runner;
use crate::script::engine::{with_script_engine, ScriptEngine};
use crate::script::{clear_context_stack, push_context_frame};

const MIB: usize = 1024 * 1024;

/// A screen unit's function of about `statements` lines, distinct by `index`.
fn unit_source(index: usize, statements: usize) -> String {
    let mut source = String::from("(function ($depth) { var out = '';\n");
    for line in 0..statements {
        source.push_str(&format!(
            "out += '<div class=\"u{index}-{line}\">' + String($depth + {line}) + '</div>'; if (out.length > {index}) out = out.slice(1);\n"
        ));
    }
    source.push_str("unitOut = out.length; })");
    source
}

/// Past the static code store, a unit's code is on the runtime's heap: many distinct units (a test run over many
/// screens) and many distinct texts keep the live heap well under the cap, where the unit cache bounded by QuickJS's
/// source bytes held about 3.5 times its bytes of code until the runtime ran out.
#[test]
fn many_distinct_units_and_texts_keep_the_heap_bounded() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    with_script_engine(ScriptEngine::Oxide, || {
        // the runtime the units and texts run on: bounded by its caches, not by a restart
        assert_eq!(crate::script::eval_value("1")?, Value::I32(1));
        let started = super::engine::start_for_tests()?;
        // 1600 units of about 7.5 KB: 12 MB of source, three times the static store's 4 MiB
        for index in 0..1600 {
            let unit = crate::screen::generate::Unit {
                id: u64::MAX - 1000 - index as u64,
                source: unit_source(index, 80),
                runtime: Arc::from(""),
                runtime_id: 0,
            };
            super::units::RUNNER.run(&unit)?;
        }
        for index in 0..2000 {
            let text = format!("[{index}, 'text {index}'].length + {index}");
            assert_eq!(crate::script::eval_value(&text)?, Value::I32(2 + index));
        }
        let running = super::engine::start_for_tests()?;
        assert!(
            std::rc::Rc::ptr_eq(&started, &running),
            "the runtime was not started again"
        );
        let memory = super::engine_memory(true);
        assert!(
            memory.live_bytes < 20 * MIB,
            "live {} of reserved {}, code {}",
            memory.live_bytes,
            memory.reserved_bytes,
            memory.code_bytes
        );
        Ok(())
    })
}

/// A script that runs out of heap fails with `out of memory`, alone: the caches go and the runtime collects, and one
/// that left its heap near the cap (a global holding what it made) is dropped and started again, so the scripts after
/// it run.
#[test]
fn running_out_of_memory_fails_that_script_alone() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    with_script_engine(ScriptEngine::Oxide, || {
        let value = |text: &str| crate::script::eval_value(text);
        assert_eq!(value("1 + 1")?, Value::I32(2));
        let hungry = "(function () { const hog = []; while (true) hog.push(new Array(100000).fill(1)); })()";
        let err = crate::script::run_script(hungry).expect_err("out of memory");
        assert!(err.message().contains("out of memory"), "{}", err.message());
        assert_eq!(value("1 + 1")?, Value::I32(2), "the runtime runs again");
        // one that keeps what it made (on a prototype: the global object's names are the context stack's): the runtime
        // starts again
        let kept = "Array.prototype.hog = []; while (true) [].hog.push(new Array(1000).fill(1))";
        let err = crate::script::run_script(kept).expect_err("out of memory");
        assert!(err.message().contains("out of memory"), "{}", err.message());
        assert_eq!(value("typeof [].hog")?, Value::string("undefined"), "a new runtime");
        assert_eq!(value("2 + 2")?, Value::I32(4));
        for index in 0..50 {
            assert_eq!(value(&format!("{index} * 2"))?, Value::I32(index * 2));
        }
        let memory = super::engine_memory(true);
        assert!(memory.live_bytes < 8 * MIB, "live {}", memory.live_bytes);
        Ok(())
    })
}
