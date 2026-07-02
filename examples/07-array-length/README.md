# 07 · Array length — size generated list responses

`GET /products` has no spec example, so mock-mesh synthesizes its body from
the schema. The schema says `minItems: 2, maxItems: 3` — fine for a smoke
test, useless for exercising pagination, scrolling or rendering performance
in a client. `array_length` overrides it:

```sh
mock-mesh --spec ../orders-api.yaml --config mock-mesh.yaml
```

```sh
# 25 products (config: array_length: 25); without the config it'd be 2–3
curl -s localhost:8080/products | jq length

# spec examples always win: still the 2-order example, verbatim
curl -s localhost:8080/orders | jq length
```

## How it works

- `array_length` applies to the **root array** of schema-generated responses
  only. Arrays nested inside objects keep their schema `minItems`/`maxItems`,
  so a big length can't explode recursively.
- The config **overrides** the schema's `minItems`/`maxItems` — explicit
  intent wins.
- Fixed value (`array_length: 25`) or range (`{ min: 10, max: 50 }`); values
  above 10 000 are rejected at load time.
- No effect on spec `example`s or fixed `response:` bodies — those are served
  verbatim.
- Works under `defaults:` too — e.g. keep every generated list short across
  the whole API while developing.
- Composes with `--seed`: a fixed length keeps bodies byte-identical; a range
  picks the same length every run.

## Client-driven sizing and pagination

The spec declares `size` and `page` query parameters on `GET /products`, so
clients can control the list per request — no config needed (it works even
without `mock-mesh.yaml`):

```sh
# exactly 100 products — a spec-declared size param beats array_length
curl -s 'localhost:8080/products?size=100' | jq length

# unparsable values are ignored (config's 25 applies), huge ones clamp to
# 10000 — client input never causes a 400
curl -s 'localhost:8080/products?size=nope' | jq length
```

Restart with `--seed` and pages become stable *and* distinct — enough to test
a client's pagination loop:

```sh
mock-mesh --spec ../orders-api.yaml --config mock-mesh.yaml --seed 42
```

```sh
curl -s 'localhost:8080/products?size=5&page=1'   # byte-identical every time
curl -s 'localhost:8080/products?size=5&page=2'   # different, also stable
```

Recognized size params (first declared wins, in this priority): `size`,
`limit`, `per_page`, `page_size`, `pageSize`; pagination: `page`. Only
parameters the spec **declares** are honored — mock-mesh never sniffs the raw
query string.
