#[derive(Debug, Clone, Copy)]
pub(crate) struct SseEventBoundary {
    event_len: usize,
    separator_len: usize,
}

impl SseEventBoundary {
    pub(crate) fn event_len(self) -> usize {
        self.event_len
    }

    #[cfg(test)]
    pub(crate) fn separator_len(self) -> usize {
        self.separator_len
    }
}

#[derive(Debug, Default)]
pub(crate) struct SseEventBuffer {
    bytes: Vec<u8>,
    start: usize,
    scan_from: usize,
    #[cfg(test)]
    framing_work_units: usize,
}

impl SseEventBuffer {
    fn extend_from_slice(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    pub(crate) fn extend_chunk_and_find_boundary(
        &mut self,
        bytes: &[u8],
        separators: &[&[u8]],
    ) -> Option<SseEventBoundary> {
        if bytes.is_empty() {
            return None;
        }
        self.extend_from_slice(bytes);
        // A newline-free byte can still confirm a CR-ending separator that was
        // deliberately deferred at the preceding chunk edge.
        self.next_boundary(separators, false)
    }

    pub(crate) fn active_bytes(&self) -> &[u8] {
        &self.bytes[self.start..]
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.start == self.bytes.len()
    }

    pub(crate) fn next_boundary(
        &mut self,
        separators: &[&[u8]],
        end_of_stream: bool,
    ) -> Option<SseEventBoundary> {
        for position in self.scan_from.max(self.start)..self.bytes.len() {
            #[cfg(test)]
            {
                self.framing_work_units += 1;
            }
            for separator in separators {
                let ends_at_buffer_edge = position + separator.len() == self.bytes.len();
                // Keep every CR-ending match pending at an open chunk edge. In
                // particular, `\r\r` may become the preferred `\r\r\n` separator.
                if self.bytes[position..].starts_with(separator)
                    && (end_of_stream || !separator.ends_with(b"\r") || !ends_at_buffer_edge)
                {
                    return Some(SseEventBoundary {
                        event_len: position - self.start,
                        separator_len: separator.len(),
                    });
                }
            }
        }
        let separator_overlap = separators
            .iter()
            .map(|separator| {
                if separator.ends_with(b"\r") {
                    separator.len()
                } else {
                    separator.len().saturating_sub(1)
                }
            })
            .max()
            .unwrap_or_default();
        self.scan_from = self
            .bytes
            .len()
            .saturating_sub(separator_overlap)
            .max(self.start);
        None
    }

    pub(crate) fn take_event(&mut self, boundary: SseEventBoundary) -> Vec<u8> {
        let event_end = self.start + boundary.event_len;
        let event = self.bytes[self.start..event_end].to_vec();
        self.start = event_end + boundary.separator_len;
        self.scan_from = self.start;
        #[cfg(test)]
        {
            self.framing_work_units += event.len();
        }
        self.compact_if_worthwhile();
        event
    }

    fn compact_if_worthwhile(&mut self) {
        const MIN_COMPACTION_BYTES: usize = 64 * 1024;

        if self.start == self.bytes.len() {
            self.bytes.clear();
            self.start = 0;
            self.scan_from = 0;
            return;
        }
        if self.start < MIN_COMPACTION_BYTES || self.start < self.bytes.len() / 2 {
            return;
        }

        let remaining = self.bytes.len() - self.start;
        self.bytes.copy_within(self.start.., 0);
        self.bytes.truncate(remaining);
        self.start = 0;
        self.scan_from = 0;
        #[cfg(test)]
        {
            self.framing_work_units += remaining;
        }
    }

    #[cfg(test)]
    fn framing_work_units(&self) -> usize {
        self.framing_work_units
    }
}

#[cfg(test)]
pub(crate) fn assert_sse_framing_work_is_linear(separators: &[&[u8]]) {
    const INPUT_BYTES: usize = 16 * 1024;
    const MAX_LINEAR_WORK_MULTIPLIER: usize = 8;

    let mut dense = SseEventBuffer::default();
    let mut next_boundary =
        dense.extend_chunk_and_find_boundary(&vec![b'\n'; INPUT_BYTES], separators);
    let mut event_count = 0_usize;
    while let Some(boundary) = next_boundary {
        assert!(dense.take_event(boundary).is_empty());
        event_count += 1;
        next_boundary = dense.next_boundary(separators, false);
    }
    assert_eq!(event_count, INPUT_BYTES / 2);
    assert!(dense.is_empty());
    assert!(
        dense.framing_work_units() <= INPUT_BYTES * MAX_LINEAR_WORK_MULTIPLIER,
        "dense SSE framing performed {} work units for {INPUT_BYTES} input bytes",
        dense.framing_work_units(),
    );

    let mut fragmented = SseEventBuffer::default();
    for _ in 0..(INPUT_BYTES / 2) {
        assert!(
            fragmented
                .extend_chunk_and_find_boundary(b"x\n", separators)
                .is_none()
        );
    }
    assert_eq!(fragmented.active_bytes().len(), INPUT_BYTES);
    assert!(
        fragmented.framing_work_units() <= INPUT_BYTES * MAX_LINEAR_WORK_MULTIPLIER,
        "fragmented SSE framing performed {} work units for {INPUT_BYTES} input bytes",
        fragmented.framing_work_units(),
    );

    let mut newline_free = SseEventBuffer::default();
    for _ in 0..INPUT_BYTES {
        assert!(
            newline_free
                .extend_chunk_and_find_boundary(b"x", separators)
                .is_none()
        );
    }
    assert_eq!(newline_free.active_bytes().len(), INPUT_BYTES);
    assert!(
        newline_free.framing_work_units() <= INPUT_BYTES * MAX_LINEAR_WORK_MULTIPLIER,
        "newline-free SSE framing performed {} work units for {INPUT_BYTES} input bytes",
        newline_free.framing_work_units(),
    );
    let work_before_empty_chunks = newline_free.framing_work_units();
    for _ in 0..INPUT_BYTES {
        assert!(
            newline_free
                .extend_chunk_and_find_boundary(b"", separators)
                .is_none()
        );
    }
    assert_eq!(
        newline_free.framing_work_units(),
        work_before_empty_chunks,
        "empty chunks must not trigger repeated suffix scans"
    );
}

#[cfg(test)]
pub(crate) fn assert_edge_deferred_separator_is_rescanned_after_newline_free_chunk(
    separators: &[&[u8]],
) {
    for separator in [b"\n\r".as_slice(), b"\r\n\r".as_slice()] {
        let mut pending = SseEventBuffer::default();
        let mut first_chunk = b"data: x".to_vec();
        first_chunk.extend_from_slice(separator);
        assert!(
            pending
                .extend_chunk_and_find_boundary(&first_chunk, separators)
                .is_none(),
            "edge-ending separator must remain deferred"
        );

        let boundary = pending
            .extend_chunk_and_find_boundary(b"x", separators)
            .expect("newline-free continuation must confirm the deferred separator");
        assert_eq!(pending.take_event(boundary), b"data: x");
        assert_eq!(pending.active_bytes(), b"x");
    }
}

#[cfg(test)]
pub(crate) fn assert_crcrlf_split_consumes_the_continuation_lf(separators: &[&[u8]]) {
    let mut pending = SseEventBuffer::default();
    assert!(
        pending
            .extend_chunk_and_find_boundary(b"data: terminal\r\r", separators)
            .is_none(),
        "CRCR must wait for a possible LF continuation at a chunk edge"
    );
    let boundary = pending
        .extend_chunk_and_find_boundary(b"\n", separators)
        .expect("the continuation LF must complete the CRCRLF separator");
    assert_eq!(boundary.separator_len(), 3);
    assert_eq!(pending.take_event(boundary), b"data: terminal");
    assert!(pending.is_empty());
}
