# exchange-data-normalizer
A low-latency market data normalizer that ingests a websocket feed from multiple cryptocurrency exchanges and publishes a unified, sequence-consistent stream of trades to downstream consumers. Built in Rust with a focus on correctness under failure - reconnects, liveness detection, backpressure - so that strategies, risk systems and analytics can consume one canonical schema instead of N exchange-specific ones.

## Why this exists
Every exchange websocket API has its own quirks: different message shapes, symbol formats, timestamp encodings, heartbeat conventions. 
A strategy or risk system that consumes raw exchange feeds ends up reimplementing per-exchange handling badly and inconsistently. This normalizer pushes that complexity into one process that handles it carefully once and exposes a unified canonical stream.

## Architecture 

```text
                          ┌──────────────┐
   wss://stream.binance ──┤ Binance      │──┐
                          │ Connector    │  │
                          └──────────────┘  │
                                            ▼
                          ┌──────────────┐  ┌─────────────┐  ┌──────────────┐
                          │              │  │ mpsc        │  │              │
                          │              │  │ channel     │  │  Publisher   │
                          │              │──▶ (10k cap)   ├─▶│  (stdout     │
   wss://ws.kraken.com ───┤ Kraken       │  │             │  │   JSON Lines)│
                          │ Connector    │  └─────────────┘  └──────────────┘
                          └──────────────┘
                                                                    
                                  metrics::counter, metrics::histogram   
                                                │
                                                ▼
                          http://localhost:9090/metrics (Prometheus)
```

The system has three main components: connectors, a shared channel, and a publisher. 
The connector manages the websocket connection with the exchange, parses incoming data to the canonical format and posts it on the channel which is shared by both connectors.
The channel is mpsc (multiple producers, single consumer) so data can be pushed concurrently from multiple connectors and since we only publish to stdout, a single consumer is sufficient. 
Lastly, we also have performance monitoring setup through the metrics package. This runs a small server that host a metrics endpoint on port 9090. It measures latency, processed trades and disconnects. 

## Build and run

```bash
# Build
cargo build --release

# Run with default configuration (connects to Binance and Kraken)
cargo run --release

# Run with verbose logging
RUST_LOG=debug cargo run --release

# Trade output (stdout) and logs (stderr) are separated; you can pipe
# trades into a file while keeping logs visible:
cargo run --release > trades.jsonl 2> logs.txt

# Inspect live metrics in another terminal
curl http://127.0.0.1:9090/metrics
```

The binary produces JSON-Lines trade output on stdout and structured tracing logs on stderr. Prometheus metrics are exposed on http://127.0.0.1:9090/metrics.
Requires a recent Rust version (tested with 1.95) and a working internet connection to the exchange WebSocket endpoints.

## Testing 

```
cargo test
```

The test suite covers two layers:

- Unit tests on the per-exchange parsing functions (parse_message for Kraken, BinanceTrade deserialization for Binance) and on the wire-format-to-canonical conversions.
- Integration test for the Binance connector against an in-process WebSocket server. This exercises the full pipe — WebSocket read, parse, canonical conversion, channel send — without depending on a live exchange.


## Canonical Schema
The main canonical type is the Trade struct:

```rust
pub struct Trade {
    pub exchange: Exchange,
    pub symbol: String,
    pub price: f64,
    pub quantity: f64,
    pub side: Side,
    pub exchange_ts_ms: u64,
    pub recv_ts_ms: u64,
}
```
Three fields are interesting here: side and two timestamps. A trade is defined from the pov of the aggressor that takes liquidity away, so we generalize the side that it was on (buy or sell). We have two timestamps so we can track the time it takes for a trade to arrive. Note that recv_ts_ms - exchange_ts_ms can occasionally be negative if the exchange's clock runs ahead of ours; v1 does not perform clock-skew correction (see roadmap for the planned binance_clock_skew_ms metric).

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

## Observability
The normalizer exposes Prometheus metrics on `:9090/metrics`:

- `trades_received_total{exchange}` — counter, incremented per canonical trade received
- `trades_dropped_total{exchange}` — counter, incremented when the publisher channel is full
- `reconnect_count_total{exchange}` — counter, incremented per session failure
- `exchange_connected{exchange}` — gauge, 1 when the connector has an active subscription, 0 otherwise
- `e2e_latency_ms{exchange}` — histogram, distribution of `recv_ts_ms - exchange_ts_ms`

## Latency
Measured over a 10-minute capture from a residential network in Paris. Numbers reflect WebSocket transit plus parse plus canonical conversion.

| Exchange | n trades | p50  | p95  | p99  | max  |
|----------|----------|------|------|------|------|
| Binance  | 34,974   | 108ms | 222ms | 332ms | 656ms |
| Kraken   | 718      | 9ms   | 20ms  | 37ms  | 42ms  |
| Combined | 35,692   | 108ms | 219ms | 332ms | 656ms |

Binance latency is consistently higher than Kraken's. This reflects the geographic location of Binance's WebSocket endpoints (Asia) versus Kraken's (US/EU); a production deployment closer to either exchange's data center would see substantially lower p50.

## Non goals v1 
The following are explicitly out of scope for v1. Each represents a deliberate choice rather than an oversight, and each is captured in the Roadmap below.

- Order book reconstruction. v1 publishes trade events only. Maintaining stateful L2 order books per (exchange, symbol), including snapshot-versus-update semantics and consistency guarantees across reconnects, is a meaningful additional concern best designed independently.
- Schema versioning. The canonical Trade schema is published as untagged JSON. Production-grade schema evolution (versioned protobuf, backwards-compatible field additions, consumer schema discovery) is deferred.
- Message broker publisher. v1 publishes to stdout (see ADR 0001). NATS, Kafka, or Redis Streams are roadmap items, not v1 features.
- Third+ exchange. v1 ships Binance and Kraken. The current code keeps two parallel connector modules rather than abstracting prematurely; a Connector trait will be introduced when adding a third exchange justifies the abstraction.
- Drop-oldest backpressure. v1 drops the newest trade when the publisher channel is full, since tokio::sync::mpsc does not natively support drop-oldest. Drop-oldest is the more correct policy for market data and is a roadmap item.
- Per-symbol staleness detection. v1's liveness timeout detects dead connections, not dead subscriptions. A live socket with arriving heartbeats but no trades is treated as healthy. Trade-rate-based staleness with per-symbol thresholds is a roadmap item.
- Persistence and replay. Trades are published as they arrive; there is no on-disk buffer, no at-least-once delivery, no replay for late-joining consumers. These properties come naturally with a broker (see roadmap).
- Authentication or private streams. v1 only consumes public market data. Account, balance, and order streams require per-exchange authentication flows and are out of scope.

## Roadmap

### Reliability and correctness

- Per-symbol trade-staleness detection with configurable thresholds (distinct from connection-liveness timeout). Address subscription-was-silently-dropped failure mode.
- Drop-oldest backpressure semantics. Likely requires swapping tokio::sync::mpsc for a different channel implementation or wrapping with a custom drop policy.
- Surface parse errors via Result<Trade, ConvertError> rather than unwrap_or(0.0) on price/quantity. Drop-with-warn is the current behavior; structured error handling is the upgrade.
- Fault-injection integration test: extend the in-process mock to drop the connection mid-stream and assert the connector reconnects and resumes producing trades.

### Observability

- Clock-skew gauge using Binance's ping payload (server-provided timestamp) to measure exchange↔client clock drift, exposed as binance_clock_skew_ms.
- `/health` endpoint returning per-exchange connection state as JSON.
- Per-symbol metrics, once symbol set is configurable.

### Architecture

- Connector trait abstraction, introduced when adding a third exchange.
- Symbol registry: a single source of truth for canonical symbols and per-exchange wire-format mappings, replacing the per-connector normalize_symbol functions.
- Configuration via TOML or environment variables (currently URLs and symbols are constants).

### Throughput and scope

- Order book reconstruction (L2) for at least one exchange, including snapshot-then-update semantics, sequence consistency under reconnects, and a top-of-book snapshot endpoint.
- Kafka / NATS JetStream publisher (see ADR 0001). Adds multi-consumer fanout, replay, and at-least-once delivery.
- Schema versioning: protobuf with explicit version fields, or versioned JSON envelopes.
- Additional exchanges: Coinbase, Bybit, OKX. Each new connector validates the canonical schema and surfaces design pressure on the Connector trait.

## References

### Architecture decisions
- [ADR 0001: Publisher transport](docs/adr/0001-publisher-transport.md)

### Exchange documentation
- Binance Spot WebSocket Streams — https://developers.binance.com/docs/binance-spot-api-docs/web-socket-streams
- Kraken WebSocket API v2 (Trade channel) — https://docs.kraken.com/api/docs/websocket-v2/trade