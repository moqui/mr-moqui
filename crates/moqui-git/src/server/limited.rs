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

//! How much of a request the git server reads (docs/interfaces/Git-Hosting.md, Request limits): `git-server`
//! `max-request` (64 MiB) for upload-pack and `max-push-size` (2 GiB) for receive-pack, counted after a gzip body is
//! inflated, and refused, not cut short, past them. Over SSH a push's whole input is one request, and an upload-pack
//! conversation is held to it round by round (`upload.rs`), as HTTP holds each of its requests.

use std::io::Read;

use super::GitService;

pub(crate) const DEFAULT_MAX_REQUEST: u64 = 64 * 1024 * 1024;
pub(crate) const DEFAULT_MAX_PUSH: u64 = 2 * 1024 * 1024 * 1024;

/// The most bytes of one request for `service`: an HTTP request, a round of an SSH upload-pack, an SSH push's input.
pub(crate) fn request_limit(service: GitService) -> u64 {
    let (name, default) = match service {
        GitService::UploadPack => ("max-request", DEFAULT_MAX_REQUEST),
        GitService::ReceivePack => ("max-push-size", DEFAULT_MAX_PUSH),
    };
    Some(crate::number_setting(name, default))
        .filter(|limit| *limit > 0)
        .unwrap_or(default)
}

/// A reader that gives at most `limit` bytes and then an error, where `Read::take` would end the input quietly: a
/// request cut at the limit must not be read as a whole one.
pub(crate) struct Capped<R> {
    inner: R,
    left: u64,
    limit: u64,
}

impl<R: Read> Capped<R> {
    pub(crate) fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            left: limit,
            limit,
        }
    }

    /// The next round of a conversation: `limit` bytes more.
    pub(crate) fn renew(&mut self) {
        self.left = self.limit;
    }
}

impl<R: Read> Read for Capped<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            // at the limit: the input may end here, but not go on
            let mut probe = [0u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(std::io::Error::other(format!(
                    "the git request is over {} bytes",
                    self.limit
                ))),
            };
        }
        let most = buf.len().min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let count = self.inner.read(&mut buf[..most])?;
        self.left -= count as u64;
        Ok(count)
    }
}

#[cfg(test)]
#[path = "limited_tests.rs"]
mod tests;
