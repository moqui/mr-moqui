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

//! The body of a response that streams ([`moqui_core::web::ResponseStream`]): a window of its source read only when
//! the client has taken the one before, on a blocking thread (in an execution context of its own for a source in the
//! database, at most the webapp's `transfer-workers` of those reading at once). A slow download holds no request worker,
//! and its memory is a window.

use std::sync::{Arc, OnceLock};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::resource::source::READ_WINDOW;
use moqui_core::web::ResponseStream;
use tokio::sync::Semaphore;

/// The reads of database sources that may run at once, for every streamed response of the process: the `webroot`
/// webapp's `transfer-workers` of the first factory that streams one.
fn readers(factory: &ExecutionContextFactory) -> Arc<Semaphore> {
    static READERS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    Arc::clone(READERS.get_or_init(|| {
        let workers = factory
            .webapp_setting("webroot", "transfer-workers")
            .parse::<usize>()
            .ok()
            .filter(|workers| *workers > 0)
            .unwrap_or(16);
        Arc::new(Semaphore::new(workers))
    }))
}

struct State {
    stream: ResponseStream,
    sent: u64,
    factory: Arc<ExecutionContextFactory>,
}

/// The body that reads `stream` a window at a time.
pub(crate) fn body(factory: Arc<ExecutionContextFactory>, stream: ResponseStream) -> axum::body::Body {
    let state = State {
        stream,
        sent: 0,
        factory,
    };
    let windows = futures_util::stream::unfold(Some(state), |state| async move {
        let state = state?;
        if state.sent >= state.stream.length {
            return None;
        }
        let want = usize::try_from(state.stream.length - state.sent)
            .unwrap_or(usize::MAX)
            .min(READ_WINDOW);
        let offset = state.stream.offset + state.sent;
        let source = Arc::clone(&state.stream.source);
        let factory = Arc::clone(&state.factory);
        let needs_context = source.needs_context();
        let permit = if needs_context {
            readers(&factory).acquire_owned().await.ok()
        } else {
            None
        };
        let read = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if needs_context {
                let context = factory.new_context();
                let _entered = context.enter();
                source.read_at(offset, want)
            } else {
                source.read_at(offset, want)
            }
        })
        .await;
        let failed = |text: String| Some((Err(std::io::Error::other(text)), None));
        match read {
            Ok(Ok(bytes)) if !bytes.is_empty() => {
                let count = bytes.len() as u64;
                let next = State {
                    sent: state.sent + count,
                    ..state
                };
                Some((Ok(bytes::Bytes::from(bytes)), Some(next)))
            }
            // a stream of unknown length (a proxied body sent in chunks) ends when its source does
            Ok(Ok(_)) if state.stream.length == ResponseStream::UNKNOWN_LENGTH => None,
            Ok(Ok(_)) => {
                tracing::warn!("a streamed response ended at {offset} of {} bytes", state.stream.length);
                failed("the content ended early".to_string())
            }
            Ok(Err(err)) => {
                tracing::warn!("a streamed response could not be read at {offset}: {}", err.message());
                failed(err.message().to_string())
            }
            Err(err) => failed(err.to_string()),
        }
    });
    // hyper may poll a body once more after its end, which an `unfold` must not be
    axum::body::Body::from_stream(futures_util::StreamExt::fuse(windows))
}
