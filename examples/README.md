# Examples

Hands-on, runnable examples — each one teaches a single feature. They all
share one spec, [`orders-api.yaml`](orders-api.yaml), so you can see how the
same API behaves as you layer different behavior configs onto it.

Work through them in order:

| # | Example | Teaches |
|---|---|---|
| 01 | [quickstart](01-quickstart) | Serve a spec with zero config; spec examples vs. schema-synthesized fake data; `--validate`, `--seed` |
| 02 | [latency](02-latency) | `defaults` + per-route latency with jitter |
| 03 | [errors](03-errors) | `error_mode`: forced/probabilistic status, `hang`, `abort` |
| 04 | [rate-limiting](04-rate-limiting) | Token bucket (`rps`/`burst`), `reject_probability`, custom status |
| 05 | [admin-overrides](05-admin-overrides) | Flip behavior at runtime via the `/_mockmesh` admin API |
| 06 | [attachments](06-attachments) | Serve files & binary bodies: `body_file`, `body_base64`, `content_type`, `filename` |

## Prerequisites

Install mock-mesh (see the [root README](../README.md)), or run from a
checkout — replace `mock-mesh` in the commands with `cargo run --`:

```sh
# installed:
mock-mesh --spec orders-api.yaml

# from a checkout (run from the repo root, adjust the path):
cargo run -- --spec examples/orders-api.yaml
```

The examples assume the default bind `localhost:8080` and use `curl` plus
[`jq`](https://jqlang.github.io/jq/) for the admin walkthrough. Each example's
`README.md` has the exact commands; `cd` into the example directory first so
the relative `../orders-api.yaml` path resolves.

## How a config maps to the spec

A behavior config (`--config`) doesn't redefine routes — it *matches* spec
operations by HTTP method + path **shape**, so `/orders/{orderId}` in the spec
matches `/orders/{id}` in the config. A config can also add routes the spec
doesn't have. Unknown fields are rejected, so typos fail loudly at load.
