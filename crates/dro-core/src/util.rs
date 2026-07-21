//! Small helpers.

use core::ops::RangeInclusive;

/// The sample rate a VGM file's sample counts are expressed in.
pub const VGM_SAMPLE_RATE: u32 = 44_100;

/// Converts a sample count to milliseconds, rounding half away from zero.
///
/// Equivalent to `floor((samples / (frequency / 1000)) + 0.5)`, but done in
/// exact integer arithmetic so native and wasm agree bit for bit:
/// `floor((2 * samples * 1000 + frequency) / (2 * frequency))`.
#[must_use]
pub fn smp_to_ms(samples: u32, frequency: u32) -> u32 {
    debug_assert!(frequency > 0, "frequency must be non-zero");
    let samples = u64::from(samples);
    let frequency = u64::from(frequency);
    let ms = (2 * samples * 1000 + frequency) / (2 * frequency);
    u32::try_from(ms).unwrap_or(u32::MAX)
}

/// Formats a millisecond count as `MM:SS`, truncating seconds.
///
/// Minutes are not clamped: a 100-minute song renders as `100:00`.
#[must_use]
pub fn ms_to_timestr(ms: u32) -> String {
    to_timestr(ms / 60_000, (ms % 60_000) / 1000)
}

/// Formats minutes and seconds as `MM:SS`.
#[must_use]
fn to_timestr(minutes: u32, seconds: u32) -> String {
    format!("{minutes:02}:{seconds:02}")
}

/// Groups a sorted, de-duplicated list of indices into contiguous inclusive ranges.
///
/// Deletion here is a single forward compaction pass, so ascending order is the
/// natural input and output.
///
/// # Panics
/// In debug builds, if `indices` is not strictly ascending.
#[must_use]
pub fn condense_ranges(indices: &[usize]) -> Vec<RangeInclusive<usize>> {
    debug_assert!(
        indices.windows(2).all(|w| w[0] < w[1]),
        "indices must be sorted ascending and de-duplicated"
    );

    let mut ranges: Vec<RangeInclusive<usize>> = Vec::new();
    for &index in indices {
        match ranges.last_mut() {
            Some(range) if *range.end() + 1 == index => *range = *range.start()..=index,
            _ => ranges.push(index..=index),
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smp_to_ms_rounds_half_away_from_zero() {
        // floor((samples / (44100 / 1000)) + 0.5)
        assert_eq!(smp_to_ms(0, VGM_SAMPLE_RATE), 0);
        assert_eq!(smp_to_ms(44_100, VGM_SAMPLE_RATE), 1000);
        assert_eq!(smp_to_ms(44, VGM_SAMPLE_RATE), 1); // 0.9977 -> 1
        assert_eq!(smp_to_ms(22, VGM_SAMPLE_RATE), 0); // 0.4989 -> 0
        assert_eq!(smp_to_ms(23, VGM_SAMPLE_RATE), 1); // 0.5215 -> 1
        assert_eq!(smp_to_ms(735, VGM_SAMPLE_RATE), 17); // 16.667 -> 17
    }

    #[test]
    fn smp_to_ms_matches_the_float_formula_over_a_wide_range() {
        for samples in (0..2_000_000).step_by(9_973) {
            let expected =
                ((f64::from(samples) / (f64::from(VGM_SAMPLE_RATE) / 1000.0)) + 0.5).floor() as u32;
            assert_eq!(
                smp_to_ms(samples, VGM_SAMPLE_RATE),
                expected,
                "samples={samples}"
            );
        }
    }

    #[test]
    fn timestr_formatting() {
        assert_eq!(ms_to_timestr(0), "00:00");
        assert_eq!(ms_to_timestr(999), "00:00");
        assert_eq!(ms_to_timestr(1000), "00:01");
        assert_eq!(ms_to_timestr(59_999), "00:59");
        assert_eq!(ms_to_timestr(60_000), "01:00");
        assert_eq!(ms_to_timestr(2683), "00:02");
        // Minutes are never truncated to two digits.
        assert_eq!(ms_to_timestr(100 * 60_000), "100:00");
    }

    #[test]
    fn condense_ranges_groups_runs() {
        assert_eq!(condense_ranges(&[]), vec![]);
        assert_eq!(condense_ranges(&[5]), vec![5..=5]);
        assert_eq!(condense_ranges(&[1, 3, 4, 6]), vec![1..=1, 3..=4, 6..=6]);
        assert_eq!(condense_ranges(&[0, 1, 2, 3]), vec![0..=3]);
        assert_eq!(condense_ranges(&[2, 4, 6]), vec![2..=2, 4..=4, 6..=6]);
    }

    #[test]
    fn condense_ranges_covers_every_input_index_exactly_once() {
        let indices = [0, 1, 2, 5, 9, 10, 11, 12, 20];
        let covered: Vec<usize> = condense_ranges(&indices).into_iter().flatten().collect();
        assert_eq!(covered, indices);
    }
}
