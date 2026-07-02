//! mock-mesh behavior config: per-endpoint latency, rate limiting, error
//! switches and fixed response overrides. Strict parsing
//! (`deny_unknown_fields`) so typos fail loudly instead of being ignored.

use std::collections::BTreeMap;

use http::Method;
use serde::{Deserialize, Serialize};

use crate::error::LoadError;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MockConfig {
    /// Fallback behavior applied to every endpoint whose rule leaves the
    /// corresponding field unset.
    #[serde(default)]
    pub defaults: Behavior,
    #[serde(default)]
    pub endpoints: Vec<EndpointRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointRule {
    /// OpenAPI-style template, e.g. `/users/{id}`. Param names don't have
    /// to match the spec — comparison is on segment shape.
    pub path: String,
    #[serde(default)]
    pub method: MethodMatch,
    #[serde(default)]
    pub behavior: Behavior,
    /// When set, replaces the spec-derived response entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<FixedResponse>,
}

/// "GET", "post", … or "any"/"*".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum MethodMatch {
    #[default]
    Any,
    One(Method),
}

impl TryFrom<String> for MethodMatch {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        if s.eq_ignore_ascii_case("any") || s == "*" {
            return Ok(MethodMatch::Any);
        }
        Method::from_bytes(s.to_ascii_uppercase().as_bytes())
            .map(MethodMatch::One)
            .map_err(|_| format!("invalid HTTP method: {s}"))
    }
}

impl<'de> Deserialize<'de> for MethodMatch {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        MethodMatch::try_from(s).map_err(serde::de::Error::custom)
    }
}

impl Serialize for MethodMatch {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            MethodMatch::Any => s.serialize_str("any"),
            MethodMatch::One(m) => s.serialize_str(m.as_str()),
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Behavior {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency: Option<LatencySpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimitSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_mode: Option<ErrorModeSpec>,
}

impl Behavior {
    /// Fill unset fields from `defaults`.
    pub fn merged_with(&self, defaults: &Behavior) -> Behavior {
        Behavior {
            latency: self.latency.clone().or_else(|| defaults.latency.clone()),
            rate_limit: self
                .rate_limit
                .clone()
                .or_else(|| defaults.rate_limit.clone()),
            error_mode: self
                .error_mode
                .clone()
                .or_else(|| defaults.error_mode.clone()),
        }
    }

    pub fn validate(&self, context: &str) -> Result<(), LoadError> {
        if let Some(rl) = &self.rate_limit {
            if rl.rps <= 0.0 || rl.rps.is_nan() || !rl.rps.is_finite() {
                return Err(LoadError::Validation(format!(
                    "{context}: rate_limit.rps must be a positive number"
                )));
            }
            if rl.burst == 0 {
                return Err(LoadError::Validation(format!(
                    "{context}: rate_limit.burst must be >= 1"
                )));
            }
            if !(0.0..=1.0).contains(&rl.reject_probability) {
                return Err(LoadError::Validation(format!(
                    "{context}: rate_limit.reject_probability must be within [0, 1]"
                )));
            }
        }
        if let Some(ErrorModeSpec::Status {
            code, probability, ..
        }) = &self.error_mode
        {
            if http::StatusCode::from_u16(*code).is_err() {
                return Err(LoadError::Validation(format!(
                    "{context}: error_mode.code {code} is not a valid HTTP status"
                )));
            }
            if let Some(p) = probability
                && !(0.0..=1.0).contains(p)
            {
                return Err(LoadError::Validation(format!(
                    "{context}: error_mode.probability must be within [0, 1]"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LatencySpec {
    /// Base delay applied to every request.
    #[serde(default)]
    pub fixed_ms: u64,
    /// Extra uniform random delay in `[0, jitter_ms]`.
    #[serde(default)]
    pub jitter_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimitSpec {
    /// Token refill rate (tokens per second).
    pub rps: f64,
    /// Bucket capacity (max burst).
    #[serde(default = "default_burst")]
    pub burst: u32,
    /// Probability of rejecting a request that *passed* the bucket —
    /// simulates flaky upstream throttling clients can't predict.
    #[serde(default)]
    pub reject_probability: f64,
    /// Status returned on rejection (usually 429).
    #[serde(default = "default_limited_status")]
    pub response_status: u16,
}

fn default_burst() -> u32 {
    1
}

fn default_limited_status() -> u16 {
    429
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ErrorModeSpec {
    /// Respond with a fixed error status, optionally only for a fraction of
    /// requests (`probability`; omitted = always).
    Status {
        code: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        probability: Option<f64>,
    },
    /// Accept the request and hold the connection open without answering.
    Hang {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_secs: Option<u64>,
    },
    /// Reset the TCP connection (RST) mid-request.
    Abort,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixedResponse {
    #[serde(default = "default_ok_status")]
    pub status: u16,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// JSON body. At most one `body*` field may be set.
    pub body: Option<serde_json::Value>,
    /// Raw text body, served as text/plain by default.
    pub body_text: Option<String>,
    /// Base64-encoded binary body — for attachments and other non-text
    /// payloads inlined directly in the config. Served as
    /// application/octet-stream by default.
    pub body_base64: Option<String>,
    /// Path (relative to the working directory) of a file whose bytes form
    /// the body. Read once at load time. Content-Type is guessed from the
    /// extension unless `content_type` is set.
    pub body_file: Option<String>,
    /// Explicit Content-Type, overriding the default chosen for the body kind.
    pub content_type: Option<String>,
    /// When set, adds `Content-Disposition: attachment; filename="..."` so
    /// clients treat the response as a downloadable attachment.
    pub filename: Option<String>,
}

fn default_ok_status() -> u16 {
    200
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn parses_full_config() {
        let yaml = r#"
defaults:
  latency: { fixed_ms: 20, jitter_ms: 30 }
endpoints:
  - path: /users/{id}
    method: GET
    behavior:
      rate_limit: { rps: 50, burst: 10, reject_probability: 0.05 }
      error_mode: { kind: status, code: 500, probability: 0.1 }
  - path: /payments
    method: POST
    behavior:
      error_mode: { kind: hang, max_secs: 120 }
  - path: /flaky
    behavior:
      error_mode: { kind: abort }
"#;
        let cfg: MockConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(cfg.endpoints.len(), 3);
        assert_eq!(cfg.endpoints[0].method, MethodMatch::One(Method::GET));
        assert_eq!(cfg.endpoints[2].method, MethodMatch::Any);
        assert!(matches!(
            cfg.endpoints[2].behavior.error_mode,
            Some(ErrorModeSpec::Abort)
        ));
    }

    #[test]
    fn unknown_fields_rejected() {
        let yaml = "endpoints:\n  - path: /x\n    latencyy: 5\n";
        assert!(serde_yaml_ng::from_str::<MockConfig>(yaml).is_err());
    }

    #[test]
    fn defaults_merge_fills_unset_only() {
        let defaults = Behavior {
            latency: Some(LatencySpec {
                fixed_ms: 20,
                jitter_ms: 0,
            }),
            rate_limit: None,
            error_mode: None,
        };
        let own = Behavior {
            latency: Some(LatencySpec {
                fixed_ms: 100,
                jitter_ms: 5,
            }),
            rate_limit: None,
            error_mode: None,
        };
        let merged = own.merged_with(&defaults);
        assert_eq!(merged.latency.unwrap().fixed_ms, 100);
        let merged_empty = Behavior::default().merged_with(&defaults);
        assert_eq!(merged_empty.latency.unwrap().fixed_ms, 20);
    }

    #[test]
    fn validation_rejects_bad_probability() {
        let b = Behavior {
            latency: None,
            rate_limit: Some(RateLimitSpec {
                rps: 10.0,
                burst: 1,
                reject_probability: 1.5,
                response_status: 429,
            }),
            error_mode: None,
        };
        assert!(b.validate("test").is_err());
    }
}
