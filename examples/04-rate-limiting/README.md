# 04 · Rate limiting — make the mock push back

Simulate throttling so you can verify your client honors `429`/`Retry-After`
and backs off correctly.

```sh
mock-mesh --spec ../orders-api.yaml --config mock-mesh.yaml
```

```sh
# Burst of 5 succeeds, then 429s appear (~2/sec refill). Fire 10 fast:
for i in $(seq 1 10); do curl -s -o /dev/null -w '%{http_code} ' localhost:8080/orders; done; echo
# e.g. 200 200 200 200 200 429 429 429 429 429

# See the Retry-After header on a throttled response:
curl -s -D - -o /dev/null localhost:8080/orders | grep -i retry-after

# Unpredictable: ~25% of these 429 even though the bucket is huge:
for i in $(seq 1 12); do curl -s -o /dev/null -w '%{http_code} ' localhost:8080/orders/x; done; echo

# Custom rejection status (503 instead of 429):
curl -s -o /dev/null -w '%{http_code}\n' -X POST localhost:8080/payments
curl -s -o /dev/null -w '%{http_code}\n' -X POST localhost:8080/payments  # 2nd within 1s → 503
```

## Fields

| Field | Meaning | Default |
|---|---|---|
| `rps` | Token refill rate (tokens/sec); must be > 0 | required |
| `burst` | Bucket capacity (max immediate burst) | `1` |
| `reject_probability` | Chance (0–1) of rejecting a request that *passed* the bucket | `0` |
| `response_status` | Status returned on rejection | `429` |

The token bucket is **deterministic**; `reject_probability` is **random**
(stays random even under `--seed`, by design — a limiter that always or never
fired would be useless for testing). A throttled request is rejected before
any latency is applied.

To reset a bucket mid-test, see the admin API in
[05-admin-overrides](../05-admin-overrides).
