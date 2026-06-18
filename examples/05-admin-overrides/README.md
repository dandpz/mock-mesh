# 05 · Admin overrides — flip behavior at runtime, no file edits

The admin API (under `/_mockmesh`) lets a test script change a route's
simulation on the fly, run assertions, then restore it — without touching the
config file. No config needed here; start from the bare spec:

```sh
mock-mesh --spec ../orders-api.yaml
```

## Walkthrough

```sh
BASE=localhost:8080

# 1. List routes and grab the short key for "GET /orders".
#    (You can also address a route by its URL-encoded id, e.g. GET%20%2Forders.)
curl -s $BASE/_mockmesh/routes | jq -r '.routes[] | "\(.key)\t\(.id)"'
KEY=$(curl -s $BASE/_mockmesh/routes | jq -r '.routes[] | select(.id=="GET /orders") | .key')

# 2. Normal response right now:
curl -s -o /dev/null -w 'before: %{http_code}\n' $BASE/orders

# 3. Force it into 503 failure:
curl -s -X PUT $BASE/_mockmesh/routes/$KEY/overrides \
  -H 'content-type: application/json' \
  -d '{"error_mode": {"kind": "status", "code": 503}}'
curl -s -o /dev/null -w 'during: %{http_code}\n' $BASE/orders   # 503

# 4. ... run your test that asserts the client handles 503 ...

# 5. Clear the override — back to the normal mock:
curl -s -X DELETE $BASE/_mockmesh/routes/$KEY/overrides
curl -s -o /dev/null -w 'after: %{http_code}\n' $BASE/orders    # 200
```

## Override payload

`PUT .../overrides` **replaces** the whole override set (omitted fields are
cleared):

| Field | Effect |
|---|---|
| `error_mode` | Same shape as in the config (`status` / `hang` / `abort`) |
| `latency` | `{ fixed_ms, jitter_ms }` |
| `enabled` | `false` disables error simulation on the route entirely — including any file-configured error mode — without editing files |

```sh
# Add 1s latency and disable any configured error mode in one shot:
curl -s -X PUT $BASE/_mockmesh/routes/$KEY/overrides \
  -H 'content-type: application/json' \
  -d '{"latency": {"fixed_ms": 1000}, "enabled": false}'
```

## Other admin endpoints

```sh
curl -s $BASE/_mockmesh/health                              # status, generation, uptime
curl -s -X POST $BASE/_mockmesh/routes/$KEY/rate-limit/reset  # refill the token bucket
curl -s -X POST $BASE/_mockmesh/reload                      # re-read spec + config from disk
curl -s $BASE/_mockmesh/config                              # the loaded config (debugging)
```

Overrides and rate-limiter state survive hot reloads (keyed by `METHOD /path`).

## Locking down the admin API

It's open by default only on loopback. If you bind a non-loopback address,
require a bearer token (or remove the admin API entirely):

```sh
mock-mesh --spec ../orders-api.yaml --host 0.0.0.0 --admin-token s3cret
curl -s -H 'authorization: Bearer s3cret' localhost:8080/_mockmesh/routes
# or:  mock-mesh --spec ../orders-api.yaml --no-admin
```
