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
