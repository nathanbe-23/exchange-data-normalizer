# 0001: Publisher - Transport 

## Status
Accepted

## Context
The normalizer component needs to push canonical trades to downstream consumers (strategies, risk systems, analytics, storage, ...) and the choice of architecture affects:
- Deployment complexity
- Multi-consumer fanout (single consumer or multiple)
- Debuggability
- Replayability 
- Coupling (does swapping transport require to touch connectors?)


## Decision
For v1, we publish to stdout since this is sufficient for a POC and we made abstraction of the publishing through a separate module. The connectors publish trades to a bounded single mpsc, a dedicated publisher task drains the channel and writes to stdout. This can be later replaced to kafka or nats in v2, which is then more a logisitcs effort.

## Rationale
- ** Zero deplyoment dependencies **  The normalizer runs as a single binary with no external infrastructure. A reviewer or operator can `cargo run` and immediately see output.
- ** Trivial debugging ** `tail -f`, `jq`, `grep`, and shell redirects all work as expected. Inspecting the canonical stream requires no special tooling.
- ** Unix composable ** Output pipes naturally into downstream tools or files, including for the latency measurement that backs this README.
- ** Swap Ready ** The Publisher abstraction means moving to NATS JetStream (the planned v2 transport) is a localized change, not an architectural one. The decision to defer it to v2 is about scope, not design.

A message broker (NATS or Kafka) was considered for v1 but rejected: it would add deployment complexity for no immediate benefit at the demo scale of one consumer. Direct in-process RPC was considered and rejected as over-engineered for a single-process system.

## Consequences
- No multi-consumer fanout - A single downstream reader consumes the stream; branching to multiple consumers requires a broker.
- No replay or buffering - If a downstream consumer disconnects or is slow, trades pile up in the OS pipe buffer and are eventually dropped (the drop-newest backpressure policy applies upstream of stdout).
- No persistence across restarts - ach run produces a fresh stream from the exchanges' "now."
- Acceptable for a v1 poc and for development / local-debugging context. Production deployment with multiple consumers or replay requirements is out of scope and captured in the roadmap as v2, together with a message broker.

