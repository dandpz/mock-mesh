//! Regex-free request → rule matching. Exact paths are an O(1) hash hit;
//! templated paths are a linear scan over candidates pre-sorted by
//! specificity at compile time. No regex anywhere → no ReDoS surface.

use http::Method;

use super::{MockRule, MockTable, normalize, split_path};
use std::sync::Arc;

/// Precedence: method-exact > any-method-exact > method-templated >
/// any-method-templated. Within templated: most literals first, then
/// leftmost-literal wins (encoded in the compile-time sort).
pub fn find_rule<'t>(
    table: &'t MockTable,
    method: &Method,
    path: &str,
) -> Option<&'t Arc<MockRule>> {
    let path = normalize(path);
    let method_table = table.methods.get(method);

    if let Some(rule) = method_table.and_then(|t| t.exact.get(path)) {
        return Some(rule);
    }
    if let Some(rule) = table.any.exact.get(path) {
        return Some(rule);
    }

    let segs: Vec<&str> = split_path(path).collect();
    if let Some(rule) = method_table.and_then(|t| scan(&t.templated, &segs)) {
        return Some(rule);
    }
    scan(&table.any.templated, &segs)
}

fn scan<'t>(candidates: &'t [Arc<MockRule>], segs: &[&str]) -> Option<&'t Arc<MockRule>> {
    candidates.iter().find(|r| r.path.matches(segs))
}
