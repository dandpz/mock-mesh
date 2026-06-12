//! Local `$ref` resolution against the raw spec document.
//!
//! The whole spec is kept as one `serde_json::Value`, so a `$ref` like
//! `#/components/schemas/Pet` is just an RFC 6901 pointer lookup. Only
//! document-local refs are supported; external files/URLs are rejected at
//! load time (no file reads or network fetches beyond the given paths).

use serde_json::Value;

use crate::error::LoadError;
use crate::openapi::model::Schema;

pub struct RefResolver<'a> {
    root: &'a Value,
}

impl<'a> RefResolver<'a> {
    pub fn new(root: &'a Value) -> Self {
        Self { root }
    }

    /// Resolve a `$ref` string to its raw value.
    pub fn lookup(&self, reference: &str) -> Result<&'a Value, LoadError> {
        let Some(pointer) = reference.strip_prefix('#') else {
            return Err(LoadError::ExternalRef(reference.to_string()));
        };
        self.root
            .pointer(pointer)
            .ok_or_else(|| LoadError::UnknownRef(reference.to_string()))
    }

    /// Resolve a `$ref` string to a typed `Schema`.
    pub fn resolve_schema(&self, reference: &str) -> Result<Schema, LoadError> {
        let raw = self.lookup(reference)?;
        serde_json::from_value(raw.clone()).map_err(|e| LoadError::Parse {
            kind: "schema",
            path: std::path::PathBuf::from(reference),
            message: e.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_local_ref() {
        let root = json!({
            "components": { "schemas": { "Pet": { "type": "string" } } }
        });
        let r = RefResolver::new(&root);
        let schema = r.resolve_schema("#/components/schemas/Pet").unwrap();
        assert_eq!(schema.ty.unwrap().primary(), Some("string"));
    }

    #[test]
    fn rejects_external_ref() {
        let root = json!({});
        let r = RefResolver::new(&root);
        assert!(matches!(
            r.lookup("other.yaml#/components/schemas/Pet"),
            Err(LoadError::ExternalRef(_))
        ));
    }

    #[test]
    fn unknown_ref_errors() {
        let root = json!({});
        let r = RefResolver::new(&root);
        assert!(matches!(
            r.lookup("#/components/schemas/Missing"),
            Err(LoadError::UnknownRef(_))
        ));
    }

    #[test]
    fn ref_with_escaped_segments() {
        let root = json!({ "components": { "schemas": { "a/b": { "type": "integer" } } } });
        let r = RefResolver::new(&root);
        let schema = r.resolve_schema("#/components/schemas/a~1b").unwrap();
        assert_eq!(schema.ty.unwrap().primary(), Some("integer"));
    }
}
