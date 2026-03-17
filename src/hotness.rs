use chrono::{DateTime, Utc};

const DEFAULT_HALF_LIFE_DAYS: f64 = 7.0;
const IMPORTANCE_WEIGHT: f64 = 0.7;
const HOTNESS_WEIGHT: f64 = 0.3;

/// Compute hotness score based on access frequency and recency.
/// Returns value in [0.0, 1.0].
///
/// Formula: sigmoid(log1p(active_count)) * exp(-age_days / half_life)
pub fn hotness_score(active_count: i64, updated_at: &DateTime<Utc>, half_life_days: Option<f64>) -> f64 {
    let half_life = half_life_days.unwrap_or(DEFAULT_HALF_LIFE_DAYS);
    let age_days = (Utc::now() - *updated_at).num_seconds() as f64 / 86400.0;

    let frequency = sigmoid((active_count as f64).ln_1p());
    let recency = (-age_days / half_life).exp();

    frequency * recency
}

/// Blend importance and hotness into a final ranking score.
pub fn final_score(importance: f64, active_count: i64, updated_at: &DateTime<Utc>) -> f64 {
    let hotness = hotness_score(active_count, updated_at, None);
    IMPORTANCE_WEIGHT * importance + HOTNESS_WEIGHT * hotness
}

fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_zero_access_low_hotness() {
        let now = Utc::now();
        let score = hotness_score(0, &now, None);
        // sigmoid(log1p(0)) = sigmoid(0) = 0.5, recency = 1.0 → 0.5
        assert!((score - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_high_access_high_hotness() {
        let now = Utc::now();
        let score = hotness_score(100, &now, None);
        // sigmoid(log1p(100)) ≈ sigmoid(4.62) ≈ 0.99, recency = 1.0 → ~0.99
        assert!(score > 0.95);
    }

    #[test]
    fn test_old_access_decayed() {
        let thirty_days_ago = Utc::now() - Duration::days(30);
        let score = hotness_score(100, &thirty_days_ago, None);
        // recency = exp(-30/7) ≈ 0.014, so score should be much lower
        assert!(score < 0.05);
    }

    #[test]
    fn test_final_score_blending() {
        let now = Utc::now();
        let score = final_score(1.0, 0, &now);
        // importance=1.0 weighted 0.7, hotness=0.5 weighted 0.3
        // 0.7 * 1.0 + 0.3 * 0.5 = 0.85
        assert!((score - 0.85).abs() < 0.01);
    }

    #[test]
    fn test_final_score_zero_importance() {
        let now = Utc::now();
        let score = final_score(0.0, 100, &now);
        // importance=0.0 weighted 0.7, hotness≈0.99 weighted 0.3
        // 0.0 + 0.3 * 0.99 ≈ 0.297
        assert!(score > 0.25 && score < 0.35);
    }
}
