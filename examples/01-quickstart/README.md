# 01 · Quickstart — serve a spec, no config

The simplest possible use: point mock-mesh at an OpenAPI spec and it serves a
mock for every operation. No config file, no simulation.

```sh
mock-mesh --spec ../orders-api.yaml
```

Then, in another terminal:

```sh
# Served verbatim from the spec's `example`:
curl localhost:8080/orders

# No example on this operation → fake data synthesized from the schema
# (uuid, email, enum, date-time, array of line items):
curl localhost:8080/orders/any-id
curl -X POST localhost:8080/orders

# The `default` response is served as 200:
curl localhost:8080/health

# What got loaded:
curl localhost:8080/_mockmesh/routes
```

## Things to try

**Validate without serving** — parse the spec, print the route table, exit
(handy in CI):

```sh
mock-mesh --spec ../orders-api.yaml --validate
```

**Deterministic fake data** — `--seed` makes schema-synthesized bodies
byte-identical across requests *and* restarts, so they're safe for snapshot
tests:

```sh
mock-mesh --spec ../orders-api.yaml --seed 42
# every `curl localhost:8080/orders/x` now returns the same Order
```

**Pick a free port** — `--port 0` binds an OS-assigned port (printed at
startup); `-p 9000` sets it explicitly.

Next: [02-latency](../02-latency) adds a behavior config.
