---
name: mock-mesh
description: >-
  Drive mock-mesh — a single-binary mock HTTP server generated from an OpenAPI
  spec, with per-endpoint network-fault simulation (latency, rate limiting,
  forced errors, hangs, TCP resets). Use this skill to stand up a mock backend
  from a spec, hit its endpoints, inject faults at runtime via the /_mockmesh
  admin API, and tear it down — so you can exercise a client against a flaky or
  failing dependency without touching real services. Triggers: "mock this API",
  "stand up a fake backend", "simulate a 500 / timeout / rate limit", "test my
  client against a flaky upstream".
---

# Driving mock-mesh

mock-mesh turns an OpenAPI spec into a running mock server and lets you flip
each endpoint into failure modes at runtime. The binary is `mock-mesh`
(`cargo run --` from a checkout, or installed on PATH).

A starter spec + config ship beside this file:
`.claude/skills/mock-mesh/examples/openapi.yaml` and `…/mock-mesh.yaml`. Use
them to try things immediately, or point `--spec`/`--config` at the real files.

## 1. Inspect before running

```sh
mock-mesh --spec <spec.yaml> --validate
```
Parses spec + optional `--config`, prints the route table (method, path,
response source, simulations), and exits. Use this first to confirm the spec
loads and to see each route's `key`.

## 2. Start the server (background)

```sh
# Auto-pick a free port, log to a file, run in the background:
mock-mesh --spec <spec.yaml> --config <config.yaml> --port 0 >/tmp/mockmesh.log 2>&1 &
```
- Binds **127.0.0.1** by default (loopback only).
- Read the chosen address from the startup log line `mock-mesh listening
  addr=127.0.0.1:PORT …` in `/tmp/mockmesh.log`. With a fixed `--port 8080`
  you already know it.
- `--seed <N>` makes schema-synthesized bodies byte-identical across runs
  (good for snapshot assertions). Chaos (probabilities, jitter) stays random.
- Health check: `curl -s localhost:PORT/_mockmesh/health`.

## 3. Hit endpoints

```sh
curl -s localhost:PORT/widgets               # a mocked response
curl -s localhost:PORT/_mockmesh/routes      # every rule: id, key, behavior, hits
```
Responses come from the spec's `example` (verbatim) or are synthesized from
the JSON Schema (uuid/email/date-time/enum/arrays/$ref).

## 4. Inject faults at runtime (admin API)

All under `/_mockmesh`. If the server was started with `--admin-token T`, add
`-H "authorization: Bearer T"` to every admin call. Routes are addressed by the
short `key` from the listing (or the URL-encoded full id like `GET%20%2Fwidgets`).

```sh
BASE=localhost:PORT

# Grab a route key by its id ("METHOD /path"):
KEY=$(curl -s $BASE/_mockmesh/routes | jq -r '.routes[]|select(.id=="GET /widgets").key')

# Force responses into an error. PUT REPLACES the whole override set;
# omitted fields are cleared.
curl -s -X PUT $BASE/_mockmesh/routes/$KEY/overrides \
  -H 'content-type: application/json' \
  -d '{"error_mode":{"kind":"status","code":503,"body":{"error":"down"}}}'

# ... exercise your client, assert it handles the failure ...

# Restore normal behavior:
curl -s -X DELETE $BASE/_mockmesh/routes/$KEY/overrides
```

### Override payload (`PUT …/overrides`)
| Field | Meaning |
|---|---|
| `error_mode` | `{"kind":"status","code":503,"body":{…},"probability":0.3}` — `body`/`probability` optional (omit `probability` = always). Or `{"kind":"hang","max_secs":5}` (hold open, never answer). Or `{"kind":"abort"}` (TCP reset). |
| `latency` | `{"fixed_ms":200,"jitter_ms":100}` |
| `enabled` | `false` disables error simulation on the route entirely (incl. file-configured), without editing files |

### Other admin endpoints
```sh
curl -s        $BASE/_mockmesh/routes/$KEY                       # one rule
curl -s -X POST $BASE/_mockmesh/routes/$KEY/rate-limit/reset      # refill token bucket
curl -s -X POST $BASE/_mockmesh/reload                           # re-read spec+config from disk
curl -s        $BASE/_mockmesh/config                            # loaded config (debug)
```

## 5. Config file (static simulation)

Instead of (or alongside) runtime overrides, declare behavior in a `--config`
file. Matched to spec routes by method + path **shape** (`/widgets/{id}` ≡
`/widgets/{widgetId}`); can also add config-only routes. Unknown fields are
rejected (typos fail loudly).

```yaml
defaults:                              # applied where a route leaves a field unset
  latency: { fixed_ms: 20, jitter_ms: 30 }
endpoints:
  - path: /widgets
    method: GET                        # or "any" (default)
    behavior:
      latency: { fixed_ms: 150, jitter_ms: 100 }
      rate_limit: { rps: 50, burst: 10, reject_probability: 0.05, response_status: 429 }
      error_mode: { kind: status, code: 500, probability: 0.1, body: { error: "boom" } }
  - path: /widgets/{id}
    method: DELETE
    response: { status: 204 }          # replace the spec-derived response entirely
```

Schema cheat-sheet:
- `latency`: `fixed_ms` (base delay) + `jitter_ms` (uniform 0..jitter added on top).
- `rate_limit`: `rps` (refill/sec, > 0), `burst` (capacity, ≥1, default 1),
  `reject_probability` (0–1, also drop this fraction of passing requests),
  `response_status` (rejection status, default 429).
- `error_mode.kind`: `status` (`code`, optional `body`, optional `probability`),
  `hang` (`max_secs`, default 120), `abort`.

Per-request order is **error switch → rate limit → latency → body**: aborts/hangs
preempt everything; throttled requests don't pay latency.

## 6. Tear down

Stop the background process (`kill %1`, or SIGINT/SIGTERM). In-flight requests
drain within `--shutdown-grace-secs` (default 10).

## Notes
- Loopback-only by default; if you must bind `--host 0.0.0.0`, also pass
  `--admin-token` (or `--no-admin`) — otherwise anyone on the network can flip
  simulations.
- `--no-watch` disables hot reload; otherwise editing the spec/config on disk
  swaps the rule table atomically (admin overrides survive the reload).
