# FINAL-EFFICIENCY-20260912: native execution and provider transport

Baseline e32c13c5b04c3d5db1b60637be115c2b61fcfb78, initially clean main,
now codex/final-resource-efficiency-20260912. Inventory completed with rg.

## Provider edit map

- Targets: providers network_transport.rs, new private transport-pool helper and
  tests, url_policy.rs documentation only. Existing public Provider and
  ResolvedNetworkTarget methods stay in place; no public symbol/dependency move.
- Entry: ProviderHttpTarget::prepare, called by all adapters and embeddings.
- Accept PERF-03/F07 client reuse; keep both DNS lookups, complete answer-set
  equality, pinning, no proxy/redirect, request-local credential headers and
  actual peer validation. Relaxing DNS checks is not accepted.
- Cache key: exact canonical URL (including complete typed policy), pinned
  socket addresses and timeout. Only credential-free transport is cached.
  Current Core connection/credential authority is still checked per dispatch;
  an equal transport key does not carry approval or credential authority.
- Bounded cache/expiry and idle connections; DNS/network work stays outside its
  map lock. Policy/address drift misses the cache. Endpoint construction remains
  lazy and every request validates the current target before dispatch.
- Expected delta: <=250-line private helper and focused tests; current facade
  delegates client creation. No cap increase or archived report edits.
- Tests before/after: providers lib and loopback actual public-provider probe;
  focused pool partition/expiry/admission tests and existing URL policy tests.

## Core edit map

- Targets: app/runtime_control.rs or a new private bounded blocking helper,
  app/generation_workflow.rs and generation/delivery.rs, owned lifecycle caller
  wiring and regression tests. Storage remains transaction owner.
- Entry: execute_generation_task / forward_generation_events / terminal
  persistence. Existing Core public contracts remain in place.
- Accept PERF-02: blocking DB wait/fsync/terminal transformation must not occupy
  the two network/runtime worker threads. Queue and running work stay bounded;
  permits follow actual native work, never just a cancelled awaiter.
- Preserve checkpoint interval/byte threshold, ordered forwarding, final flush,
  terminal-persistence-before-event, unknown outcome classification and shutdown.
- PERF-08 uses the accepted schema 0043 suffix journal after review of triggers,
  live pending-message reads and restart recovery. The forwarder passes only
  new UTF-8 bytes with the last successful durable offset. The crash-loss
  interval and FULL durability remain unchanged.
- Tests: core generation/events/lifecycle and chat vertical slices, deliberate
  slow blocking operation with async progress/cancellation and bounded admission.

The same executor also covers the memory supervisor's periodic claim and
credential-free summary preparation in orchestration_runtime/auxiliary_tasks.rs.
Those are the repeated DB scan and source transformation paths; the broker and
network dispatch remain async. The helper borrows its calling Core through the
await so the worker's clone cannot become the last Core owner during shutdown.
No source symbols move and no new public entry point is introduced.

Governing material: root/Core/Storage AGENTS, ADR 0001-0006 and current contract
manifests. Source cap extractions, if needed, will be recorded before movement.

## Implemented bounds and regression evidence

The shared transport cache holds at most 32 credential-free clients, expires
entries after a fixed 60 seconds, and keeps at most two idle connections per
host for 30 seconds. Exact URL/policy/address/timeout changes do not reuse an
entry. The public-provider loopback regression made 20 requests over one TCP
connection and verified request-local synthetic credentials A/B/none. A second
regression closes the first Tokio runtime and successfully reconnects from a
new runtime, without retaining a stale reactor-bound socket.

Core permits at most two actual blocking jobs. The current-thread runtime
regression verifies asynchronous progress while they run, and verifies that
cancelling an awaiter does not release its still-running job's permit. The
forwarder awaits checkpoint work in order and advances its offset only on
success. It drains before terminal persistence, which keeps the active
generation guard until the actual write finishes. Credentials are dropped
before that terminal closure begins.

An independent final integration review found no additional confirmed
regression in these ordering and transport boundaries. Complete local gate
results are recorded in final-efficiency-review.md; neither synthetic connection
counts nor executor tests establish device-wide latency or battery savings.
