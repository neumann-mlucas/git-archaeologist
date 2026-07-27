use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum BucketSize {
    Commit,
    Day,
    Week,
    Month,
}

impl BucketSize {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "commit" => Some(Self::Commit),
            "day" => Some(Self::Day),
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            _ => None,
        }
    }
}

/// Auto-pick a bucket size based on total commit count.
pub fn auto(total_commits: usize) -> BucketSize {
    match total_commits {
        0..=499 => BucketSize::Commit,
        500..=4_999 => BucketSize::Day,
        5_000..=49_999 => BucketSize::Week,
        _ => BucketSize::Month,
    }
}

/// Compute the bucket key for a commit timestamp.
///
/// Returns an integer suitable for grouping/ordering:
/// - Commit: unix seconds (each commit its own bucket)
/// - Day:    YYYYMMDD
/// - Week:   YYYYWW (ISO week)
/// - Month:  YYYYMM
pub fn bucket_key(ts: OffsetDateTime, size: BucketSize) -> i64 {
    match size {
        BucketSize::Commit => ts.unix_timestamp(),
        BucketSize::Day => {
            let d = ts.date();
            (d.year() as i64) * 10_000
                + (u8::from(d.month()) as i64) * 100
                + (d.day() as i64)
        }
        BucketSize::Week => {
            // Use ISO year (not calendar year) so keys sort correctly across
            // Dec/Jan boundaries where the ISO week rolls into the next year.
            let (iso_year, iso_week, _) = ts.date().to_iso_week_date();
            (iso_year as i64) * 100 + (iso_week as i64)
        }
        BucketSize::Month => {
            (ts.year() as i64) * 100 + (u8::from(ts.month()) as i64)
        }
    }
}
