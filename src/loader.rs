//! Reads spec + config files from disk, detects JSON vs YAML, and parses
//! into typed models. The spec is also kept as a raw `serde_json::Value`
//! so `$ref` resolution stays a pointer lookup.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use crate::config::model::MockConfig;
use crate::error::LoadError;
use crate::openapi::model::OpenApiDoc;

/// Hard cap on spec/config file size — cheap insurance against YAML bombs.
const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct LoadedPaths {
    pub spec: PathBuf,
    pub config: Option<PathBuf>,
}

#[derive(Debug)]
pub struct LoadedDocs {
    /// Raw spec document for `$ref` resolution.
    pub spec_root: Arc<Value>,
    pub spec: OpenApiDoc,
    pub config: MockConfig,
}

pub fn load_all(paths: &LoadedPaths) -> Result<LoadedDocs, LoadError> {
    let spec_root = parse_document(&paths.spec)?;
    let spec: OpenApiDoc =
        serde_json::from_value(spec_root.clone()).map_err(|e| LoadError::Parse {
            kind: "OpenAPI spec",
            path: paths.spec.clone(),
            message: e.to_string(),
        })?;
    if !spec.openapi.starts_with("3.") {
        return Err(LoadError::Parse {
            kind: "OpenAPI spec",
            path: paths.spec.clone(),
            message: format!(
                "unsupported OpenAPI version {:?} (need 3.0/3.1)",
                spec.openapi
            ),
        });
    }

    let config = match &paths.config {
        Some(p) => {
            let raw = parse_document(p)?;
            serde_json::from_value(raw).map_err(|e| LoadError::Parse {
                kind: "mock config",
                path: p.clone(),
                message: e.to_string(),
            })?
        }
        None => MockConfig::default(),
    };

    config.defaults.validate("defaults")?;
    for ep in &config.endpoints {
        ep.behavior.validate(&ep.path)?;
        if is_reserved(&ep.path) {
            return Err(LoadError::ReservedPath(ep.path.clone()));
        }
        if let Some(r) = &ep.response {
            http::StatusCode::from_u16(r.status).map_err(|_| {
                LoadError::Validation(format!("{}: status {} is invalid", ep.path, r.status))
            })?;
            if r.body.is_some() && r.body_text.is_some() {
                return Err(LoadError::Validation(format!(
                    "{}: body and body_text are mutually exclusive",
                    ep.path
                )));
            }
        }
    }
    for path in spec.paths.keys() {
        if is_reserved(path) {
            return Err(LoadError::ReservedPath(path.clone()));
        }
    }

    Ok(LoadedDocs {
        spec_root: Arc::new(spec_root),
        spec,
        config,
    })
}

pub fn is_reserved(path: &str) -> bool {
    path == "/_mockmesh" || path.starts_with("/_mockmesh/")
}

/// Read a file and parse it into a JSON value, accepting JSON or YAML.
/// Extension decides first; otherwise the first non-whitespace byte is
/// sniffed (`{`/`[` → JSON) for better error messages.
pub fn parse_document(path: &Path) -> Result<Value, LoadError> {
    let meta = std::fs::metadata(path).map_err(|e| LoadError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(LoadError::TooLarge {
            path: path.to_path_buf(),
            size: meta.len(),
            max: MAX_FILE_BYTES,
        });
    }
    let text = std::fs::read_to_string(path).map_err(|e| LoadError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;

    let looks_json = match path.extension().and_then(|e| e.to_str()) {
        Some("json") => true,
        Some("yaml") | Some("yml") => false,
        _ => matches!(
            text.trim_start().as_bytes().first(),
            Some(b'{') | Some(b'[')
        ),
    };

    if looks_json {
        serde_json::from_str(&text).map_err(|e| LoadError::Parse {
            kind: "JSON document",
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    } else {
        serde_yaml_ng::from_str(&text).map_err(|e| LoadError::Parse {
            kind: "YAML document",
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use std::io::Write;

    fn write_temp(ext: &str, content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new().suffix(ext).tempfile().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn parses_yaml_and_json() {
        let y = write_temp(".yaml", "a: 1\n");
        assert_eq!(parse_document(y.path()).unwrap()["a"], 1);
        let j = write_temp(".json", r#"{"a": 1}"#);
        assert_eq!(parse_document(j.path()).unwrap()["a"], 1);
    }

    #[test]
    fn sniffs_json_without_extension() {
        let f = write_temp("", r#"  {"a": 2}"#);
        assert_eq!(parse_document(f.path()).unwrap()["a"], 2);
    }

    #[test]
    fn rejects_reserved_paths() {
        let spec = write_temp(
            ".yaml",
            "openapi: 3.0.0\npaths:\n  /_mockmesh/evil:\n    get:\n      responses: {}\n",
        );
        let err = load_all(&LoadedPaths {
            spec: spec.path().to_path_buf(),
            config: None,
        })
        .unwrap_err();
        assert!(matches!(err, LoadError::ReservedPath(_)));
    }

    #[test]
    fn rejects_non_3x_spec() {
        let spec = write_temp(".yaml", "openapi: 2.0.0\npaths: {}\n");
        assert!(
            load_all(&LoadedPaths {
                spec: spec.path().to_path_buf(),
                config: None,
            })
            .is_err()
        );
    }
}
