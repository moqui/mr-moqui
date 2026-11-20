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

//! The answer to a link connection's role byte: a code, then the length (2 bytes, big-endian) and text of a message.

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

/// The accepting end's answer to the role byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Answer {
    Ok = 0,
    /// The key is not paired with this end, or was revoked.
    Refused = 1,
    /// No up connection of that generation (a down connection).
    Stale = 2,
    /// Anything else: a bad role, an error on the accepting end.
    Failed = 3,
}

impl Answer {
    fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Ok,
            1 => Self::Refused,
            2 => Self::Stale,
            _ => Self::Failed,
        }
    }
}

/// The longest message an [`Answer`] carries.
const MAX_ANSWER_TEXT: usize = 1024;

pub(crate) async fn write_answer<S: AsyncWrite + Unpin>(
    stream: &mut S,
    answer: Answer,
    message: &str,
) -> std::io::Result<()> {
    let text = message.as_bytes();
    let text = text.get(..MAX_ANSWER_TEXT).unwrap_or(text);
    let length = u16::try_from(text.len()).unwrap_or(u16::MAX);
    let mut frame = Vec::with_capacity(3 + text.len());
    frame.push(answer as u8);
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(text);
    stream.write_all(&frame).await?;
    stream.flush().await
}

pub(crate) async fn read_answer<S: AsyncRead + Unpin>(stream: &mut S) -> std::io::Result<(Answer, String)> {
    let mut head = [0_u8; 3];
    stream.read_exact(&mut head).await?;
    let length = usize::from(u16::from_be_bytes([head[1], head[2]]));
    if length > MAX_ANSWER_TEXT {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the other end's answer is too long",
        ));
    }
    let mut text = vec![0_u8; length];
    stream.read_exact(&mut text).await?;
    Ok((Answer::from_code(head[0]), String::from_utf8_lossy(&text).into_owned()))
}
