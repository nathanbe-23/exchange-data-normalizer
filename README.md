# exchange-data-normalizer
A low-latency market data normalizer that ingests a websocket feed from multiple cryptocurrency exchanges and publishes a uniefied, sequence-consistent stream of trades to downstream consumers. Built in Rust with a focus on correctness under failure - reconnects, livenes detection, backpressure - so that strategies, risk systems and analytics can consume once canonical schema instead of N exchange-specific ones.

## Why this exists
Every exchange websocket API has its own quirks: different message shapes, symbol formats, timestamp encodings, heartbeat conventions. 
A strategy or risk system that consumes raw exchange feeds ends up reimplementing per-exchange handling badle and inconsistently. This normalizer pushes that complexity into one process that handles it carefully once and exposes a unified canonical stream.

## Latency numbers:
Binance: n=34974, p50=108ms, p95=222ms, p99=332ms, max=656ms
Kraken: n=718, p50=9ms, p95=20ms, p99=37ms, max=42ms
Combined: n=35692, p50=108ms, p95=219ms, p99=332ms, max=656ms

## Connection lifecycle

Each connector runs in an outer reconnect loop wrapping a single-session
function. A session connects, subscribes, validates the subscription
acknowledgment, and reads messages until the connection ends or fails.

### Failure detection

WebSocket disconnects don't always surface as application-layer errors.
Operating systems often suspend rather than kill TCP connections during
network changes (cable pulls, Wi-Fi handoffs, mobile network switches),
which means a dead connection can sit silently with no error propagating
to the application. Relying on TCP keepalive is insufficient — its
default timer is hours.

Each connector therefore implements its own liveness timeout based on the
exchange's expected message cadence:

| Exchange | Expected gap        | Liveness timeout | Source         |
|----------|---------------------|------------------|----------------|
| Kraken   | ~1s heartbeat       | 15s              | Application    |
| Binance  | 20s server ping     | 240s             | Protocol-level |

If no message of any kind arrives within the timeout, the session is
treated as dead. The outer loop resets backoff and reconnects.

### Reconnect behavior

After a session ends (cleanly or with error), the outer loop sleeps with
exponential backoff before the next attempt:

- Initial backoff: 500ms
- Multiplier: 2× per failed attempt, with random jitter up to 25% added
- Cap: 30s
- Reset: on successful session start

The 30s cap ensures even a deeply unavailable upstream is retried at
reasonable cadence — a connector's job is to stay connected, not to give
up. The jitter prevents thundering-herd reconnect storms if multiple
clients are reconnecting after a server restart.

### Known limitations

The current liveness check detects dead **connections** but not dead
**subscriptions**. Heartbeats and pings keep arriving on a live socket
even if the server has silently dropped the subscription you requested
— from the connector's perspective, all is well, but no trades flow.

A complete fix requires a separate trade-staleness timer that resets only
on actual trade messages, with per-symbol thresholds (BTC/USD trades
several times per second; some altcoins go minutes between trades). This
is on the roadmap; v1 ships with the simpler "any-message" liveness
check, which handles the common failure modes.

### Sequence guarantees

After a reconnect, the connector receives a fresh `snapshot` from each
exchange before returning to live `update` messages. Downstream consumers
should treat snapshots as authoritative — if you maintain stateful
derivations (e.g., reconstructed order books in future versions), reset
that state on snapshot. v1 publishes both snapshot and update trades
identically, since for trades both are real market events.


### Backpressure

Connectors push canonical trades onto a bounded MPSC channel (capacity
10,000). When the channel is full — meaning the publisher cannot keep up
with the inbound rate — connectors **drop the new trade rather than
block**. Blocking would back up into the WebSocket read loop, causing
exchange-side disconnects for "slow consumer" reasons.

This is a deliberately simple policy for v1. A more correct policy is
**drop-oldest** (preserve the freshest data), which requires either a
custom channel implementation or a different crate.