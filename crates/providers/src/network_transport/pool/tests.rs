use super::*;
use crate::url_policy::{ApprovedLocalNetworkOrigin, UrlPolicy};
use std::{cell::Cell, net::IpAddr};

fn client() -> CoreResult<Client> {
    Client::builder()
        .no_proxy()
        .build()
        .map_err(|_| client_error())
}

#[test]
fn reuses_only_exact_target_policy_pin_timeout_and_unexpired_transport() {
    let canonical = UrlPolicy::public()
        .canonicalize("https://example.com/v1")
        .unwrap();
    let first = ["93.184.216.34:443".parse().unwrap()];
    let second = ["93.184.216.35:443".parse().unwrap()];
    let now = Instant::now();
    let timeout = Duration::from_secs(20);
    let builds = Cell::new(0);
    let build = || {
        builds.set(builds.get() + 1);
        client()
    };
    let mut pool = TransportPool::default();
    for _ in 0..3 {
        pool.client(&canonical, &first, timeout, now, build)
            .unwrap();
    }
    assert_eq!(builds.get(), 1);
    pool.client(&canonical, &second, timeout, now, build)
        .unwrap();
    assert_eq!(
        builds.get(),
        2,
        "changed pinned addresses require a new client"
    );
    assert_eq!(pool.entries.len(), 1, "retire the obsolete pin");
    pool.client(
        &canonical,
        &second,
        timeout + Duration::from_secs(1),
        now,
        build,
    )
    .unwrap();
    assert_eq!(
        builds.get(),
        3,
        "request timeout is part of the transport contract"
    );
    pool.client(
        &canonical,
        &second,
        timeout + Duration::from_secs(1),
        now + TRANSPORT_LIFETIME,
        build,
    )
    .unwrap();
    assert_eq!(
        builds.get(),
        4,
        "fixed lifetime must not extend on cache hits"
    );
}

#[test]
fn bounded_cache_evicts_oldest_unused_transport_and_never_reuses_a_different_url() {
    let mut pool = TransportPool::default();
    let addresses = ["127.0.0.1:1234".parse().unwrap()];
    let timeout = Duration::from_secs(1);
    let now = Instant::now();
    for i in 0..MAX_TRANSPORTS + 10 {
        let url = UrlPolicy::local_loopback()
            .canonicalize(&format!("http://127.0.0.1:1234/v{i}"))
            .unwrap();
        pool.client(&url, &addresses, timeout, now, client).unwrap();
        assert!(pool.entries.len() <= MAX_TRANSPORTS);
    }
    assert!(
        pool.entries
            .front()
            .unwrap()
            .canonical
            .as_str()
            .ends_with("/v10")
    );
}

#[test]
fn identical_url_does_not_cross_approved_network_policy() {
    let address: IpAddr = "192.168.1.2".parse().unwrap();
    let other: IpAddr = "192.168.1.3".parse().unwrap();
    let origin = "http://models.lan:1234";
    let first = ApprovedLocalNetworkOrigin::new(origin, &[address]).unwrap();
    let second = ApprovedLocalNetworkOrigin::new(origin, &[address, other]).unwrap();
    let first = UrlPolicy::approved_local_network(first)
        .canonicalize(origin)
        .unwrap();
    let second = UrlPolicy::approved_local_network(second)
        .canonicalize(origin)
        .unwrap();
    let mut pool = TransportPool::default();
    let now = Instant::now();
    let addresses = ["192.168.1.2:1234".parse().unwrap()];
    let builds = Cell::new(0);
    let build = || {
        builds.set(builds.get() + 1);
        client()
    };
    pool.client(&first, &addresses, Duration::from_secs(1), now, build)
        .unwrap();
    pool.client(&second, &addresses, Duration::from_secs(1), now, build)
        .unwrap();
    assert_eq!(first.as_str(), second.as_str());
    assert_eq!(
        builds.get(),
        2,
        "equal endpoint/pin with different full policy must miss"
    );
}
