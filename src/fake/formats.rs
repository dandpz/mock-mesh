//! Format-aware fake string generation. Everything is hand-rolled (uuid,
//! RFC 3339 timestamps, base64) to keep the dependency tree small.

use rand::RngExt;
use rand::rngs::SmallRng;

use crate::openapi::model::Schema;

const WORDS: &[&str] = &[
    "alpha", "breeze", "cobalt", "delta", "ember", "falcon", "garnet", "harbor", "indigo",
    "jasper", "krypton", "lumen", "meadow", "nimbus", "onyx", "prairie", "quartz", "raven",
    "sierra", "tundra", "umber", "vertex", "willow", "xenon", "yonder", "zephyr",
];

pub fn string_for(schema: &Schema, rng: &mut SmallRng) -> String {
    let s = match schema.format.as_deref() {
        Some("uuid") => uuid_v4(rng),
        Some("email") => format!("{}{}@example.com", word(rng), rng.random_range(1..100)),
        Some("date") => {
            let (y, m, d) = random_date(rng);
            format!("{y:04}-{m:02}-{d:02}")
        }
        Some("date-time") => {
            let (y, m, d) = random_date(rng);
            format!(
                "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
                rng.random_range(0..24),
                rng.random_range(0..60),
                rng.random_range(0..60)
            )
        }
        Some("uri") | Some("url") => format!("https://example.com/{}", word(rng)),
        Some("hostname") => format!("{}.example.com", word(rng)),
        Some("ipv4") => format!(
            "10.{}.{}.{}",
            rng.random_range(0..256),
            rng.random_range(0..256),
            rng.random_range(1..255)
        ),
        Some("ipv6") => format!(
            "2001:db8::{:x}:{:x}",
            rng.random_range(0u16..=u16::MAX),
            rng.random_range(0u16..=u16::MAX)
        ),
        Some("byte") => {
            let bytes: Vec<u8> = (0..9).map(|_| rng.random_range(0..=255)).collect();
            base64(&bytes)
        }
        Some("password") => "hunter2".to_string(),
        _ => format!("{}-{}", word(rng), word(rng)),
    };
    clamp_len(s, schema.min_length, schema.max_length, rng)
}

fn word(rng: &mut SmallRng) -> &'static str {
    WORDS[rng.random_range(0..WORDS.len())]
}

/// Dates in 2026, capped at day 28 so every month is valid.
fn random_date(rng: &mut SmallRng) -> (u16, u8, u8) {
    (2026, rng.random_range(1..=12), rng.random_range(1..=28))
}

fn uuid_v4(rng: &mut SmallRng) -> String {
    let mut b = [0u8; 16];
    rng.fill(&mut b[..]);
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0],
        b[1],
        b[2],
        b[3],
        b[4],
        b[5],
        b[6],
        b[7],
        b[8],
        b[9],
        b[10],
        b[11],
        b[12],
        b[13],
        b[14],
        b[15]
    )
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn clamp_len(mut s: String, min: Option<usize>, max: Option<usize>, rng: &mut SmallRng) -> String {
    if let Some(min) = min {
        while s.chars().count() < min {
            s.push(char::from(b'a' + rng.random_range(0..26u8)));
        }
    }
    if let Some(max) = max
        && s.chars().count() > max
    {
        s = s.chars().take(max).collect();
    }
    s
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use rand::SeedableRng;

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(1)
    }

    fn schema_with_format(f: &str) -> Schema {
        serde_json::from_value(serde_json::json!({ "type": "string", "format": f })).unwrap()
    }

    #[test]
    fn uuid_shape() {
        let s = string_for(&schema_with_format("uuid"), &mut rng());
        assert_eq!(s.len(), 36);
        assert_eq!(s.as_bytes()[14], b'4'); // version nibble
        let parts: Vec<&str> = s.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
    }

    #[test]
    fn email_shape() {
        let s = string_for(&schema_with_format("email"), &mut rng());
        assert!(s.contains('@') && s.ends_with("example.com"));
    }

    #[test]
    fn date_time_is_rfc3339ish() {
        let s = string_for(&schema_with_format("date-time"), &mut rng());
        assert_eq!(s.len(), 20);
        assert!(s.ends_with('Z') && s.contains('T'));
    }

    #[test]
    fn base64_known_vector() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }

    #[test]
    fn length_clamping() {
        let schema: Schema = serde_json::from_value(serde_json::json!({
            "type": "string", "minLength": 40, "maxLength": 45
        }))
        .unwrap();
        let s = string_for(&schema, &mut rng());
        let n = s.chars().count();
        assert!((40..=45).contains(&n));
    }
}
