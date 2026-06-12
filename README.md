# mock-mesh

A single-binary, high-throughput mock HTTP server driven by an OpenAPI spec —
with developer-centric network simulation: precise latency injection,
randomized rate-limiting, and intentional error-state switches you can flip
at runtime.

```
mock-mesh --spec api.yaml --config mock-mesh.yaml --port 8080
```

- **OpenAPI 3.0 / 3.1** (JSON or YAML): every path/operation becomes a mock
  route. Spec `example`/`examples` are served verbatim; everything else gets
  plausible fake data synthesized from the JSON Schema (`$ref`, `enum`,
  `oneOf`/`anyOf`/`allOf`, formats like `uuid`, `email`, `date-time`, …).
- **Network simulation** per endpoint: fixed + jittered latency, token-bucket
  rate limiting with probabilistic rejections, forced error statuses,
  black-hole hangs, and real TCP connection resets.
- **Hot reload**: edit the spec or config and the rule table swaps atomically
  — no restart, in-flight requests finish against the old snapshot.
- **Admin API** under `/_mockmesh/` to inspect routes and flip simulations at
  runtime (scriptable from your test suites).

## Install / build

```sh
cargo build --release        # target/release/mock-mesh
```

## Quick start

```sh
mock-mesh --spec tests/fixtures/petstore.yaml --config tests/fixtures/config-chaos.yaml

curl localhost:8080/pets                  # spec example, +latency
curl localhost:8080/pets/42               # schema-synthesized Pet
curl localhost:8080/_mockmesh/routes      # what's loaded
```

`mock-mesh --spec api.yaml --validate` parses everything, prints the route
table, and exits — handy in CI.

## Configuration file

The config file (JSON or YAML, `--config`) augments spec routes and can add
routes of its own. Endpoints are matched to spec routes by method + path
*shape*, so `/users/{id}` in the config matches `/users/{userId}` in the spec.

```yaml
defaults:                       # applied where an endpoint leaves a field unset
  latency: { fixed_ms: 20, jitter_ms: 30 }

endpoints:
  # Add chaos to a spec route
  - path: /users/{id}
    method: GET                 # or "any" (default)
    behavior:
      latency: { fixed_ms: 150, jitter_ms: 100 }    # 150ms + uniform 0–100ms
      rate_limit:
        rps: 50                 # token refill rate
        burst: 10               # bucket capacity
        reject_probability: 0.05  # also reject 5% of passing requests (flaky upstream)
      error_mode:
        kind: status
        code: 500
        probability: 0.10       # omit = always
        body: { error: "internal" }

  # Replace the spec-derived response entirely
  - path: /users/{id}
    method: DELETE
    response: { status: 204 }

  # Config-only route (not in the spec)
  - path: /internal/feature-flags
    method: GET
    response:
      status: 200
      headers: { x-mock: "true" }
      body: { dark_mode: true }

  # Accept the request and never answer (bounded at max_secs, default 120)
  - path: /payments
    method: POST
    behavior:
      error_mode: { kind: hang, max_secs: 30 }

  # Reset the TCP connection mid-request (client sees "connection reset by peer")
  - path: /webhooks/flaky
    method: POST
    behavior:
      error_mode: { kind: abort }
```

Unknown config fields are rejected (typos fail loudly). The simulation order
per request is: **error switch → rate limit → latency → body** — aborts and
hangs preempt everything, and throttled requests don't pay the latency cost.

### Rate limiting semantics

Two composable mechanisms:

1. `rps` + `burst` — a deterministic token bucket. When empty, requests get
   `429` (configurable via `response_status`) with a `Retry-After` header.
2. `reject_probability` — each request that *passed* the bucket is rejected
   with this probability. Use it to test client retry logic against
   throttling that can't be predicted.

### Response selection from the spec

For each operation mock-mesh picks the lowest 2xx response, else `default`
(served as 200), else the lowest other status; `2XX`-style ranges map to the
range floor. Content negotiation prefers `application/json`, then any
`*+json`, then the first media type.

## Hot reload

Both files are watched (parent-directory watch, so editor rename-saves and
Kubernetes configmap updates are caught). Changes are debounced (300ms),
re-parsed and validated; **a broken file never takes the server down** — the
previous rules stay live and the error is logged. Disable with `--no-watch`.

Runtime state survives reloads: admin overrides, rate-limiter token debt and
hit counters carry over per route (keyed by `METHOD /path`; renaming a path
drops its overrides).

## Admin API

All under `/_mockmesh`. Routes are addressed by the short `key` from the
listing, or by the URL-encoded full id (`GET%20%2Fpets`).

| Method | Path | Effect |
|---|---|---|
| GET | `/_mockmesh/health` | status, rule generation, uptime |
| GET | `/_mockmesh/routes` | all rules: id, key, source, behavior, overrides, hits |
| GET | `/_mockmesh/routes/{id}` | one rule |
| PUT | `/_mockmesh/routes/{id}/overrides` | replace runtime overrides (see below) |
| DELETE | `/_mockmesh/routes/{id}/overrides` | clear overrides, re-enable file config |
| POST | `/_mockmesh/routes/{id}/rate-limit/reset` | refill the token bucket |
| POST | `/_mockmesh/reload` | force re-read of spec + config (422 + message on parse error) |
| GET | `/_mockmesh/config` | the loaded mock config, for debugging |

Flip an endpoint into failure mode from a test:

```sh
KEY=$(curl -s localhost:8080/_mockmesh/routes | jq -r '.routes[] | select(.id=="GET /pets") | .key')
curl -X PUT localhost:8080/_mockmesh/routes/$KEY/overrides \
  -H 'content-type: application/json' \
  -d '{"error_mode": {"kind": "status", "code": 503}}'
# ... run the test ...
curl -X DELETE localhost:8080/_mockmesh/routes/$KEY/overrides
```

`PUT` replaces the whole override set; omitted fields are cleared.
`{"enabled": false}` disables error simulation on the route entirely
(including file-configured error modes) without touching files.

## Determinism

`--seed <u64>` makes schema-synthesized bodies **byte-identical** per
endpoint across requests *and* restarts (the RNG is derived from the seed and
the route id) — ideal for snapshot tests. Chaos decisions (probabilistic
errors, jitter, random rejections) intentionally stay random even when
seeded; a "10% errors" endpoint that always or never failed would be useless.

## CLI reference

| Flag | Default | |
|---|---|---|
| `--spec <PATH>` | required | OpenAPI 3.0/3.1 file (JSON/YAML) |
| `--config <PATH>` | – | behavior config |
| `--host` | `127.0.0.1` | bind address |
| `--port`, `-p` | `8080` | bind port (0 = pick free) |
| `--admin-token` | – | require `Authorization: Bearer` on `/_mockmesh` (env `MOCKMESH_ADMIN_TOKEN`) |
| `--no-admin` | – | remove the admin API entirely |
| `--no-watch` | – | disable hot reload |
| `--seed <u64>` | – | deterministic fake data |
| `--validate` | – | parse, print route table, exit |
| `--max-body-bytes` | `1048576` | request body cap |
| `--shutdown-grace-secs` | `10` | drain window on SIGINT/SIGTERM |
| `--log` | `info` | log filter (`RUST_LOG` also honored) |

## Security notes

mock-mesh is a development tool. Defaults are chosen accordingly:

- Binds **loopback only** by default. If you bind `0.0.0.0`, set
  `--admin-token` (or `--no-admin`) — otherwise anyone on the network can
  flip your simulations; the server logs a loud warning in that case.
- Request bodies are capped (`--max-body-bytes`), spec/config files are
  capped at 20 MiB, path matching is segment-based (no regex, no ReDoS), and
  only document-local `$ref`s are resolved (no file or network fetches).
- The request path is panic-free by construction
  (`clippy::unwrap_used = deny`).

## Limitations (v0.1)

- `additionalProperties`-only schemas generate `{}`.
- An `abort` on an HTTP/2 connection kills all multiplexed streams on it.
- External `$ref`s (other files/URLs) are rejected at load time.

## License

MIT © Daniele Dapuzzo — <https://github.com/dandpz/mock-mesh>
