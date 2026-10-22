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

//! The bodies of the responses a script opened with `ec.service.rest()...send()`, still to be read: kept here by a
//! number the script holds, so a large body never becomes a script value. `response.forward` moves one into the web
//! response as a stream ([`stream_of`]); `text()` and `bytes()` read one whole. What the outermost script on the
//! thread leaves unread is dropped when it ends ([`clear`]).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};

use crate::context::Error;

use crate::resource::source::ByteSource;
use crate::web::ResponseStream;

/// A body still to be read, and its length when the server said it.
type Kept = (Box<dyn Read + Send>, Option<u64>);

thread_local! {
    static OPEN: RefCell<HashMap<u32, Kept>> = RefCell::new(HashMap::new());
    static NEXT: Cell<u32> = const { Cell::new(1) };
}

/// Keep an opened body; its number for the script.
pub(crate) fn register(reader: Box<dyn Read + Send>, length: Option<u64>) -> u32 {
    let id = NEXT.with(|next| {
        let id = next.get();
        next.set(id.wrapping_add(1).max(1));
        id
    });
    OPEN.with(|open| open.borrow_mut().insert(id, (reader, length)));
    id
}

fn take(id: u32) -> Option<Kept> {
    OPEN.with(|open| open.borrow_mut().remove(&id))
}

/// Read a kept body whole, at most `max` bytes, and forget it.
///
/// # Errors
///
/// Returns an error when the body was already read or forwarded, is larger than `max`, or the read fails.
pub(crate) fn read_all(id: u32, max: u64) -> Result<Vec<u8>, Error> {
    let (reader, _) = take(id).ok_or_else(|| Error::new("the response body was already read or forwarded"))?;
    let mut bytes = Vec::new();
    reader
        .take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|err| Error::new("could not read the response body").with_source(err))?;
    if bytes.len() as u64 > max {
        return Err(Error::new(format!("the response body is larger than {max} bytes")));
    }
    Ok(bytes)
}

/// A kept body as the stream of a web response, and forget it here.
pub(crate) fn stream_of(id: u32) -> Option<ResponseStream> {
    let (reader, length) = take(id)?;
    Some(ResponseStream {
        source: Arc::new(ReaderSource {
            state: Mutex::new((reader, 0)),
            length,
        }),
        offset: 0,
        length: length.unwrap_or(ResponseStream::UNKNOWN_LENGTH),
    })
}

/// Drop the bodies no one read.
pub(crate) fn clear() {
    OPEN.with(|open| open.borrow_mut().clear());
}

/// A body read once from the start, as the host streams it: each read must start where the last one ended.
struct ReaderSource {
    state: Mutex<(Box<dyn Read + Send>, u64)>,
    length: Option<u64>,
}

impl ByteSource for ReaderSource {
    fn len(&self) -> u64 {
        self.length.unwrap_or(ResponseStream::UNKNOWN_LENGTH)
    }

    fn read_at(&self, offset: u64, max: usize) -> Result<Vec<u8>, moqui_lib::error::Error> {
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        let (reader, position) = &mut *state;
        if offset != *position {
            return Err(moqui_lib::error::Error::new(format!(
                "a proxied body is read in order: asked for {offset}, at {position}"
            )));
        }
        let mut buffer = vec![0; max];
        let mut filled = 0;
        while filled < max {
            match reader.read(&mut buffer[filled..]) {
                Ok(0) => break,
                Ok(count) => filled += count,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Err(err) => {
                    return Err(moqui_lib::error::Error::new("could not read the proxied body").with_source(err))
                }
            }
        }
        buffer.truncate(filled);
        *position += filled as u64;
        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_body_is_read_once_whole_or_as_a_stream() {
        let id = register(Box::new(std::io::Cursor::new(b"hello world".to_vec())), None);
        let stream = stream_of(id).unwrap();
        assert_eq!(stream.length, ResponseStream::UNKNOWN_LENGTH);
        assert_eq!(stream.source.read_at(0, 5).unwrap(), b"hello");
        assert!(stream.source.read_at(0, 5).is_err(), "in order only");
        assert_eq!(stream.source.read_at(5, 100).unwrap(), b" world");
        assert!(stream.source.read_at(11, 100).unwrap().is_empty());
        assert!(stream_of(id).is_none(), "forwarded once");

        let id = register(Box::new(std::io::Cursor::new(vec![7; 10])), Some(10));
        assert!(read_all(id, 5).is_err(), "over the limit");
        let id = register(Box::new(std::io::Cursor::new(vec![7; 10])), Some(10));
        assert_eq!(read_all(id, 10).unwrap().len(), 10);
        let id = register(Box::new(std::io::Cursor::new(vec![1])), None);
        clear();
        assert!(read_all(id, 10).is_err());
    }
}
