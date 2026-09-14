//! Bounded replay ring for a PTY's output (#287).
//!
//! Every byte a child writes to its PTY passes through one of these rings, whether or not
//! a UI is attached (#207). The ring serves two consumers with different needs:
//!
//! * **Inject** (`recent_output`) wants a small, lossily decoded tail to compare before and
//!   after a keystroke. That view is fixed at [`RECENT_OUTPUT_VIEW_BYTES`] so its
//!   behaviour is unchanged by how much history the ring retains.
//! * **Terminal replay** (`snapshot`) wants as much history as the ring holds, addressed
//!   by absolute byte offset so a freshly mounted pane can splice the snapshot and the
//!   live event stream together without gaps or duplicates.
//!
//! Offsets are monotonic counts of bytes recorded since the session started. A chunk
//! recorded when `total_written == N` starts at offset `N`; a snapshot taken afterwards
//! reports `offset_end == N + len`. The frontend applies live chunks whose offset is at
//! or past the snapshot's `offset_end` and drops the rest.

use std::collections::VecDeque;

/// Default bytes of history retained per PTY. Sized for several minutes of a busy TUI.
pub const DEFAULT_REPLAY_CAPACITY: usize = 512 * 1024;
/// Smallest configurable ring: the legacy diagnostic tail.
pub const MIN_REPLAY_CAPACITY: usize = 8 * 1024;
/// Largest configurable ring, bounding `agents × cap` memory.
pub const MAX_REPLAY_CAPACITY: usize = 1024 * 1024;
/// Bytes of the diagnostic tail exposed through `recent_output` for inject (#207/#256).
pub const RECENT_OUTPUT_VIEW_BYTES: usize = 8 * 1024;

/// Clamp an operator-supplied replay capacity into the supported range.
pub fn clamp_replay_capacity(requested: usize) -> usize {
    requested.clamp(MIN_REPLAY_CAPACITY, MAX_REPLAY_CAPACITY)
}

/// The retained history of a PTY, aligned so it can be written straight into a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSnapshot {
    /// Absolute offset of the first byte in `data`.
    pub offset_start: u64,
    /// Absolute offset one past the last byte in `data`; equal to the ring's total.
    pub offset_end: u64,
    pub data: Vec<u8>,
}

pub struct OutputRing {
    buf: VecDeque<u8>,
    capacity: usize,
    total: u64,
}

impl OutputRing {
    pub fn new(capacity: usize) -> Self {
        let capacity = clamp_replay_capacity(capacity);
        Self {
            buf: VecDeque::with_capacity(capacity.min(64 * 1024)),
            capacity,
            total: 0,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Bytes recorded since the session started, including evicted ones.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn total_written(&self) -> u64 {
        self.total
    }

    /// Bytes currently retained.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Append `bytes`, evicting the oldest retained bytes once the ring is full.
    /// Returns the absolute offset at which `bytes` starts.
    pub fn record(&mut self, bytes: &[u8]) -> u64 {
        let start = self.total;
        let len = bytes.len();

        if len >= self.capacity {
            self.buf.clear();
            self.buf.extend(&bytes[len - self.capacity..]);
        } else {
            let overflow = (self.buf.len() + len).saturating_sub(self.capacity);
            if overflow > 0 {
                self.buf.drain(..overflow);
            }
            self.buf.extend(bytes);
        }

        self.total += len as u64;
        start
    }

    /// The last `max_bytes` retained bytes, lossily decoded. This is the inject view.
    pub fn tail_lossy(&self, max_bytes: usize) -> String {
        let skip = self.buf.len().saturating_sub(max_bytes);
        let bytes: Vec<u8> = self.buf.iter().skip(skip).copied().collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Everything retained, trimmed to a boundary a terminal parser can start from.
    pub fn snapshot(&self) -> OutputSnapshot {
        let (front, back) = self.buf.as_slices();
        let mut bytes = Vec::with_capacity(self.buf.len());
        bytes.extend_from_slice(front);
        bytes.extend_from_slice(back);

        let start = aligned_start(&bytes);
        let retained = (bytes.len() - start) as u64;
        OutputSnapshot {
            offset_start: self.total - retained,
            offset_end: self.total,
            data: bytes.split_off(start),
        }
    }
}

/// Index of the first byte a fresh terminal can safely parse from.
///
/// Ring eviction cuts the stream at an arbitrary byte, so the retained head may be the
/// tail of a control sequence (`2;5H` from a cursor move, the rest of an OSC title) or a
/// continuation byte of a multi-byte character. Feeding either to xterm paints garbage.
///
/// Rule: start at the first ESC (0x1B) when there is one. ESC never appears inside a
/// UTF-8 multi-byte encoding, and every sequence that begins at a retained ESC is intact
/// because all later bytes are retained too (the ring only evicts from the front). A
/// stream that has no ESC at all is plain text, so we only skip UTF-8 continuation bytes.
pub fn aligned_start(bytes: &[u8]) -> usize {
    if let Some(esc) = bytes.iter().position(|byte| *byte == 0x1b) {
        return esc;
    }
    bytes
        .iter()
        .position(|byte| byte & 0xC0 != 0x80)
        .unwrap_or(bytes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_returns_the_chunk_start_offset_and_advances_the_total() {
        let mut ring = OutputRing::new(MIN_REPLAY_CAPACITY);
        assert_eq!(ring.record(b"hello"), 0);
        assert_eq!(ring.record(b" world"), 5);
        assert_eq!(ring.total_written(), 11);
        assert_eq!(ring.snapshot().data, b"hello world");
    }

    #[test]
    fn capacity_is_clamped_to_the_supported_range() {
        assert_eq!(OutputRing::new(1).capacity(), MIN_REPLAY_CAPACITY);
        assert_eq!(OutputRing::new(usize::MAX).capacity(), MAX_REPLAY_CAPACITY);
        assert_eq!(
            OutputRing::new(DEFAULT_REPLAY_CAPACITY).capacity(),
            DEFAULT_REPLAY_CAPACITY
        );
        assert_eq!(clamp_replay_capacity(0), MIN_REPLAY_CAPACITY);
    }

    #[test]
    fn eviction_keeps_the_newest_bytes_and_offsets_stay_absolute() {
        let mut ring = OutputRing::new(MIN_REPLAY_CAPACITY);
        let capacity = ring.capacity();
        let first = vec![b'a'; capacity - 10];
        ring.record(&first);
        let offset = ring.record(&vec![b'b'; 20]);

        assert_eq!(offset, (capacity - 10) as u64);
        assert_eq!(ring.len(), capacity);
        let snapshot = ring.snapshot();
        assert_eq!(snapshot.offset_end, (capacity + 10) as u64);
        assert_eq!(snapshot.offset_start, 10);
        assert!(snapshot.data.ends_with(&[b'b'; 20]));
        assert_eq!(snapshot.data.len(), capacity);
    }

    #[test]
    fn a_chunk_larger_than_the_ring_keeps_only_its_tail() {
        let mut ring = OutputRing::new(MIN_REPLAY_CAPACITY);
        let capacity = ring.capacity();
        let huge: Vec<u8> = (0..(capacity * 2)).map(|i| (i % 251) as u8).collect();
        ring.record(&huge);

        assert_eq!(ring.len(), capacity);
        assert_eq!(ring.total_written(), (capacity * 2) as u64);
        let snapshot = ring.snapshot();
        let start = snapshot.offset_start as usize;
        assert_eq!(&huge[start..], snapshot.data.as_slice());
    }

    #[test]
    fn the_recent_output_view_is_the_ring_tail() {
        let mut ring = OutputRing::new(DEFAULT_REPLAY_CAPACITY);
        let payload: Vec<u8> = (0..(RECENT_OUTPUT_VIEW_BYTES * 3))
            .map(|i| b'a' + (i % 26) as u8)
            .collect();
        ring.record(&payload);

        let view = ring.tail_lossy(RECENT_OUTPUT_VIEW_BYTES);
        assert_eq!(view.len(), RECENT_OUTPUT_VIEW_BYTES);
        assert_eq!(
            view.as_bytes(),
            &payload[payload.len() - RECENT_OUTPUT_VIEW_BYTES..]
        );
        assert_eq!(ring.snapshot().data, payload, "the full ring keeps more than the view");
    }

    #[test]
    fn snapshot_never_starts_inside_a_csi_sequence() {
        let mut ring = OutputRing::new(MIN_REPLAY_CAPACITY);
        let capacity = ring.capacity();
        let frame = b"\x1b[12;34Hafter\x1b[2Jtail";
        ring.record(frame);
        // The ring evicts from the front, so bytes recorded AFTER the frame are what
        // push the cut into the middle of `ESC [ 12 ; 34 H` (three bytes past its start).
        let padding = vec![b'y'; capacity - frame.len() + 3];
        ring.record(&padding);

        let snapshot = ring.snapshot();
        assert!(
            snapshot.data.starts_with(b"\x1b[2Jtail"),
            "got {:?}",
            String::from_utf8_lossy(&snapshot.data[..16])
        );
        assert!(snapshot.data.ends_with(b"yyyy"));
        assert_eq!(
            snapshot.offset_end - snapshot.offset_start,
            snapshot.data.len() as u64
        );
        assert_eq!(snapshot.offset_end, ring.total_written());
        assert_eq!(snapshot.offset_start, 13, "13 bytes precede the second ESC");
    }

    #[test]
    fn snapshot_never_starts_inside_an_osc_string() {
        let mut ring = OutputRing::new(MIN_REPLAY_CAPACITY);
        let capacity = ring.capacity();
        let frame = b"\x1b]0;window title\x07\x1b[Hbody";
        ring.record(frame);
        // Cut six bytes into the OSC string: `ESC ] 0 ; w i` are evicted.
        let padding = vec![b'y'; capacity - frame.len() + 6];
        ring.record(&padding);

        let snapshot = ring.snapshot();
        assert!(snapshot.data.starts_with(b"\x1b[Hbody"), "got {:?}", &snapshot.data[..12]);
        assert_eq!(snapshot.data.len(), b"\x1b[Hbody".len() + padding.len());
    }

    #[test]
    fn snapshot_keeps_a_sequence_that_starts_exactly_at_the_ring_head() {
        let mut ring = OutputRing::new(MIN_REPLAY_CAPACITY);
        let capacity = ring.capacity();
        ring.record(&vec![b'x'; capacity]);
        let frame = b"\x1b[H\x1b[2Jfull frame";
        ring.record(frame);
        // Evict exactly the padding so the ring begins at the ESC.
        let snapshot = ring.snapshot();
        assert!(snapshot.data.ends_with(frame));
        assert_eq!(snapshot.data[0], 0x1b);
        assert_eq!(snapshot.data.len(), frame.len());
    }

    #[test]
    fn plain_text_snapshot_skips_a_split_multibyte_character() {
        // "é" is C3 A9; leave only the continuation byte at the head.
        assert_eq!(aligned_start(b"\xa9plain"), 1);
        assert_eq!(aligned_start(b"plain"), 0);
        assert_eq!(aligned_start(b"\x80\x80"), 2);
        assert_eq!(aligned_start(b""), 0);
    }

    #[test]
    fn empty_ring_snapshot_is_empty_with_zero_offsets() {
        let ring = OutputRing::new(MIN_REPLAY_CAPACITY);
        assert!(ring.is_empty());
        assert_eq!(
            ring.snapshot(),
            OutputSnapshot { offset_start: 0, offset_end: 0, data: Vec::new() }
        );
    }
}
