use super::tests::descriptor;
use super::*;
use std::{
    io::Write,
    sync::{
        Barrier, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};
use tempfile::NamedTempFile;

#[test]
fn busy_asset_does_not_block_another_slot_or_table_lookup() {
    let mut source = NamedTempFile::new().unwrap();
    source.write_all(b"01234567").unwrap();
    let a = descriptor(&"ab".repeat(32), 8);
    let b = descriptor(&"cd".repeat(32), 8);
    let cache = Mutex::new(VerifiedAssetCache::default());
    let first = cache.lock().unwrap().lease(&a).unwrap();
    let held = first.lock().unwrap();
    thread::scope(|scope| {
        let (send, receive) = mpsc::channel();
        scope.spawn(move || {
            let other = cache.lock().unwrap().lease(&b).unwrap();
            let mut entry = other.lock().unwrap();
            let _job = cache.lock().unwrap().begin_verification(8).unwrap();
            entry
                .insert(AssetFileSnapshot::capture(source.reopen().unwrap()).unwrap())
                .unwrap();
            let CacheLookup::Hit(bytes) = entry.read_range(2, 3).unwrap() else {
                panic!("hit")
            };
            send.send(bytes).unwrap();
        });
        assert_eq!(
            receive
                .recv_timeout(Duration::from_secs(2))
                .expect("other digest must progress while first is held"),
            b"234"
        );
    });
    drop(held);
}

#[test]
fn same_asset_waiters_share_one_verification_and_serial_seek() {
    let mut source = NamedTempFile::new().unwrap();
    source.write_all(b"01234567").unwrap();
    let descriptor = descriptor(&"ef".repeat(32), 8);
    let cache = Mutex::new(VerifiedAssetCache::default());
    let ready = Barrier::new(8);
    let hashes = AtomicUsize::new(0);
    thread::scope(|scope| {
        for index in 0..8 {
            let source = &source;
            let cache = &cache;
            let ready = &ready;
            let hashes = &hashes;
            let descriptor = &descriptor;
            scope.spawn(move || {
                ready.wait();
                let lease = cache.lock().unwrap().lease(descriptor).unwrap();
                let mut entry = lease.lock().unwrap();
                if matches!(entry.contains_verified().unwrap(), CacheLookup::Miss) {
                    let _job = cache.lock().unwrap().begin_verification(8).unwrap();
                    hashes.fetch_add(1, Ordering::Relaxed);
                    entry
                        .insert(AssetFileSnapshot::capture(source.reopen().unwrap()).unwrap())
                        .unwrap();
                }
                let CacheLookup::Hit(bytes) = entry.read_range(index, 1).unwrap() else {
                    panic!("hit")
                };
                assert_eq!(bytes, vec![b'0' + u8::try_from(index).unwrap()]);
            });
        }
    });
    assert_eq!(hashes.load(Ordering::Relaxed), 1);
}

#[test]
fn held_slots_are_not_evicted_and_busy_jobs_do_not_spend_hash_credit() {
    let mut cache = VerifiedAssetCache::new(2, Duration::from_mins(1));
    let a = cache.lease(&descriptor(&"ab".repeat(32), 8)).unwrap();
    let b = cache.lease(&descriptor(&"cd".repeat(32), 8)).unwrap();
    let third = descriptor(&"ef".repeat(32), 8);
    assert_eq!(
        cache.lease(&third).err().unwrap().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(cache.entries.len(), 2);
    drop(a);
    let c = cache.lease(&third).unwrap();
    assert_eq!(cache.entries.len(), 2);
    drop((b, c));
    let jobs = (0..4)
        .map(|_| cache.begin_verification(1).unwrap())
        .collect::<Vec<_>>();
    let budget = cache.verification_budget.remaining();
    assert_eq!(
        cache.begin_verification(1).err().unwrap().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(cache.verification_budget.remaining(), budget);
    drop(jobs);
    let _released = cache
        .begin_verification(1)
        .expect("job permits return on drop");
}
