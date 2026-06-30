# 06 · Attachments — serve files and binary payloads

Mock endpoints that return downloadable files (PDF, CSV) or inline binary
(images) instead of JSON — useful for testing report/export/download flows.

```sh
mock-mesh --spec ../orders-api.yaml --config mock-mesh.yaml
```

> Run from this directory: `body_file` paths are relative to the working
> directory, and this config uses bare filenames (`invoice.pdf`, `orders.csv`).

```sh
# PDF invoice — note Content-Type and the Content-Disposition attachment header:
curl -i localhost:8080/orders/ord_001/invoice

# Save it to disk and open it:
curl -s localhost:8080/orders/ord_001/invoice -o invoice.pdf && open invoice.pdf

# CSV export — text/csv, served inline:
curl -i localhost:8080/orders/export.csv

# Inline 1x1 PNG from base64, no file needed:
curl -s localhost:8080/brand/logo.png | file -
```

## The four response fields

| field | effect |
|---|---|
| `body_file` | serve a file's bytes (path relative to the working dir, read into memory at load) |
| `body_base64` | inline binary body, base64-encoded in the config |
| `content_type` | explicit Content-Type; overrides the extension guess |
| `filename` | adds `Content-Disposition: attachment; filename="..."` |

`body`, `body_text`, `body_base64`, and `body_file` are mutually exclusive —
at most one per response. A missing `body_file` or malformed `body_base64`
fails loudly at load.

## Content-Type

For `body_file`, Content-Type is guessed from the extension (pdf, png, jpeg,
gif, svg, webp, csv, txt, html, xml, zip, gz, wasm, json), falling back to
`application/octet-stream`. `body_base64` defaults to
`application/octet-stream`. Set `content_type` to override either.

## Notes

- Files are loaded into memory once at startup (and on hot reload). Suited to
  fixture-sized payloads, not multi-GB streams.
- Attachment bodies compose with every other behavior — add `latency` to
  simulate a slow download, or `error_mode` to fail it. Simulation order is
  unchanged: **error switch → rate limit → latency → body**.
