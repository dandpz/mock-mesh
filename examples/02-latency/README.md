# 02 · Latency — make a fast mock feel like a real network

A behavior config (`--config`) augments the spec. Here every route gets a
small baseline delay via `defaults`, and two routes override it with heavier
latency.

```sh
mock-mesh --spec ../orders-api.yaml --config mock-mesh.yaml
```

Watch the timings:

```sh
# ~25–50ms (the default latency)
curl -w '\n%{time_total}s\n' -o /dev/null -s localhost:8080/health

# ~200–350ms (per-route override: fixed 200ms + 0–150ms jitter)
curl -w '\n%{time_total}s\n' -o /dev/null -s localhost:8080/orders

# ~500ms (fixed, no jitter)
curl -w '\n%{time_total}s\n' -o /dev/null -s -X POST localhost:8080/payments
```

## How it works

- `latency.fixed_ms` — delay added to every matching response.
- `latency.jitter_ms` — extra **uniform random** delay in `[0, jitter_ms]`,
  added on top of `fixed_ms`. Use it to make latency realistic, not constant.
- `defaults` fills any field a route leaves unset; a route's own `latency`
  replaces the default entirely (no merging of sub-fields).

Latency is paid *after* rate-limit and error checks, so a throttled or
errored request returns immediately — see [03-errors](../03-errors) and
[04-rate-limiting](../04-rate-limiting).
