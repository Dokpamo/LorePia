//! Credential-free transport reuse. A hit never substitutes for fresh target
//! resolution, DNS revalidation or the response's connected-peer check.

use std::{
    collections::VecDeque,
    net::SocketAddr,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use lorepia_domain::CoreResult;
use reqwest::Client;

use super::{CanonicalUrl, ResolvedNetworkTarget, client_error};

const MAX_TRANSPORTS: usize = 32;
const TRANSPORT_LIFETIME: Duration = Duration::from_mins(1);
const IDLE_CONNECTION_LIFETIME: Duration = Duration::from_secs(30);
const MAX_IDLE_CONNECTIONS: usize = 2;

struct Entry {
    canonical: CanonicalUrl,
    addresses: Vec<SocketAddr>,
    timeout: Duration,
    created_at: Instant,
    client: Client,
}

#[derive(Default)]
struct TransportPool {
    entries: VecDeque<Entry>,
}

impl TransportPool {
    fn client(
        &mut self,
        canonical: &CanonicalUrl,
        addresses: &[SocketAddr],
        timeout: Duration,
        now: Instant,
        build: impl FnOnce() -> CoreResult<Client>,
    ) -> CoreResult<Client> {
        self.entries.retain(|entry| {
            now.saturating_duration_since(entry.created_at) < TRANSPORT_LIFETIME
                // An observed pin/config change retires the old transport for
                // that exact endpoint. Already admitted requests own their lease.
                && (entry.canonical != *canonical
                    || (entry.addresses == addresses && entry.timeout == timeout))
        });
        if let Some(index) = self.entries.iter().position(|entry| {
            entry.canonical == *canonical
                && entry.addresses == addresses
                && entry.timeout == timeout
        }) {
            let entry = self
                .entries
                .remove(index)
                .expect("matched transport exists");
            let client = entry.client.clone();
            self.entries.push_back(entry);
            return Ok(client);
        }
        let client = build()?;
        if self.entries.len() == MAX_TRANSPORTS {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            canonical: canonical.clone(),
            addresses: addresses.to_vec(),
            timeout,
            created_at: now,
            client: client.clone(),
        });
        Ok(client)
    }
}

pub(super) fn client_for(target: &ResolvedNetworkTarget, timeout: Duration) -> CoreResult<Client> {
    static POOL: OnceLock<Mutex<TransportPool>> = OnceLock::new();
    let mut pool = POOL
        .get_or_init(|| Mutex::new(TransportPool::default()))
        .lock()
        .map_err(|_| client_error())?;
    pool.client(
        target.url(),
        target.socket_addresses(),
        timeout,
        Instant::now(),
        || {
            let builder = Client::builder()
                .timeout(timeout)
                .connect_timeout(timeout.min(Duration::from_secs(30)))
                .pool_idle_timeout(IDLE_CONNECTION_LIFETIME)
                .pool_max_idle_per_host(MAX_IDLE_CONNECTIONS)
                .redirect(reqwest::redirect::Policy::none())
                .referer(false)
                .no_proxy();
            target
                .pin_reqwest_builder(builder)
                .build()
                .map_err(|_| client_error())
        },
    )
}

#[cfg(test)]
mod tests;
