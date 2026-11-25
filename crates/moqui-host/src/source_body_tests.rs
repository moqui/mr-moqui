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
use moqui_core::{
    context::ExecutionContext,
    resource::source::ByteSource,
    telemetry::{attach, capture, TraceContext},
};
use moqui_lib::error::Error;

struct ObservedSource(Option<TraceContext>);
impl ByteSource for ObservedSource {
    fn len(&self) -> u64 {
        2
    }
    fn needs_context(&self) -> bool {
        true
    }
    fn read_at(&self, _: u64, _: usize) -> Result<Vec<u8>, Error> {
        assert!(
            ExecutionContext::with_current(|_| ()).is_some(),
            "database source requires an entered worker context"
        );
        assert_eq!(
            capture(),
            self.0,
            "transfer worker must restore the captured request context, including anonymous reads"
        );
        Ok(b"ok".to_vec())
    }
}

#[tokio::test]
async fn source_reads_restore_request_context_without_holding_a_guard_across_await() {
    let context = ExecutionContextFactory::test_context();
    let marker = TraceContext::extract("00-11111111111111111111111111111111-2222222222222222-01", "").unwrap();
    for trace in [Some(marker), None] {
        let body = {
            let _trace = attach(trace.clone());
            body(
                Arc::clone(context.factory()),
                ResponseStream {
                    source: Arc::new(ObservedSource(trace)),
                    offset: 0,
                    length: 2,
                },
            )
        };
        assert!(
            capture().is_none(),
            "caller context is restored before async consumption"
        );
        assert_eq!(&axum::body::to_bytes(body, 16).await.unwrap()[..], b"ok");
    }
}

/// A source of `windows` windows that counts its reads.
struct CountedSource {
    windows: u64,
    reads: std::sync::atomic::AtomicUsize,
}

impl ByteSource for CountedSource {
    fn len(&self) -> u64 {
        self.windows * READ_WINDOW as u64
    }
    fn read_at(&self, offset: u64, max: usize) -> Result<Vec<u8>, Error> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(vec![0; max.min(usize::try_from(self.len() - offset).unwrap())])
    }
}

/// A window whose read is queued for a blocking thread when the client goes away is not read when the thread takes it:
/// with one blocking thread held busy, the body asks for its first window, is dropped, and the thread is let go.
#[test]
fn a_read_queued_when_the_body_is_dropped_is_not_made() {
    let context = ExecutionContextFactory::test_context();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    let source = Arc::new(CountedSource {
        windows: 2,
        reads: Default::default(),
    });
    runtime.block_on(async {
        let (release, held) = std::sync::mpsc::channel::<()>();
        let busy = tokio::task::spawn_blocking(move || held.recv());
        let stream = ResponseStream {
            source: Arc::clone(&source) as Arc<dyn ByteSource>,
            offset: 0,
            length: source.len(),
        };
        let mut data = body(Arc::clone(context.factory()), stream).into_data_stream();
        let waited = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            futures_util::StreamExt::next(&mut data),
        )
        .await;
        assert!(waited.is_err(), "the read waits for the one blocking thread");
        drop(data);
        release.send(()).unwrap();
        busy.await.unwrap().unwrap();
        // the queued read runs on the thread now free, after the one it waited for
        tokio::task::spawn_blocking(|| ()).await.unwrap();
    });
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        0,
        "nothing is read for a body dropped"
    );
}

/// A read whose body is gone by the time its thread takes it reads nothing.
#[test]
fn a_read_queued_after_the_body_is_gone_is_skipped() {
    let source = CountedSource {
        windows: 1,
        reads: Default::default(),
    };
    let gone = AtomicBool::new(true);
    assert!(read_window(&source, &gone, None, 0, 16).is_err());
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    gone.store(false, Ordering::SeqCst);
    assert_eq!(read_window(&source, &gone, None, 0, 16).unwrap().len(), 16);
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);
}
