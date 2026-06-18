# 03 · Errors — exercise your client's failure handling

Force endpoints into failure so you can test how your client reacts: retries,
timeouts, circuit breakers, error surfacing.

```sh
mock-mesh --spec ../orders-api.yaml --config mock-mesh.yaml
```

```sh
# Always 500 with a custom body:
curl -i -X POST localhost:8080/orders

# 503 ~30% of the time, normal Order the rest — run it a few times:
for i in $(seq 1 10); do curl -s -o /dev/null -w '%{http_code} ' localhost:8080/orders/x; done; echo

# Hangs with no response, then the connection times out at max_secs=5.
# (Ctrl-C to stop waiting.)
curl -m 8 -X POST localhost:8080/payments

# Connection reset — curl reports "Recv failure" / "reset by peer", no status:
curl -i localhost:8080/health
```

## The three error modes

| `kind` | Effect | Key fields |
|---|---|---|
| `status` | Fixed HTTP error response | `code`, optional `body`, optional `probability` (omit = always) |
| `hang` | Accept then never answer | `max_secs` (default 120) |
| `abort` | TCP RST mid-request | — |

`probability` (0–1) applies to `status` only — it's the fraction of requests
that fail; the rest fall through to the normal mock response.

## Simulation order

Per request: **error switch → rate limit → latency → body**. Aborts and hangs
preempt everything else, and an errored request never pays the latency cost.

Flip these at runtime without editing the file — see
[05-admin-overrides](../05-admin-overrides).
