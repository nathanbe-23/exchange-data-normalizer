import json
from collections import defaultdict

latencies_by_exchange = defaultdict(list)
all_latencies = []

with open('trades.jsonl') as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        try:
            t = json.loads(line)
        except json.JSONDecodeError:
            continue
        latency = t['recv_ts_ms'] - t['exchange_ts_ms']
        latencies_by_exchange[t['exchange']].append(latency)
        all_latencies.append(latency)

def percentiles(data, name):
    if not data:
        print(f"{name}: no data")
        return
    data.sort()
    n = len(data)
    print(f"{name}: n={n}, "
          f"p50={data[n//2]}ms, "
          f"p95={data[int(n*0.95)]}ms, "
          f"p99={data[int(n*0.99)]}ms, "
          f"max={data[-1]}ms")

for exchange, latencies in latencies_by_exchange.items():
    percentiles(latencies, exchange)
percentiles(all_latencies, "Combined")
