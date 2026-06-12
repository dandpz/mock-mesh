//! Schema → plausible fake JSON. Recursion is bounded by a depth cap and a
//! `$ref` stack (cycles degrade to `null`, never overflow). With `--seed`
//! the RNG is derived from `seed ^ fnv1a64(rule_id)`, making every response
//! for an endpoint byte-identical across requests and restarts.

pub mod formats;

use rand::RngExt;
use rand::rngs::SmallRng;
use serde_json::{Map, Value};

use crate::openapi::model::{RefOr, Schema, TypeOrTypes};
use crate::openapi::resolve::RefResolver;

const MAX_DEPTH: u8 = 8;
/// Objects with more properties than this only get `required` + a few extra.
const MAX_EAGER_PROPS: usize = 8;

pub struct GenCtx<'a> {
    pub rng: SmallRng,
    pub resolver: RefResolver<'a>,
    depth: u8,
    ref_stack: Vec<String>,
}

impl<'a> GenCtx<'a> {
    pub fn new(rng: SmallRng, root: &'a Value) -> Self {
        Self {
            rng,
            resolver: RefResolver::new(root),
            depth: 0,
            ref_stack: Vec::new(),
        }
    }
}

pub fn generate(schema: &Schema, ctx: &mut GenCtx<'_>) -> Value {
    if ctx.depth > MAX_DEPTH {
        return Value::Null;
    }

    if let Some(example) = &schema.example {
        return example.clone();
    }
    if let Some(default) = &schema.default {
        return default.clone();
    }
    if let Some(values) = &schema.enum_values
        && !values.is_empty()
    {
        let i = ctx.rng.random_range(0..values.len());
        return values[i].clone();
    }

    if let Some(all_of) = &schema.all_of {
        return generate_all_of(all_of, ctx);
    }
    if let Some(first) = schema
        .one_of
        .as_ref()
        .or(schema.any_of.as_ref())
        .and_then(|v| v.first())
    {
        return generate_ref_or(first, ctx);
    }

    // Skew toward real values: nullable only occasionally yields null.
    if schema.is_nullable() && ctx.rng.random_range(0..10) == 0 {
        return Value::Null;
    }

    let ty = schema
        .ty
        .as_ref()
        .and_then(TypeOrTypes::primary)
        .map(str::to_owned)
        .or_else(|| infer_type(schema));

    match ty.as_deref() {
        Some("string") => Value::String(formats::string_for(schema, &mut ctx.rng)),
        Some("integer") => Value::from(gen_integer(schema, &mut ctx.rng)),
        Some("number") => gen_number(schema, &mut ctx.rng),
        Some("boolean") => Value::Bool(ctx.rng.random_range(0..2) == 1),
        Some("array") => gen_array(schema, ctx),
        Some("object") => gen_object(schema, ctx),
        _ => Value::Null,
    }
}

fn infer_type(schema: &Schema) -> Option<String> {
    if schema.properties.is_some() {
        Some("object".to_string())
    } else if schema.items.is_some() {
        Some("array".to_string())
    } else {
        None
    }
}

fn generate_ref_or(ref_or: &RefOr<Schema>, ctx: &mut GenCtx<'_>) -> Value {
    match ref_or {
        RefOr::Item(s) => {
            ctx.depth += 1;
            let v = generate(s, ctx);
            ctx.depth -= 1;
            v
        }
        RefOr::Ref { reference } => {
            if ctx.ref_stack.iter().any(|r| r == reference) || ctx.depth > MAX_DEPTH {
                return Value::Null; // cycle or too deep: degrade gracefully
            }
            let Ok(schema) = ctx.resolver.resolve_schema(reference) else {
                return Value::Null;
            };
            ctx.ref_stack.push(reference.clone());
            ctx.depth += 1;
            let v = generate(&schema, ctx);
            ctx.depth -= 1;
            ctx.ref_stack.pop();
            v
        }
    }
}

/// Best-effort allOf: generate each member and merge resulting objects.
fn generate_all_of(members: &[RefOr<Schema>], ctx: &mut GenCtx<'_>) -> Value {
    let mut merged = Map::new();
    for member in members {
        if let Value::Object(obj) = generate_ref_or(member, ctx) {
            merged.extend(obj);
        }
    }
    Value::Object(merged)
}

fn gen_integer(schema: &Schema, rng: &mut SmallRng) -> i64 {
    let min = schema.minimum.map_or(0, |m| m.ceil() as i64);
    let max = schema
        .maximum
        .map_or(min.max(0) + 1000, |m| m.floor() as i64);
    if min >= max {
        return min;
    }
    rng.random_range(min..=max)
}

fn gen_number(schema: &Schema, rng: &mut SmallRng) -> Value {
    let min = schema.minimum.unwrap_or(0.0);
    let max = schema.maximum.unwrap_or(min + 1000.0);
    let raw = if min < max {
        rng.random_range(min..max)
    } else {
        min
    };
    // Two decimals look plausible and serialize compactly.
    Value::from((raw * 100.0).round() / 100.0)
}

fn gen_array(schema: &Schema, ctx: &mut GenCtx<'_>) -> Value {
    let min = schema.min_items.unwrap_or(2);
    let max = schema.max_items.unwrap_or(min.max(2)).max(min);
    let len = if min < max {
        ctx.rng.random_range(min..=max.min(min + 4))
    } else {
        min
    };
    let Some(items) = &schema.items else {
        return Value::Array(Vec::new());
    };
    Value::Array((0..len).map(|_| generate_ref_or(items, ctx)).collect())
}

fn gen_object(schema: &Schema, ctx: &mut GenCtx<'_>) -> Value {
    let Some(props) = &schema.properties else {
        return Value::Object(Map::new());
    };
    let eager = props.len() <= MAX_EAGER_PROPS;
    let mut out = Map::new();
    let mut extra_budget = MAX_EAGER_PROPS.saturating_sub(schema.required.len());
    for (name, prop) in props {
        let required = schema.required.iter().any(|r| r == name);
        if !eager && !required {
            if extra_budget == 0 {
                continue;
            }
            extra_budget -= 1;
        }
        out.insert(name.clone(), generate_ref_or(prop, ctx));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use rand::SeedableRng;
    use serde_json::json;

    fn gen_from(schema_json: Value, root: &Value, seed: u64) -> Value {
        let schema: Schema = serde_json::from_value(schema_json).unwrap();
        let mut ctx = GenCtx::new(SmallRng::seed_from_u64(seed), root);
        generate(&schema, &mut ctx)
    }

    #[test]
    fn deterministic_with_same_seed() {
        let root = json!({});
        let schema = json!({
            "type": "object",
            "required": ["id", "name", "tags"],
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "name": { "type": "string" },
                "tags": { "type": "array", "items": { "type": "string" } }
            }
        });
        let a = gen_from(schema.clone(), &root, 42);
        let b = gen_from(schema, &root, 42);
        assert_eq!(a, b);
    }

    #[test]
    fn example_wins_over_generation() {
        let root = json!({});
        let v = gen_from(json!({ "type": "string", "example": "fixed" }), &root, 1);
        assert_eq!(v, json!("fixed"));
    }

    #[test]
    fn enum_picks_member() {
        let root = json!({});
        let v = gen_from(json!({ "enum": ["a", "b", "c"] }), &root, 7);
        assert!(["a", "b", "c"].contains(&v.as_str().unwrap()));
    }

    #[test]
    fn circular_ref_degrades_to_null() {
        let root = json!({
            "components": { "schemas": { "Node": {
                "type": "object",
                "required": ["next"],
                "properties": { "next": { "$ref": "#/components/schemas/Node" } }
            } } }
        });
        let v = gen_from(
            json!({ "$ref-free": true, "type": "object", "required": ["next"],
                "properties": { "next": { "$ref": "#/components/schemas/Node" } } }),
            &root,
            3,
        );
        // The chain must terminate in a null, not overflow.
        let mut cur = &v;
        let mut hops = 0;
        while let Some(next) = cur.get("next") {
            cur = next;
            hops += 1;
            assert!(hops < 32);
        }
        assert!(cur.is_null() || cur.is_object());
    }

    #[test]
    fn one_of_picks_first() {
        let root = json!({});
        let v = gen_from(
            json!({ "oneOf": [ { "type": "integer", "minimum": 5, "maximum": 5 },
                               { "type": "string" } ] }),
            &root,
            9,
        );
        assert_eq!(v, json!(5));
    }

    #[test]
    fn all_of_merges_objects() {
        let root = json!({});
        let v = gen_from(
            json!({ "allOf": [
                { "type": "object", "required": ["a"], "properties": { "a": { "type": "integer", "minimum": 1, "maximum": 1 } } },
                { "type": "object", "required": ["b"], "properties": { "b": { "type": "integer", "minimum": 2, "maximum": 2 } } }
            ] }),
            &root,
            11,
        );
        assert_eq!(v, json!({ "a": 1, "b": 2 }));
    }

    #[test]
    fn integer_respects_bounds() {
        let root = json!({});
        for seed in 0..50 {
            let v = gen_from(
                json!({ "type": "integer", "minimum": 10, "maximum": 20 }),
                &root,
                seed,
            );
            let n = v.as_i64().unwrap();
            assert!((10..=20).contains(&n));
        }
    }

    #[test]
    fn type_array_31_style() {
        let root = json!({});
        let v = gen_from(json!({ "type": ["string", "null"] }), &root, 5);
        assert!(v.is_string() || v.is_null());
    }
}
