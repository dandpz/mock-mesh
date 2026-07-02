//! Compiled mock rules. A `MockTable` is an immutable snapshot swapped
//! wholesale on reload (via `ArcSwap`); the mutable bits (admin overrides,
//! rate-limit buckets, hit counters) live in `RuleRuntime` and are carried
//! over across reloads by rule id.

pub mod compile;
pub mod matcher;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};

use arc_swap::ArcSwapOption;
use bytes::Bytes;
use http::{HeaderName, HeaderValue, Method, StatusCode};
use serde_json::Value;

use crate::config::model::{Behavior, ErrorModeSpec, LatencySpec, MockConfig};
use crate::openapi::model::Schema;
use crate::simulate::rate_limit::TokenBucket;

/// Stable identity of a rule across reloads, e.g. `"GET /users/{id}"`.
pub type RuleId = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    Literal(String),
    Param(String),
}

#[derive(Debug, Clone)]
pub struct CompiledPath {
    pub raw: String,
    pub segs: Vec<Seg>,
    pub literal_count: usize,
}

impl CompiledPath {
    pub fn parse(raw: &str) -> CompiledPath {
        let segs: Vec<Seg> = split_path(raw)
            .map(|s| {
                if s.starts_with('{') && s.ends_with('}') && s.len() > 2 {
                    Seg::Param(s[1..s.len() - 1].to_string())
                } else {
                    Seg::Literal(s.to_string())
                }
            })
            .collect();
        let literal_count = segs.iter().filter(|s| matches!(s, Seg::Literal(_))).count();
        CompiledPath {
            raw: raw.to_string(),
            segs,
            literal_count,
        }
    }

    pub fn is_templated(&self) -> bool {
        self.literal_count != self.segs.len()
    }

    /// Shape string used to match config rules to spec routes regardless of
    /// param names: `/users/{id}` and `/users/{userId}` → `/users/{}`.
    pub fn shape(&self) -> String {
        let mut out = String::new();
        for seg in &self.segs {
            out.push('/');
            match seg {
                Seg::Literal(l) => out.push_str(l),
                Seg::Param(_) => out.push_str("{}"),
            }
        }
        if out.is_empty() {
            out.push('/');
        }
        out
    }

    pub fn matches(&self, path_segs: &[&str]) -> bool {
        if self.segs.len() != path_segs.len() {
            return false;
        }
        self.segs.iter().zip(path_segs).all(|(seg, got)| match seg {
            Seg::Literal(l) => l == got,
            Seg::Param(_) => !got.is_empty(),
        })
    }
}

/// Split a normalized path into segments. `/` yields no segments.
pub fn split_path(path: &str) -> impl Iterator<Item = &str> {
    path.trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
}

/// Trailing-slash insensitive normalization: `/users/` == `/users`.
pub fn normalize(path: &str) -> &str {
    if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        path
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    Spec,
    Config,
    Both,
}

/// How the response body is produced. `Fixed`/`Example` are serialized once
/// at compile time — the hot path just clones `Bytes` (refcount bump).
pub enum ResponsePlan {
    Fixed {
        status: StatusCode,
        headers: Vec<(HeaderName, HeaderValue)>,
        body: Bytes,
        content_type: HeaderValue,
    },
    Example {
        status: StatusCode,
        body: Bytes,
        content_type: HeaderValue,
    },
    Schema {
        status: StatusCode,
        schema: Arc<Schema>,
        root: Arc<Value>,
        /// Spec-declared query params recognized at compile time: the client
        /// can size the root array (`?size=100`) and vary seeded content per
        /// page (`?page=2`).
        size_param: Option<String>,
        page_param: Option<String>,
    },
    Empty {
        status: StatusCode,
    },
}

impl ResponsePlan {
    pub fn kind(&self) -> &'static str {
        match self {
            ResponsePlan::Fixed { .. } => "fixed",
            ResponsePlan::Example { .. } => "example",
            ResponsePlan::Schema { .. } => "schema",
            ResponsePlan::Empty { .. } => "empty",
        }
    }
}

/// Admin-mutable runtime state; survives hot reloads (matched by rule id).
pub struct RuleRuntime {
    /// Master switch for error simulation on this rule.
    pub error_enabled: AtomicBool,
    /// Admin override; when set, wins over the file-configured error mode.
    pub error_override: ArcSwapOption<ErrorModeSpec>,
    /// Admin override; when set, wins over the file-configured latency.
    pub latency_override: ArcSwapOption<LatencySpec>,
    pub bucket: ArcSwapOption<TokenBucket>,
    pub hits: AtomicU64,
}

impl Default for RuleRuntime {
    fn default() -> Self {
        Self {
            error_enabled: AtomicBool::new(true),
            error_override: ArcSwapOption::empty(),
            latency_override: ArcSwapOption::empty(),
            bucket: ArcSwapOption::empty(),
            hits: AtomicU64::new(0),
        }
    }
}

impl RuleRuntime {
    pub fn clear_overrides(&self) {
        self.error_override.store(None);
        self.latency_override.store(None);
        self.error_enabled
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

pub struct MockRule {
    pub id: RuleId,
    /// Short stable key (fnv-1a hex) usable in admin URLs.
    pub key: String,
    /// `None` = matches any method (config-only rules).
    pub method: Option<Method>,
    pub path: CompiledPath,
    pub source: RuleSource,
    pub plan: ResponsePlan,
    /// Effective behavior after merging endpoint config with defaults.
    pub behavior: Behavior,
    pub runtime: Arc<RuleRuntime>,
}

#[derive(Default)]
pub struct MethodTable {
    pub exact: HashMap<String, Arc<MockRule>>,
    /// Sorted most-specific first (literal count desc, then leftmost-literal).
    pub templated: Vec<Arc<MockRule>>,
}

pub struct MockTable {
    pub methods: HashMap<Method, MethodTable>,
    pub any: MethodTable,
    /// All rules, for admin listing and id lookup.
    pub rules: Vec<Arc<MockRule>>,
    pub generation: u64,
    /// The raw (unmerged) config, exposed by the admin API for debugging.
    pub config: Arc<MockConfig>,
}

impl MockTable {
    pub fn rule_by_id(&self, id_or_key: &str) -> Option<&Arc<MockRule>> {
        self.rules
            .iter()
            .find(|r| r.id == id_or_key || r.key == id_or_key)
    }
}

/// FNV-1a 64-bit — tiny stable hash for rule keys and seed derivation.
pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_template() {
        let p = CompiledPath::parse("/users/{id}/posts");
        assert_eq!(p.segs.len(), 3);
        assert_eq!(p.literal_count, 2);
        assert!(p.is_templated());
        assert_eq!(p.shape(), "/users/{}/posts");
    }

    #[test]
    fn matches_segments() {
        let p = CompiledPath::parse("/users/{id}");
        assert!(p.matches(&["users", "42"]));
        assert!(!p.matches(&["users"]));
        assert!(!p.matches(&["teams", "42"]));
    }

    #[test]
    fn normalize_trailing_slash() {
        assert_eq!(normalize("/users/"), "/users");
        assert_eq!(normalize("/"), "/");
    }
}
