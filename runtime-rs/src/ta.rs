// Copyright (c) 2026 OptionLab LLC. All rights reserved.

pub fn sma_series(values: &[f64], n: usize) -> Result<Vec<Option<f64>>, String> {
    if n == 0 {
        return Err("n must be > 0".to_string());
    }
    let mut out = Vec::with_capacity(values.len());
    let mut rolling_sum = 0.0;
    for (index, value) in values.iter().enumerate() {
        rolling_sum += value;
        if index >= n {
            rolling_sum -= values[index - n];
        }
        out.push(if index >= n - 1 {
            Some(rolling_sum / n as f64)
        } else {
            None
        });
    }
    Ok(out)
}

pub fn ema_series(values: &[f64], n: usize) -> Result<Vec<Option<f64>>, String> {
    if n == 0 {
        return Err("n must be > 0".to_string());
    }
    let mut out = Vec::with_capacity(values.len());
    let alpha = 2.0 / (n as f64 + 1.0);
    let mut ema_previous = None;
    let mut seed_sum = 0.0;
    for (index, value) in values.iter().copied().enumerate() {
        if index < n - 1 {
            out.push(None);
            seed_sum += value;
            continue;
        }
        if index == n - 1 {
            seed_sum += value;
            let seeded = seed_sum / n as f64;
            ema_previous = Some(seeded);
            out.push(Some(seeded));
            continue;
        }
        let next = alpha * value + (1.0 - alpha) * ema_previous.expect("seeded ema");
        ema_previous = Some(next);
        out.push(Some(next));
    }
    Ok(out)
}

pub fn rsi_series(values: &[f64], n: usize) -> Result<Vec<Option<f64>>, String> {
    if n == 0 {
        return Err("n must be > 0".to_string());
    }
    let mut out = Vec::with_capacity(values.len());
    let mut gains = vec![0.0];
    let mut losses = vec![0.0];
    for index in 1..values.len() {
        let change = values[index] - values[index - 1];
        gains.push(change.max(0.0));
        losses.push((-change).max(0.0));
    }

    let mut average_gain = None;
    let mut average_loss = None;
    for index in 0..values.len() {
        if index < n {
            out.push(None);
            continue;
        }
        if index == n {
            average_gain = Some(gains[1..n + 1].iter().sum::<f64>() / n as f64);
            average_loss = Some(losses[1..n + 1].iter().sum::<f64>() / n as f64);
        } else {
            average_gain = Some(
                (average_gain.expect("average gain") * (n - 1) as f64 + gains[index]) / n as f64,
            );
            average_loss = Some(
                (average_loss.expect("average loss") * (n - 1) as f64 + losses[index]) / n as f64,
            );
        }
        let average_gain = average_gain.expect("average gain initialized");
        let average_loss = average_loss.expect("average loss initialized");
        if average_loss == 0.0 {
            out.push(Some(100.0));
        } else {
            let rs = average_gain / average_loss;
            out.push(Some(100.0 - (100.0 / (1.0 + rs))));
        }
    }
    Ok(out)
}

pub fn cross_over(a: &[Option<f64>], b: &[Option<f64>]) -> bool {
    if a.len() < 2 || b.len() < 2 {
        return false;
    }
    let (previous_a, current_a) = (a[a.len() - 2], a[a.len() - 1]);
    let (previous_b, current_b) = (b[b.len() - 2], b[b.len() - 1]);
    match (previous_a, previous_b, current_a, current_b) {
        (Some(previous_a), Some(previous_b), Some(current_a), Some(current_b)) => {
            previous_a <= previous_b && current_a > current_b
        }
        _ => false,
    }
}

pub fn cross_under(a: &[Option<f64>], b: &[Option<f64>]) -> bool {
    if a.len() < 2 || b.len() < 2 {
        return false;
    }
    let (previous_a, current_a) = (a[a.len() - 2], a[a.len() - 1]);
    let (previous_b, current_b) = (b[b.len() - 2], b[b.len() - 1]);
    match (previous_a, previous_b, current_a, current_b) {
        (Some(previous_a), Some(previous_b), Some(current_a), Some(current_b)) => {
            previous_a >= previous_b && current_a < current_b
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{cross_over, cross_under, ema_series, rsi_series, sma_series};

    fn assert_series_close(actual: &[Option<f64>], expected: &[Option<f64>]) {
        assert_eq!(actual.len(), expected.len());
        for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
            match (actual, expected) {
                (Some(actual), Some(expected)) => {
                    assert!(
                        (actual - expected).abs() < 1e-9,
                        "index {index}: expected {expected}, got {actual}"
                    );
                }
                _ => assert_eq!(actual, expected, "index {index}"),
            }
        }
    }

    #[test]
    fn ta_indicator_series_match_python_source_shapes() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0];

        assert_series_close(
            &sma_series(&values, 3).expect("sma"),
            &[None, None, Some(2.0), Some(3.0), Some(4.0)],
        );
        assert_series_close(
            &ema_series(&values, 3).expect("ema"),
            &[None, None, Some(2.0), Some(3.0), Some(4.0)],
        );
        assert!(rsi_series(&[1.0, 2.0, 3.0, 2.0, 4.0], 2)
            .expect("rsi")
            .last()
            .and_then(|value| *value)
            .is_some_and(|value| value > 50.0));
        assert!(cross_over(&[Some(1.0), Some(2.0)], &[Some(2.0), Some(1.0)]));
        assert!(cross_under(
            &[Some(2.0), Some(1.0)],
            &[Some(1.0), Some(2.0)]
        ));
    }

    #[test]
    fn ta_indicator_series_reject_invalid_windows() {
        assert_eq!(
            sma_series(&[1.0, 2.0, 3.0], 0),
            Err("n must be > 0".to_string())
        );
        assert_eq!(
            ema_series(&[1.0, 2.0, 3.0], 0),
            Err("n must be > 0".to_string())
        );
        assert_eq!(
            rsi_series(&[1.0, 2.0, 3.0], 0),
            Err("n must be > 0".to_string())
        );
    }

    #[test]
    fn crosses_return_false_for_missing_or_short_series() {
        assert!(!cross_over(&[Some(1.0)], &[Some(1.0)]));
        assert!(!cross_under(&[Some(1.0)], &[Some(1.0)]));
        assert!(!cross_over(&[None, Some(2.0)], &[Some(1.0), Some(1.5)]));
        assert!(!cross_under(&[Some(2.0), Some(1.0)], &[Some(1.0), None]));
    }
}
