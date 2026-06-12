//! Merges the OpenAPI spec and the mock config into an immutable
//! `MockTable`. Bodies that can be produced ahead of time (spec examples,
//! fixed config responses) are serialized to `Bytes` here, once.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bytes::Bytes;
use http::{HeaderName, HeaderValue, Method, StatusCode};

use crate::config::model::{EndpointRule, FixedResponse, MethodMatch};
use crate::error::LoadError;
use crate::loader::LoadedDocs;
use crate::openapi::model::{MediaType, Operation, RefOr, ResponseObj};
use crate::openapi::resolve::RefResolver;

use super::{
    CompiledPath, MethodTable, MockRule, MockTable, ResponsePlan, RuleSource, Seg, fnv1a64,
};

pub fn build_table(docs: &LoadedDocs, prev: Option<&MockTable>) -> Result<MockTable, LoadError> {
    let resolver = RefResolver::new(&docs.spec_root);
    let defaults = &docs.config.defaults;
    let mut rules: Vec<Arc<MockRule>> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut used_config: HashSet<usize> = HashSet::new();

    // Spec routes, augmented by matching config endpoints.
    for (raw_path, item) in &docs.spec.paths {
        let path = CompiledPath::parse(raw_path);
        for (method, op) in item.operations() {
            let shape_key = format!("{method} {}", path.shape());
            if !seen.insert(shape_key.clone()) {
                return Err(LoadError::Validation(format!(
                    "duplicate route {method} {raw_path} (same shape as an earlier route)"
                )));
            }

            let config_ep =
                find_config_match(&docs.config.endpoints, &method, &path).inspect(|(i, _)| {
                    used_config.insert(*i);
                });

            let (plan, source) = match config_ep.and_then(|(_, ep)| ep.response.as_ref()) {
                Some(fixed) => (plan_from_fixed(fixed, raw_path)?, RuleSource::Both),
                None => {
                    let source = if config_ep.is_some() {
                        RuleSource::Both
                    } else {
                        RuleSource::Spec
                    };
                    (plan_from_operation(op, &resolver, docs)?, source)
                }
            };

            let behavior = config_ep
                .map(|(_, ep)| ep.behavior.clone())
                .unwrap_or_default()
                .merged_with(defaults);

            rules.push(make_rule(
                Some(method),
                path.clone(),
                source,
                plan,
                behavior,
                prev,
            ));
        }
    }

    // Config-only routes (not present in the spec).
    for (i, ep) in docs.config.endpoints.iter().enumerate() {
        if used_config.contains(&i) {
            continue;
        }
        let path = CompiledPath::parse(&ep.path);
        let method = match &ep.method {
            MethodMatch::Any => None,
            MethodMatch::One(m) => Some(m.clone()),
        };
        let shape_key = format!(
            "{} {}",
            method.as_ref().map_or("ANY", Method::as_str),
            path.shape()
        );
        if !seen.insert(shape_key) {
            return Err(LoadError::Validation(format!(
                "duplicate config endpoint {} {}",
                method.as_ref().map_or("ANY", Method::as_str),
                ep.path
            )));
        }
        let plan = match &ep.response {
            Some(fixed) => plan_from_fixed(fixed, &ep.path)?,
            None => ResponsePlan::Empty {
                status: StatusCode::OK,
            },
        };
        let behavior = ep.behavior.merged_with(defaults);
        rules.push(make_rule(
            method,
            path,
            RuleSource::Config,
            plan,
            behavior,
            prev,
        ));
    }

    let mut methods: HashMap<Method, MethodTable> = HashMap::new();
    let mut any = MethodTable::default();
    for rule in &rules {
        let table = match &rule.method {
            Some(m) => methods.entry(m.clone()).or_default(),
            None => &mut any,
        };
        if rule.path.is_templated() {
            table.templated.push(rule.clone());
        } else {
            table
                .exact
                .insert(super::normalize(&rule.path.raw).to_string(), rule.clone());
        }
    }
    for table in methods.values_mut() {
        sort_by_specificity(&mut table.templated);
    }
    sort_by_specificity(&mut any.templated);

    Ok(MockTable {
        methods,
        any,
        rules,
        generation: prev.map_or(1, |p| p.generation + 1),
        config: Arc::new(docs.config.clone()),
    })
}

/// Most-specific first: more literals win; on a tie the leftmost literal
/// wins (`/a/{x}/c` vs `/a/b/{y}` → the latter first). Raw path last for a
/// deterministic total order.
fn sort_by_specificity(rules: &mut [Arc<MockRule>]) {
    rules.sort_by(|a, b| {
        b.path
            .literal_count
            .cmp(&a.path.literal_count)
            .then_with(|| seg_kinds(&a.path.segs).cmp(&seg_kinds(&b.path.segs)))
            .then_with(|| a.path.raw.cmp(&b.path.raw))
    });
}

fn seg_kinds(segs: &[Seg]) -> Vec<u8> {
    segs.iter()
        .map(|s| match s {
            Seg::Literal(_) => 0,
            Seg::Param(_) => 1,
        })
        .collect()
}

fn make_rule(
    method: Option<Method>,
    path: CompiledPath,
    source: RuleSource,
    plan: ResponsePlan,
    behavior: crate::config::model::Behavior,
    prev: Option<&MockTable>,
) -> Arc<MockRule> {
    let id = format!(
        "{} {}",
        method.as_ref().map_or("ANY", Method::as_str),
        path.raw
    );
    let key = format!("{:012x}", fnv1a64(id.as_bytes()) & 0xffff_ffff_ffff);

    // Carry runtime state (admin overrides, bucket debt, hit count) across
    // reloads so a config edit doesn't reset live simulations.
    let runtime = prev
        .and_then(|t| t.rule_by_id(&id))
        .map(|r| r.runtime.clone())
        .unwrap_or_default();

    match &behavior.rate_limit {
        Some(spec) => {
            let keep = runtime
                .bucket
                .load()
                .as_ref()
                .is_some_and(|b| b.params() == (spec.rps, spec.burst));
            if !keep {
                runtime.bucket.store(Some(Arc::new(
                    crate::simulate::rate_limit::TokenBucket::new(spec.rps, spec.burst),
                )));
            }
        }
        None => runtime.bucket.store(None),
    }

    Arc::new(MockRule {
        id,
        key,
        method,
        path,
        source,
        plan,
        behavior,
        runtime,
    })
}

fn find_config_match<'c>(
    endpoints: &'c [EndpointRule],
    method: &Method,
    path: &CompiledPath,
) -> Option<(usize, &'c EndpointRule)> {
    let shape = path.shape();
    endpoints.iter().enumerate().find(|(_, ep)| {
        let method_ok = match &ep.method {
            MethodMatch::Any => true,
            MethodMatch::One(m) => m == method,
        };
        method_ok && CompiledPath::parse(&ep.path).shape() == shape
    })
}

fn plan_from_fixed(fixed: &FixedResponse, context: &str) -> Result<ResponsePlan, LoadError> {
    let status = StatusCode::from_u16(fixed.status).map_err(|_| {
        LoadError::Validation(format!("{context}: invalid status {}", fixed.status))
    })?;
    let mut headers = Vec::with_capacity(fixed.headers.len());
    for (name, value) in &fixed.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| LoadError::Validation(format!("{context}: invalid header name {name}")))?;
        let value = HeaderValue::from_str(value).map_err(|_| {
            LoadError::Validation(format!("{context}: invalid header value for {name}"))
        })?;
        headers.push((name, value));
    }
    let (body, content_type) = match (&fixed.body, &fixed.body_text) {
        (Some(json), _) => (
            Bytes::from(serde_json::to_vec(json).map_err(|e| {
                LoadError::Validation(format!("{context}: unserializable body: {e}"))
            })?),
            HeaderValue::from_static("application/json"),
        ),
        (None, Some(text)) => (
            Bytes::from(text.clone().into_bytes()),
            HeaderValue::from_static("text/plain; charset=utf-8"),
        ),
        (None, None) => (Bytes::new(), HeaderValue::from_static("application/json")),
    };
    Ok(ResponsePlan::Fixed {
        status,
        headers,
        body,
        content_type,
    })
}

fn plan_from_operation(
    op: &Operation,
    resolver: &RefResolver<'_>,
    docs: &LoadedDocs,
) -> Result<ResponsePlan, LoadError> {
    let Some((status, response_ref)) = pick_response(op) else {
        return Ok(ResponsePlan::Empty {
            status: StatusCode::OK,
        });
    };

    let resolved: ResponseObj;
    let response: &ResponseObj = match response_ref {
        RefOr::Item(r) => r,
        RefOr::Ref { reference } => {
            let raw = resolver.lookup(reference)?;
            resolved = serde_json::from_value(raw.clone()).map_err(|e| LoadError::Parse {
                kind: "response object",
                path: reference.into(),
                message: e.to_string(),
            })?;
            &resolved
        }
    };

    let Some((media_key, media)) = pick_content(response) else {
        return Ok(ResponsePlan::Empty { status });
    };

    if let Some(example) = example_value(media, resolver)? {
        let (body, content_type) = serialize_example(&example, media_key);
        return Ok(ResponsePlan::Example {
            status,
            body,
            content_type,
        });
    }

    match &media.schema {
        Some(schema_ref) => {
            let schema = match schema_ref {
                RefOr::Item(s) => s.clone(),
                RefOr::Ref { reference } => resolver.resolve_schema(reference)?,
            };
            Ok(ResponsePlan::Schema {
                status,
                schema: Arc::new(schema),
                root: docs.spec_root.clone(),
            })
        }
        None => Ok(ResponsePlan::Empty { status }),
    }
}

/// Pick the response to mock: lowest 2xx, else `default` (served as 200),
/// else the lowest other status. Range keys like `2XX` map to the floor.
fn pick_response(op: &Operation) -> Option<(StatusCode, &RefOr<ResponseObj>)> {
    op.responses
        .iter()
        .filter_map(|(key, resp)| {
            let (rank, code) = score_status_key(key)?;
            Some((rank, code, resp))
        })
        .min_by_key(|(rank, code, _)| (*rank, *code))
        .and_then(|(_, code, resp)| StatusCode::from_u16(code).ok().map(|s| (s, resp)))
}

fn score_status_key(key: &str) -> Option<(u8, u16)> {
    if let Ok(code) = key.parse::<u16>() {
        let rank = if (200..300).contains(&code) { 0 } else { 3 };
        return Some((rank, code));
    }
    if key.len() == 3 && key[1..].eq_ignore_ascii_case("xx") {
        let hundreds = key.as_bytes()[0].checked_sub(b'0')?;
        if (1..=5).contains(&hundreds) {
            let code = u16::from(hundreds) * 100;
            let rank = if hundreds == 2 { 1 } else { 3 };
            return Some((rank, code));
        }
    }
    if key == "default" {
        return Some((2, 200));
    }
    None
}

/// Prefer application/json, then any `+json` type, then the first entry.
fn pick_content(response: &ResponseObj) -> Option<(&str, &MediaType)> {
    if let Some(m) = response.content.get("application/json") {
        return Some(("application/json", m));
    }
    if let Some((k, m)) = response
        .content
        .iter()
        .find(|(k, _)| k.split(';').next().is_some_and(|t| t.ends_with("+json")))
    {
        return Some((k.as_str(), m));
    }
    response.content.iter().next().map(|(k, m)| (k.as_str(), m))
}

fn example_value(
    media: &MediaType,
    resolver: &RefResolver<'_>,
) -> Result<Option<serde_json::Value>, LoadError> {
    if let Some(v) = &media.example {
        return Ok(Some(v.clone()));
    }
    if let Some(first) = media.examples.values().next() {
        let value = match first {
            RefOr::Item(e) => e.value.clone(),
            RefOr::Ref { reference } => {
                let raw = resolver.lookup(reference)?;
                raw.get("value").cloned()
            }
        };
        return Ok(value);
    }
    Ok(None)
}

fn serialize_example(example: &serde_json::Value, media_key: &str) -> (Bytes, HeaderValue) {
    let is_json = media_key == "application/json"
        || media_key
            .split(';')
            .next()
            .is_some_and(|t| t.ends_with("+json"));
    if !is_json && let serde_json::Value::String(s) = example {
        let content_type = HeaderValue::from_str(media_key)
            .unwrap_or_else(|_| HeaderValue::from_static("text/plain; charset=utf-8"));
        return (Bytes::from(s.clone().into_bytes()), content_type);
    }
    (
        Bytes::from(serde_json::to_vec(example).unwrap_or_default()),
        HeaderValue::from_static("application/json"),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn status_key_priority() {
        // lowest 2xx beats default beats 4xx
        assert!(score_status_key("200").unwrap() < score_status_key("201").unwrap());
        assert!(score_status_key("201").unwrap() < score_status_key("2XX").unwrap());
        assert!(score_status_key("2XX").unwrap() < score_status_key("default").unwrap());
        assert!(score_status_key("default").unwrap() < score_status_key("404").unwrap());
        assert!(score_status_key("bogus").is_none());
    }
}
