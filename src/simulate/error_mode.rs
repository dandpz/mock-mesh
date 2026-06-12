//! Error-state switches: forced status codes (optionally probabilistic),
//! black-hole hangs, and TCP connection aborts.

use std::time::Duration;

use rand::RngExt;
use rand::rngs::SmallRng;

use crate::config::model::ErrorModeSpec;

/// Hung requests are always bounded so they can't pin connections forever.
pub const DEFAULT_HANG_SECS: u64 = 120;

pub enum ErrorAction {
    Respond {
        status: u16,
        body: Option<serde_json::Value>,
    },
    Hang(Duration),
    Abort,
}

/// Decide what (if anything) the error switch does to this request.
pub fn decide(spec: &ErrorModeSpec, rng: &mut SmallRng) -> Option<ErrorAction> {
    match spec {
        ErrorModeSpec::Status {
            code,
            body,
            probability,
        } => {
            if let Some(p) = probability
                && !rng.random_bool(p.clamp(0.0, 1.0))
            {
                return None;
            }
            Some(ErrorAction::Respond {
                status: *code,
                body: body.clone(),
            })
        }
        ErrorModeSpec::Hang { max_secs } => Some(ErrorAction::Hang(Duration::from_secs(
            max_secs.unwrap_or(DEFAULT_HANG_SECS),
        ))),
        ErrorModeSpec::Abort => Some(ErrorAction::Abort),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn status_without_probability_always_fires() {
        let spec = ErrorModeSpec::Status {
            code: 500,
            body: None,
            probability: None,
        };
        let mut rng = SmallRng::seed_from_u64(0);
        for _ in 0..10 {
            assert!(matches!(
                decide(&spec, &mut rng),
                Some(ErrorAction::Respond { status: 500, .. })
            ));
        }
    }

    #[test]
    fn probability_zero_never_fires() {
        let spec = ErrorModeSpec::Status {
            code: 500,
            body: None,
            probability: Some(0.0),
        };
        let mut rng = SmallRng::seed_from_u64(0);
        for _ in 0..100 {
            assert!(decide(&spec, &mut rng).is_none());
        }
    }

    #[test]
    fn hang_uses_default_cap() {
        let spec = ErrorModeSpec::Hang { max_secs: None };
        let mut rng = SmallRng::seed_from_u64(0);
        match decide(&spec, &mut rng) {
            Some(ErrorAction::Hang(d)) => assert_eq!(d.as_secs(), DEFAULT_HANG_SECS),
            _ => panic!("expected hang"),
        }
    }
}
