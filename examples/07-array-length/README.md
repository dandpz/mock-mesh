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
