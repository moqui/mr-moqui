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

//! The OpenSearch and Chromium archives built into the binary under the features `opensearch` and `chromium`
//! (`embed-opensearch` and `embed-chromium` of the binary): gzipped tars, embedded as `./moq` downloaded them, which the
//! host unpacks into the install directory and runs as their own processes, as moqui-drivers does with Postgres. They
//! are for packaging and delivery; once unpacked they are files on disk like any other install.

/// One program archive built in.
#[derive(Clone, Copy, Debug)]
pub struct Archive {
    /// The gzipped tar, as the build found it: its SHA-256 is the archive file's.
    pub bytes: &'static [u8],
    /// The OpenSearch version or the Chromium revision.
    pub version: &'static str,
}

#[cfg(feature = "opensearch")]
static OPENSEARCH: &[u8] = include_bytes!(env!("MOQUI_OPENSEARCH_STAGED"));

#[cfg(feature = "chromium")]
static CHROMIUM: &[u8] = include_bytes!(env!("MOQUI_CHROMIUM_STAGED"));

/// The OpenSearch archive (`opensearch-min`, Linux), when the binary has one.
#[must_use]
pub fn opensearch() -> Option<Archive> {
    #[cfg(feature = "opensearch")]
    {
        Some(Archive {
            bytes: OPENSEARCH,
            version: env!("MOQUI_OPENSEARCH_EMBEDDED_VERSION"),
        })
    }
    #[cfg(not(feature = "opensearch"))]
    {
        None
    }
}

/// The Chromium archive (a snapshot build, repacked as a gzipped tar), when the binary has one.
#[must_use]
pub fn chromium() -> Option<Archive> {
    #[cfg(feature = "chromium")]
    {
        Some(Archive {
            bytes: CHROMIUM,
            version: env!("MOQUI_CHROMIUM_EMBEDDED_REVISION"),
        })
    }
    #[cfg(not(feature = "chromium"))]
    {
        None
    }
}
