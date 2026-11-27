//! The spans of one trace that wait for its root to decide whether the trace is kept, at most tail-trace-spans of
//! them: the tail under a root open on its thread (`sdk.rs`) and the export worker's buffer of each trace (`worker`).
//! Full, a buffer drops the newcomer, or, for an important span (one that failed, or a slow artifact hit), the latest
//! ordinary span it holds, so a long trace that ends in a failure keeps the failure (MON-UI-003). It counts each span
//! it drops, which the kept root records (`moqui.trace.dropped_spans`).

/// A span as a trace buffer holds it.
pub(super) trait Waiting {
    /// It failed or was a slow artifact hit: it keeps its trace, and a full buffer makes room for it.
    fn important(&self) -> bool;
}

/// What [`TraceBuffer::add`] did with a span.
pub(super) enum Added<T> {
    Held,
    /// The buffer was full and the span was dropped.
    Dropped,
    /// The span took the place of this ordinary one, which was dropped.
    Replaced(T),
}

pub(super) struct TraceBuffer<T> {
    spans: Vec<T>,
    /// How many of `spans` are not important: none, and a full buffer has no room to make.
    ordinary: usize,
    dropped: u64,
}

impl<T> Default for TraceBuffer<T> {
    fn default() -> Self {
        Self {
            spans: Vec::new(),
            ordinary: 0,
            dropped: 0,
        }
    }
}

impl<T: Waiting> TraceBuffer<T> {
    /// Hold `span` when there is `room` for one more. With none, an important span takes the place of the latest
    /// ordinary one, if `fits` says it may take that one's place; otherwise it is dropped.
    pub(super) fn add(&mut self, span: T, room: bool, fits: impl FnOnce(&T) -> bool) -> Added<T> {
        let important = span.important();
        if room {
            self.ordinary += usize::from(!important);
            self.spans.push(span);
            return Added::Held;
        }
        self.dropped += 1;
        if important && self.ordinary > 0 {
            if let Some(at) = self.spans.iter().rposition(|held| !held.important()) {
                if fits(&self.spans[at]) {
                    self.ordinary -= 1;
                    return Added::Replaced(std::mem::replace(&mut self.spans[at], span));
                }
            }
        }
        Added::Dropped
    }

    pub(super) fn len(&self) -> usize {
        self.spans.len()
    }

    pub(super) fn spans(&self) -> &[T] {
        &self.spans
    }

    pub(super) fn has_important(&self) -> bool {
        self.ordinary < self.spans.len()
    }

    /// The spans dropped so far.
    pub(super) fn dropped(&self) -> u64 {
        self.dropped
    }

    pub(super) fn into_spans(self) -> Vec<T> {
        self.spans
    }
}
