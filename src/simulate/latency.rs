use std::time::Duration;

use rand::RngExt;
use rand::rngs::SmallRng;

use crate::config::model::LatencySpec;

pub fn delay_for(spec: &LatencySpec, rng: &mut SmallRng) -> Duration {
    let jitter = if spec.jitter_ms > 0 {
        rng.random_range(0..=spec.jitter_ms)
    } else {
        0
    };
    Duration::from_millis(spec.fixed_ms.saturating_add(jitter))
}

pub async fn apply(spec: &LatencySpec, rng: &mut SmallRng) {
    let d = delay_for(spec, rng);
    if !d.is_zero() {
        tokio::time::sleep(d).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn jitter_within_bounds() {
        let spec = LatencySpec {
            fixed_ms: 100,
            jitter_ms: 50,
        };
        let mut rng = SmallRng::seed_from_u64(0);
        for _ in 0..100 {
            let d = delay_for(&spec, &mut rng);
            assert!((100..=150).contains(&(d.as_millis() as u64)));
        }
    }

    #[test]
    fn zero_spec_is_zero() {
        let spec = LatencySpec {
            fixed_ms: 0,
            jitter_ms: 0,
        };
        let mut rng = SmallRng::seed_from_u64(0);
        assert!(delay_for(&spec, &mut rng).is_zero());
    }
}
