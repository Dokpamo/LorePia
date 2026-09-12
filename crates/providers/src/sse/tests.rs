use super::*;

const SEPARATORS: [&[u8]; 8] = [
    b"\r\n\r\n",
    b"\n\r\n",
    b"\r\r\n",
    b"\r\n\n",
    b"\r\n\r",
    b"\n\n",
    b"\n\r",
    b"\r\r",
];

#[test]
fn events_borrow_storage_until_the_next_nonempty_append_compacts() {
    let mut pending = SseEventBuffer::default();
    let mut chunk = vec![b'x'; 70_000];
    chunk.extend_from_slice(b"\n\nsecond\n\npartial");
    let boundary = pending
        .extend_chunk_and_find_boundary(&chunk, &SEPARATORS)
        .unwrap();
    let first_address = pending.bytes.as_ptr();
    let event = pending.take_event(boundary);
    assert_eq!(event.as_ptr(), first_address);
    assert_eq!(event.len(), 70_000);
    let boundary = pending.next_boundary(&SEPARATORS, false).unwrap();
    let second_address = pending.bytes[70_002..].as_ptr();
    let event = pending.take_event(boundary);
    assert_eq!(event.as_ptr(), second_address);
    assert_eq!(event, b"second");
    assert_eq!(pending.start, 70_010);
    assert!(pending.next_boundary(&SEPARATORS, false).is_none());
    assert!(
        pending
            .extend_chunk_and_find_boundary(b"", &SEPARATORS)
            .is_none()
    );
    assert_eq!(pending.start, 70_010);
    let boundary = pending
        .extend_chunk_and_find_boundary(b"-end\n\n", &SEPARATORS)
        .unwrap();
    assert_eq!(pending.start, 0);
    assert_eq!(pending.take_event(boundary), b"partial-end");
    assert!(pending.is_empty());
    let retained_capacity = pending.bytes.capacity();
    let boundary = pending
        .extend_chunk_and_find_boundary(b"next\n\n", &SEPARATORS)
        .unwrap();
    assert_eq!(pending.bytes.len(), 6);
    assert_eq!(pending.bytes.capacity(), retained_capacity);
    assert_eq!(pending.take_event(boundary), b"next");
}

fn drain_reference(bytes: &mut Vec<u8>, eof: bool) -> Vec<Vec<u8>> {
    let mut events = Vec::new();
    loop {
        let boundary = (0..bytes.len()).find_map(|offset| {
            SEPARATORS.iter().find_map(|separator| {
                (bytes[offset..].starts_with(separator)
                    && (eof
                        || !separator.ends_with(b"\r")
                        || offset + separator.len() != bytes.len()))
                .then_some((offset, separator.len()))
            })
        });
        let Some((offset, length)) = boundary else {
            break;
        };
        events.push(bytes[..offset].to_vec());
        bytes.drain(..offset + length);
    }
    events
}

fn compare_chunks(input: &[u8], chunk_size: usize) {
    let mut pending = SseEventBuffer::default();
    let mut reference = Vec::new();
    for chunk in input.chunks(chunk_size) {
        reference.extend_from_slice(chunk);
        let expected = drain_reference(&mut reference, false);
        let mut actual = Vec::new();
        let mut boundary = pending.extend_chunk_and_find_boundary(chunk, &SEPARATORS);
        while let Some(next) = boundary {
            actual.push(pending.take_event(next).to_vec());
            boundary = pending.next_boundary(&SEPARATORS, false);
        }
        assert_eq!(actual, expected);
        assert_eq!(pending.active_bytes(), reference);
        assert!(
            pending
                .extend_chunk_and_find_boundary(b"", &SEPARATORS)
                .is_none()
        );
    }
    let expected = drain_reference(&mut reference, true);
    let mut actual = Vec::new();
    while let Some(next) = pending.next_boundary(&SEPARATORS, true) {
        actual.push(pending.take_event(next).to_vec());
    }
    assert_eq!(actual, expected);
    assert_eq!(pending.active_bytes(), reference);
}

#[test]
fn borrowed_framing_matches_reference_for_fragmented_separators_and_trailers() {
    let input = b"\n\ndata: alpha\r\n\r\ndata: beta\r\r\ndata: gamma\n\rdata: delta\r\n\ndata: epsilon\r\n\rtrailer\r";
    for size in 1..=input.len() {
        compare_chunks(input, size);
    }
    for end in 0..=input.len() {
        compare_chunks(&input[..end], 3);
    }
    let mut large = vec![b'x'; 70_000];
    large.extend_from_slice(b"\n\ndata: second\r\r\npartial");
    for size in [1024, 65_536, 70_002, large.len()] {
        compare_chunks(&large, size);
    }
}
