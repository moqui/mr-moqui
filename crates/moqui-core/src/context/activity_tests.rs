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

use std::borrow::Cow;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

use super::{note_user, pop, push, running, MirrorFrame, SLOTS};
use moqui_lib::sync::lock;

fn frame(artifact_type: &'static str, name: &str) -> MirrorFrame {
    MirrorFrame {
        artifact_type: Cow::Borrowed(artifact_type),
        name: Arc::from(name),
        action: Cow::Borrowed("view"),
        started: Instant::now(),
    }
}

fn of_thread(name: &str) -> Option<super::ThreadActivity> {
    running().into_iter().find(|activity| activity.thread_name == name)
}

fn slots_of(name: &str) -> usize {
    lock(&SLOTS)
        .iter()
        .filter_map(std::sync::Weak::upgrade)
        .filter(|slot| slot.thread_name == name)
        .count()
}

/// Runs `steps` on a thread of its own, one at each message, and answers after each.
fn stepped(name: &str, steps: Vec<Box<dyn FnOnce() + Send>>) -> (mpsc::Sender<()>, mpsc::Receiver<()>) {
    let (go, wait) = mpsc::channel::<()>();
    let (done, finished) = mpsc::channel::<()>();
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            for step in steps {
                if wait.recv().is_err() {
                    return;
                }
                step();
                let _ = done.send(());
            }
            // the thread ends when the test drops its sender
            let _ = wait.recv();
        })
        .unwrap();
    (go, finished)
}

#[test]
fn another_thread_reads_the_stack_as_it_is_pushed_and_popped() {
    let name = "activity-mirror-test";
    let (go, finished) = stepped(
        name,
        vec![
            Box::new(|| {
                let wants_user = push(frame("AT_XML_SCREEN", "apps/system/Home"), || {
                    Some("abc123".to_string())
                });
                assert!(wants_user, "the first frame of a piece of work asks for its user");
                note_user(Some("EX_JOHN_DOE".to_string()));
                // a trace is asked for only while there is none
                let wants_user = push(frame("AT_SERVICE", "org.moqui.impl.get#Thing"), || -> Option<String> {
                    panic!("the work already has a trace")
                });
                assert!(!wants_user, "the user is known");
            }),
            Box::new(pop),
            Box::new(pop),
            // a pop with nothing on the stack leaves it empty
            Box::new(pop),
            Box::new(|| {
                push(frame("AT_SERVICE", "second#Work"), || None);
            }),
        ],
    );
    let step = || {
        go.send(()).unwrap();
        finished.recv().unwrap();
    };

    assert!(of_thread(name).is_none(), "a thread that pushed nothing is not listed");
    step();
    let activity = of_thread(name).unwrap();
    let names: Vec<&str> = activity.frames.iter().map(|frame| frame.name.as_str()).collect();
    assert_eq!(
        names,
        ["apps/system/Home", "org.moqui.impl.get#Thing"],
        "the first pushed first"
    );
    assert_eq!(activity.frames[0].artifact_type, "AT_XML_SCREEN");
    assert_eq!(activity.user_id.as_deref(), Some("EX_JOHN_DOE"));
    assert_eq!(activity.trace_id.as_deref(), Some("abc123"));
    if cfg!(target_os = "linux") {
        assert!(activity.thread_id > 0, "the kernel's thread id on Linux");
    }

    step();
    assert_eq!(of_thread(name).unwrap().frames.len(), 1);
    step();
    assert!(
        of_thread(name).is_none(),
        "nothing on the stack: the thread runs no artifact"
    );
    step();
    assert!(of_thread(name).is_none());

    // the next work on the thread has its own user and trace, in the same slot
    step();
    let activity = of_thread(name).unwrap();
    assert_eq!(activity.frames.len(), 1);
    assert_eq!(activity.user_id, None, "the user went with the work it was of");
    assert_eq!(activity.trace_id, None);
    assert_eq!(slots_of(name), 1, "one slot a thread, however often it pushes");

    // the thread ends: its slot is dropped with it
    drop(go);
    for _ in 0..200 {
        if slots_of(name) == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(slots_of(name), 0, "a finished thread's slot is gone");
    assert!(of_thread(name).is_none());
}

#[test]
fn the_artifact_stack_mirrors_its_frames_and_those_it_abandons() {
    let name = "activity-stack-test";
    let (go, finished) = stepped(
        name,
        vec![Box::new(|| {
            let context = crate::context::ExecutionContextFactory::test_context();
            let _entered = context.enter();
            context.push("AT_SERVICE", "outer#Service", "all");
            context.push("AT_ENTITY", "moqui.test.TestEntity", "view");
            let activity = running()
                .into_iter()
                .find(|activity| activity.thread_name == "activity-stack-test")
                .unwrap();
            let names: Vec<&str> = activity.frames.iter().map(|frame| frame.name.as_str()).collect();
            assert_eq!(names, ["outer#Service", "moqui.test.TestEntity"]);
            context.pop().unwrap();
            // the service's frame is left on the stack: the context's drop takes it from the mirror too
        })],
    );
    go.send(()).unwrap();
    finished.recv().unwrap();
    assert!(
        of_thread(name).is_none(),
        "the frames a dropped stack still held are not left in the mirror"
    );
}
