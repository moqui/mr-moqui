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

//! Git's delta format, the body of a pack's `OFS_DELTA` and `REF_DELTA` entries: the sizes of the base and of the
//! result, then instructions that copy a span of the base or insert literal bytes. [`encode`] makes one the way git's
//! `diff-delta.c` does (an index of the base's 16-byte blocks, the target scanned a byte at a time with a rolling
//! hash, each match extended both ways); [`apply`] checks every instruction against both sizes, so a damaged delta is
//! an error rather than a wrong object. Written here: gitoxide applies deltas inside its pack reader only, and makes
//! none.

use moqui_core::context::Error;

/// The bytes of a block of the base the index holds, and of the window the target is scanned with.
const BLOCK: usize = 16;

/// The most base offsets one hash bucket keeps (git's `HASH_LIMIT`): data that repeats a block over and over is not
/// searched at every repeat.
const BUCKET_LIMIT: usize = 64;

/// The most bytes one copy instruction takes, as git writes them (the format allows 0xffffff; older readers 0x10000).
const MAX_COPY: usize = 0x10000;

/// The most bytes one insert instruction carries.
const MAX_INSERT: usize = 0x7f;

/// The multiplier of the rolling hash.
const PRIME: u32 = 0x0100_0193;

fn bad(what: &str) -> Error {
    Error::new(format!("bad git delta: {what}"))
}

/// A size as git writes it in a delta's header: 7 bits a byte, least significant first, the high bit set on every
/// byte but the last.
fn push_size(out: &mut Vec<u8>, mut size: u64) {
    loop {
        let byte = (size & 0x7f) as u8;
        size >>= 7;
        if size == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn read_size(delta: &[u8], at: &mut usize) -> Result<u64, Error> {
    let mut size = 0u64;
    let mut shift = 0u32;
    loop {
        let byte = *delta.get(*at).ok_or_else(|| bad("the header is cut short"))?;
        *at += 1;
        if shift > 63 {
            return Err(bad("a size in the header is too long"));
        }
        size |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(size);
        }
        shift += 7;
    }
}

/// The sizes of the base and of the result that a delta's header gives.
#[cfg(test)]
pub(crate) fn sizes(delta: &[u8]) -> Result<(u64, u64), Error> {
    let mut at = 0;
    let base = read_size(delta, &mut at)?;
    let result = read_size(delta, &mut at)?;
    Ok((base, result))
}

/// The object `delta` makes from `base`. Its header must name the size of `base`, and of the result `expected` when
/// given; every copy must be within `base` and the result exactly the size its header names.
pub(crate) fn apply(base: &[u8], delta: &[u8], expected: Option<u64>) -> Result<Vec<u8>, Error> {
    let mut at = 0;
    let base_size = read_size(delta, &mut at)?;
    let result_size = read_size(delta, &mut at)?;
    if base_size != base.len() as u64 {
        return Err(bad(&format!(
            "it is for a base of {base_size} bytes, not {}",
            base.len()
        )));
    }
    if expected.is_some_and(|expected| expected != result_size) {
        return Err(bad(&format!(
            "it makes {result_size} bytes, not {}",
            expected.unwrap_or_default()
        )));
    }
    let result_size = usize::try_from(result_size).map_err(|_| bad("the result is too large for memory"))?;
    // the header is not trusted for the allocation: every instruction makes at most 0xffffff bytes from a few
    let mut out = Vec::with_capacity(result_size.min(delta.len().saturating_mul(4096).max(1024)));
    while let Some(&op) = delta.get(at) {
        at += 1;
        if op & 0x80 != 0 {
            let mut offset = 0usize;
            let mut size = 0usize;
            for (bit, shift) in [(0x01, 0), (0x02, 8), (0x04, 16), (0x08, 24)] {
                if op & bit != 0 {
                    let byte = *delta.get(at).ok_or_else(|| bad("a copy is cut short"))?;
                    at += 1;
                    offset |= usize::from(byte) << shift;
                }
            }
            for (bit, shift) in [(0x10, 0), (0x20, 8), (0x40, 16)] {
                if op & bit != 0 {
                    let byte = *delta.get(at).ok_or_else(|| bad("a copy is cut short"))?;
                    at += 1;
                    size |= usize::from(byte) << shift;
                }
            }
            if size == 0 {
                size = MAX_COPY;
            }
            let end = offset
                .checked_add(size)
                .filter(|end| *end <= base.len())
                .ok_or_else(|| bad("a copy reaches past the end of the base"))?;
            if out.len() + size > result_size {
                return Err(bad("it makes more than its header says"));
            }
            out.extend_from_slice(&base[offset..end]);
        } else if op != 0 {
            let size = usize::from(op);
            let bytes = delta.get(at..at + size).ok_or_else(|| bad("an insert is cut short"))?;
            at += size;
            if out.len() + size > result_size {
                return Err(bad("it makes more than its header says"));
            }
            out.extend_from_slice(bytes);
        } else {
            return Err(bad("instruction 0 is reserved"));
        }
    }
    if out.len() != result_size {
        return Err(bad(&format!(
            "it makes {} bytes, not the {result_size} its header says",
            out.len()
        )));
    }
    Ok(out)
}

/// The hash of a block of [`BLOCK`] bytes.
fn block_hash(block: &[u8]) -> u32 {
    block.iter().fold(0u32, |hash, byte| {
        hash.wrapping_mul(PRIME).wrapping_add(u32::from(*byte))
    })
}

/// The base's blocks by hash: chains of offsets through `next`, from the bucket `heads` (0: empty, else an index + 1).
struct Index {
    heads: Vec<u32>,
    next: Vec<u32>,
    offsets: Vec<u32>,
    shift: u32,
}

impl Index {
    fn new(base: &[u8]) -> Self {
        let blocks = base.len() / BLOCK;
        let bits = blocks.max(16).next_power_of_two().trailing_zeros();
        let buckets = 1usize << bits;
        let mut index = Self {
            heads: vec![0; buckets],
            next: Vec::with_capacity(blocks),
            offsets: Vec::with_capacity(blocks),
            shift: 32 - bits,
        };
        let mut counts = vec![0u8; buckets];
        // a bucket keeps the first blocks it is given: in data that repeats, a match from early on extends furthest
        for block in 0..blocks {
            let offset = block * BLOCK;
            let bucket = index.bucket(block_hash(&base[offset..offset + BLOCK]));
            if usize::from(counts[bucket]) >= BUCKET_LIMIT {
                continue;
            }
            counts[bucket] += 1;
            index.offsets.push(offset as u32);
            index.next.push(index.heads[bucket]);
            index.heads[bucket] = index.offsets.len() as u32;
        }
        index
    }

    fn bucket(&self, hash: u32) -> usize {
        (hash.wrapping_mul(0x9e37_79b1) >> self.shift) as usize
    }
}

/// Copy `size` bytes at `offset` of the base, in instructions of at most [`MAX_COPY`] bytes.
fn push_copy(out: &mut Vec<u8>, mut offset: usize, mut size: usize) {
    while size > 0 {
        let take = size.min(MAX_COPY);
        let at = out.len();
        out.push(0x80);
        let mut op = 0x80u8;
        for (index, bit) in [0x01u8, 0x02, 0x04, 0x08].into_iter().enumerate() {
            let byte = (offset >> (index * 8)) as u8;
            if byte != 0 {
                op |= bit;
                out.push(byte);
            }
        }
        // a size of 0x10000 is written as no size bytes
        let written = if take == MAX_COPY { 0 } else { take };
        for (index, bit) in [0x10u8, 0x20, 0x40].into_iter().enumerate() {
            let byte = (written >> (index * 8)) as u8;
            if byte != 0 {
                op |= bit;
                out.push(byte);
            }
        }
        out[at] = op;
        offset += take;
        size -= take;
    }
}

fn push_insert(out: &mut Vec<u8>, bytes: &[u8]) {
    for chunk in bytes.chunks(MAX_INSERT) {
        out.push(chunk.len() as u8);
        out.extend_from_slice(chunk);
    }
}

/// A delta that makes `target` from `base`, or `None` when it would be over `max_size` bytes (or `base` is too short
/// to index), as a delta that saves little is not worth its base.
pub(crate) fn encode(base: &[u8], target: &[u8], max_size: usize) -> Option<Vec<u8>> {
    if base.len() < BLOCK || u32::try_from(base.len()).is_err() {
        return None;
    }
    let index = Index::new(base);
    let mut out = Vec::with_capacity(max_size.min(target.len() / 4 + 64));
    push_size(&mut out, base.len() as u64);
    push_size(&mut out, target.len() as u64);
    // the multiplier of the byte that leaves the window
    let leaving = (1..BLOCK).fold(1u32, |power, _| power.wrapping_mul(PRIME));
    let mut pending = 0; // where the bytes to insert start
    let mut at = 0;
    let mut hash = target.get(..BLOCK).map(block_hash);
    while at + BLOCK <= target.len() {
        let window = hash.unwrap_or_default();
        let mut best = (0usize, 0usize, 0usize); // base offset, target start, length
        let mut entry = index.heads[index.bucket(window)];
        while entry != 0 {
            let offset = index.offsets[entry as usize - 1] as usize;
            entry = index.next[entry as usize - 1];
            if base[offset..offset + BLOCK] != target[at..at + BLOCK] {
                continue;
            }
            let mut length = BLOCK;
            while offset + length < base.len()
                && at + length < target.len()
                && base[offset + length] == target[at + length]
            {
                length += 1;
            }
            // back into the bytes not yet written
            let mut back = 0;
            while at - back > pending && offset > back && base[offset - back - 1] == target[at - back - 1] {
                back += 1;
            }
            if length + back > best.2 {
                best = (offset - back, at - back, length + back);
            }
            if best.2 >= 4096 {
                break;
            }
        }
        if best.2 > 0 {
            let (offset, start, length) = best;
            push_insert(&mut out, &target[pending..start]);
            push_copy(&mut out, offset, length);
            at = start + length;
            pending = at;
            hash = target.get(at..at + BLOCK).map(block_hash);
        } else {
            if at + BLOCK < target.len() {
                let rolled = window
                    .wrapping_sub(u32::from(target[at]).wrapping_mul(leaving))
                    .wrapping_mul(PRIME)
                    .wrapping_add(u32::from(target[at + BLOCK]));
                hash = Some(rolled);
            }
            at += 1;
        }
        if out.len() + (at - pending) > max_size {
            return None;
        }
    }
    push_insert(&mut out, &target[pending..]);
    (out.len() <= max_size).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small random number generator, so the cases are the same each run.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, limit: usize) -> usize {
            (self.next() % limit.max(1) as u64) as usize
        }

        fn bytes(&mut self, count: usize) -> Vec<u8> {
            (0..count).map(|_| self.next() as u8).collect()
        }
    }

    fn round_trip(base: &[u8], target: &[u8]) -> usize {
        let delta = encode(base, target, usize::MAX).expect("a delta");
        assert_eq!(apply(base, &delta, Some(target.len() as u64)).unwrap(), target);
        assert_eq!(sizes(&delta).unwrap(), (base.len() as u64, target.len() as u64));
        delta.len()
    }

    #[test]
    fn edits_of_text_make_small_deltas() {
        let base: String = (0..2000)
            .map(|line| format!("line {line} of a file that changes a little\n"))
            .collect();
        let mut target = base.replace("line 1000 ", "line one thousand ");
        target.insert_str(0, "a new first line\n");
        target.push_str("and a new last line\n");
        let size = round_trip(base.as_bytes(), target.as_bytes());
        assert!(size < 200, "a delta of {size} bytes for three edits");
        // moved blocks are copied from where they are
        let (first, second) = base.split_at(base.len() / 2);
        let swapped = format!("{second}{first}");
        assert!(round_trip(base.as_bytes(), swapped.as_bytes()) < 64);
    }

    #[test]
    fn random_edits_round_trip() {
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        for case in 0..300 {
            let length = rng.below(5000) + BLOCK;
            let base = rng.bytes(length);
            let mut target = base.clone();
            for _ in 0..rng.below(8) {
                let at = rng.below(target.len());
                match rng.below(3) {
                    0 => {
                        let length = rng.below(300);
                        let inserted = rng.bytes(length);
                        target.splice(at..at, inserted);
                    }
                    1 => {
                        let end = (at + rng.below(300)).min(target.len());
                        target.drain(at..end);
                    }
                    _ => {
                        let from = rng.below(base.len());
                        let end = (from + rng.below(500)).min(base.len());
                        target.splice(at..at, base[from..end].iter().copied());
                    }
                }
            }
            let delta = encode(&base, &target, usize::MAX).unwrap();
            assert_eq!(apply(&base, &delta, None).unwrap(), target, "case {case}");
        }
    }

    #[test]
    fn copies_at_the_limits() {
        let mut rng = Rng(7);
        // a copy longer than one instruction takes, and offsets past 24 bits
        let base = rng.bytes((1 << 24) + 3 * MAX_COPY);
        let mut target = base[(1 << 24)..].to_vec();
        target.extend_from_slice(&base[..2 * MAX_COPY + 5]);
        let size = round_trip(&base, &target);
        assert!(size < 64, "{size}");
        // an empty target, and one that shares nothing
        round_trip(&base[..100], b"");
        round_trip(&base[..100], &rng.bytes(1000));
    }

    #[test]
    fn repeated_data_is_quick() {
        let base = vec![0u8; 16 * 1024 * 1024];
        let mut target = base.clone();
        target[8 * 1024 * 1024] = 1;
        let started = std::time::Instant::now();
        let size = round_trip(&base, &target);
        assert!(size < 2048, "{size}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_delta_over_the_limit_is_not_made() {
        let mut rng = Rng(11);
        let base = rng.bytes(4096);
        let other = rng.bytes(4096);
        assert!(encode(&base, &other, 3072).is_none());
        assert!(
            encode(b"short", b"short target", 100).is_none(),
            "a base too short to index"
        );
    }

    #[test]
    fn damaged_deltas_are_errors() {
        let base = b"0123456789abcdef0123456789abcdef".to_vec();
        let target = b"0123456789abcdef!0123456789abcdef".to_vec();
        let delta = encode(&base, &target, usize::MAX).unwrap();
        assert!(apply(&base[1..], &delta, None).is_err(), "the wrong base size");
        assert!(apply(&base, &delta, Some(5)).is_err(), "the wrong result size");
        assert!(apply(&base, &delta[..delta.len() - 1], None).is_err(), "cut short");
        let mut reserved = delta[..2].to_vec();
        reserved.push(0);
        assert!(apply(&base, &reserved, None).is_err(), "instruction 0");
        let mut past = Vec::new();
        push_size(&mut past, base.len() as u64);
        push_size(&mut past, 8);
        push_copy(&mut past, 30, 8);
        assert!(apply(&base, &past, None).is_err(), "a copy past the end of the base");
        let mut more = Vec::new();
        push_size(&mut more, base.len() as u64);
        push_size(&mut more, 4);
        push_insert(&mut more, b"too long");
        assert!(apply(&base, &more, None).is_err(), "more than the header says");
    }
}
