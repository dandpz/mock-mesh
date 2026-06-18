# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```sh
make check                                  # everything CI runs: fmt-check + clippy + tests — use before pushing
make help                                   # list all Makefile targets (run, validate, msrv, ...)
cargo test                                  # full suite: unit + integration (tests/)
cargo test --test hot_reload                # one integration suite
cargo test token_bucket_throttles_burst     # one test by name
cargo clippy --all-targets -- -D warnings   # CI gate
cargo fmt --check                           # CI gate
cargo run -- --spec tests/fixtures/petstore.yaml --config tests/fixtures/config-chaos.yaml
cargo run -- --spec tests/fixtures/petstore.yaml --validate   # parse + print route table, exit
```

CI (`.github/workflows/ci.yml`) runs fmt → clippy → test on ubuntu + macos.

## Releases

Fully automated via release-plz; never bump the version or tag by hand.

- Conventional commits land on `main` → `release-plz.yml` opens/updates a release PR (version bump + `CHANGELOG.md`). Merging that PR publishes to crates.io and pushes tag `v{version}`.
- The tag triggers `release.yml`, which builds 6 targets (Linux gnu/musl x86_64 + arm64, macOS x86_64/arm64, Windows x86_64), runs their tests, and attaches archives + a combined `SHA256SUMS` to a single GitHub Release. `release-plz.toml` sets `git_release_enable = false` so release-plz doesn't create a duplicate; the `publish` job uploads all artifacts at once (never let per-matrix release uploads back in — partial releases).
- The crate name and binary name are both `mock-mesh`. The archive layout `mock-mesh-{version}-{target}/mock-mesh` is coupled between `release.yml` (Package step), `[package.metadata.binstall]` in Cargo.toml, and `install.sh` — change them together.
- Secrets: `RELEASE_PLZ_TOKEN` (fine-grained PAT — the default `GITHUB_TOKEN` cannot push tags that trigger other workflows). crates.io auth uses trusted publishing (OIDC via `rust-lang/crates-io-auth-action`); there is no registry token to rotate.

## Architecture

mock-mesh serves mocked APIs derived from an OpenAPI 3.0/3.1 spec, augmented by a behavior config (latency, rate limiting, error modes). No compiled routes: an axum `.fallback()` handler matches every request against a rule table at request time.

### Two kinds of state — the central design decision

1. **`MockTable`** (`rules/mod.rs`) — immutable snapshot of compiled rules, held in `ArcSwap<MockTable>` inside `AppState` (`state.rs`). Replaced wholesale on hot reload; readers get wait-free, never-torn loads. Never mutate a `MockTable` in place.
2. **`RuleRuntime`** (per rule, `Arc`-shared) — the mutable bits: admin overrides (`ArcSwapOption`), error-enabled flag (`AtomicBool`), token bucket, hit counter. **Carried over across reloads** by `RuleId` (`"GET /users/{id}"`) in `rules/compile.rs::make_rule`, so admin overrides and rate-limit token debt survive config edits. Renaming a path drops its runtime state (documented behavior).

### Data flow

```
loader.rs (JSON/YAML → serde_json::Value + typed models, validation)
  → rules/compile.rs (spec × config merge → MockTable; example/fixed bodies
    pre-serialized to Bytes here — hot path never serializes them)
  → handlers/mock.rs (the hot path):
      match (rules/matcher.rs) → error switch → rate limit → latency → body
```

Simulation order is a documented invariant: error switch first (abort/hang preempt everything), rate limit before latency (throttled requests don't pay latency cost).

Config endpoints match spec routes by method + path *shape* (`CompiledPath::shape()`): param names don't matter, `/users/{id}` ≡ `/users/{userId}`. Match precedence: method-exact > any-exact > method-templated (sorted most-literals-first, leftmost-literal tiebreak, done at compile time) > any-templated.

### The raw-Value trick

The spec is parsed **once into `serde_json::Value`** (YAML included) and kept as `spec_root: Arc<Value>`; typed models deserialize from it. `$ref` resolution (`openapi/resolve.rs`) is then just an RFC 6901 pointer lookup. Only document-local refs; external refs rejected at load. The fake-data generator (`fake/`) guards cycles with a ref-stack + depth cap 8 → degrades to `null`, never overflows.

### Custom server loop (`server.rs`)

Not `axum::serve`: a hand-rolled hyper-util accept loop exists for two features handlers can't do —
- **Abort simulation**: each request gets a `ConnKiller` (CancellationToken) in its extensions; the handler cancels it, the connection task sets `SO_LINGER(0)` (via raw fd, unix-only) before drop → client sees a real RST. In-process oneshot tests have no killer → handler falls back to empty 500.
- **Graceful shutdown** with bounded drain (`--shutdown-grace-secs`).

### Hot reload (`watch.rs`)

notify watches **parent dirs** (filtered by filename) because editors replace files by rename. Events debounce 300ms → a single-consumer mpsc reload loop shared with admin `POST /_mockmesh/reload`, so reloads never race. Reload failure keeps the old table (startup failure is fatal, reload failure is not).

### Admin API (`handlers/admin.rs`)

`/_mockmesh` prefix is hard-reserved (loader rejects colliding spec/config paths). Rules addressed by short fnv key or percent-encoded full id. PUT overrides *replaces* the whole override set. Bearer auth only when `admin_token` set, constant-time compare.

## Conventions and invariants

- `clippy::unwrap_used = deny` (Cargo.toml lints). Request-path code converts every failure into a `Response`, never panics; tests open with `#![allow(clippy::unwrap_used)]`.
- No regex anywhere (matcher is segment-based) — keep it that way (ReDoS surface).
- Hand-rolled on purpose (don't add deps for these): token bucket, uuid/base64/date-time generation, FNV hash, CLI-adjacent helpers. YAML via `serde_yaml_ng` (serde_yaml is archived), isolated behind `loader::parse_document`.
- rand 0.10: `random_range`/`random_bool` live on `rand::RngExt`, not `Rng`.
- `--seed` determinism applies to **fake bodies only** (RNG = `seed ^ fnv1a64(rule_id)`); chaos decisions (probabilities, jitter) intentionally stay random — don't "fix" that.
- Integration tests use `build_router` + `tower::ServiceExt::oneshot` (no socket) except connection-level behavior (abort/hang/shutdown/hot-reload), which uses `tests/common/mod.rs::spawn_server` on a real TCP port. Shared helpers live in `tests/common/mod.rs`.
- Edition 2024: `gen` is a reserved keyword.
