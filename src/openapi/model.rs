//! Minimal serde model for the subset of OpenAPI 3.0/3.1 that mock-mesh
//! needs: paths, operations, responses, media types and schemas. Unknown
//! keywords are ignored on purpose — this is a tolerant subset parser,
//! never a validator.

use std::collections::BTreeMap;

use http::Method;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct OpenApiDoc {
    /// "3.0.x" or "3.1.x"
    pub openapi: String,
    #[serde(default)]
    pub paths: BTreeMap<String, PathItem>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PathItem {
    pub get: Option<Operation>,
    pub put: Option<Operation>,
    pub post: Option<Operation>,
    pub delete: Option<Operation>,
    pub options: Option<Operation>,
    pub head: Option<Operation>,
    pub patch: Option<Operation>,
    pub trace: Option<Operation>,
}

impl PathItem {
    pub fn operations(&self) -> impl Iterator<Item = (Method, &Operation)> {
        [
            (Method::GET, &self.get),
            (Method::PUT, &self.put),
            (Method::POST, &self.post),
            (Method::DELETE, &self.delete),
            (Method::OPTIONS, &self.options),
            (Method::HEAD, &self.head),
            (Method::PATCH, &self.patch),
            (Method::TRACE, &self.trace),
        ]
        .into_iter()
        .filter_map(|(m, op)| op.as_ref().map(|op| (m, op)))
    }
}

#[derive(Debug, Deserialize)]
pub struct Operation {
    #[serde(rename = "operationId")]
    pub operation_id: Option<String>,
    /// Keys: "200", "404", "2XX", "default"
    #[serde(default)]
    pub responses: BTreeMap<String, RefOr<ResponseObj>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ResponseObj {
    /// Media type → content, e.g. "application/json"
    #[serde(default)]
    pub content: BTreeMap<String, MediaType>,
}

#[derive(Debug, Default, Deserialize)]
pub struct MediaType {
    pub schema: Option<RefOr<Schema>>,
    pub example: Option<serde_json::Value>,
    #[serde(default)]
    pub examples: BTreeMap<String, RefOr<ExampleObj>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ExampleObj {
    pub value: Option<serde_json::Value>,
}

/// Either an inline value or a `$ref`. `Ref` is tried first: untagged
/// deserialization picks it whenever a `$ref` key is present, because the
/// inline variants have only optional fields and would match anything.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RefOr<T> {
    Ref {
        #[serde(rename = "$ref")]
        reference: String,
    },
    Item(T),
}

/// 3.1 allows `type` to be an array (e.g. `["string", "null"]`).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum TypeOrTypes {
    One(String),
    Many(Vec<String>),
}

impl TypeOrTypes {
    /// The first non-"null" type, if any.
    pub fn primary(&self) -> Option<&str> {
        match self {
            TypeOrTypes::One(t) => Some(t.as_str()),
            TypeOrTypes::Many(ts) => ts.iter().map(String::as_str).find(|t| *t != "null"),
        }
    }

    pub fn allows_null(&self) -> bool {
        match self {
            TypeOrTypes::One(t) => t == "null",
            TypeOrTypes::Many(ts) => ts.iter().any(|t| t == "null"),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Schema {
    #[serde(rename = "type")]
    pub ty: Option<TypeOrTypes>,
    pub format: Option<String>,
    #[serde(rename = "enum")]
    pub enum_values: Option<Vec<serde_json::Value>>,
    pub properties: Option<BTreeMap<String, RefOr<Schema>>>,
    #[serde(default)]
    pub required: Vec<String>,
    pub items: Option<Box<RefOr<Schema>>>,
    #[serde(rename = "oneOf")]
    pub one_of: Option<Vec<RefOr<Schema>>>,
    #[serde(rename = "anyOf")]
    pub any_of: Option<Vec<RefOr<Schema>>>,
    #[serde(rename = "allOf")]
    pub all_of: Option<Vec<RefOr<Schema>>>,
    pub example: Option<serde_json::Value>,
    pub default: Option<serde_json::Value>,
    /// 3.0 nullability; 3.1 uses `type: [T, "null"]` instead.
    pub nullable: Option<bool>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    #[serde(rename = "minLength")]
    pub min_length: Option<usize>,
    #[serde(rename = "maxLength")]
    pub max_length: Option<usize>,
    #[serde(rename = "minItems")]
    pub min_items: Option<usize>,
    #[serde(rename = "maxItems")]
    pub max_items: Option<usize>,
}

impl Schema {
    pub fn is_nullable(&self) -> bool {
        self.nullable == Some(true) || self.ty.as_ref().is_some_and(TypeOrTypes::allows_null)
    }
}
