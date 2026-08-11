//! Local wall-clock snapshot, matching the vendor's own field encoding
//! exactly (see `scripts/vendor-source-excerpt.js`, `updateDeviceTime`).

use chrono::{Datelike, Local, Timelike};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockDateFields {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub year2digit: u8,
    pub weekday: u8, // Monday=1 .. Sunday=7, matching the vendor's `day() || 7`
    pub month: u8,   // 1-12
    pub date: u8,    // day of month
}

/// Maps chrono's `Weekday` to the vendor's convention (Monday=1..Sunday=7).
/// This IS the same as chrono's own `Weekday::number_from_monday()` -- using
/// that directly rather than `num_days_from_monday() + 1` (an earlier
/// version of this comment incorrectly claimed they differed; they don't,
/// verified explicitly below with a unit test per calendar day).
fn weekday_to_vendor_encoding(weekday: chrono::Weekday) -> u8 {
    weekday.number_from_monday() as u8
}

/// Snapshots local wall-clock time ONCE (the vendor's own code samples via
/// `Uf()` once before its send loop, not per repeat -- see PROTOCOL.md).
/// Callers should call this once and reuse the result for the whole
/// `set-time` transaction.
pub fn snapshot_local() -> ClockDateFields {
    let now = Local::now();
    ClockDateFields {
        hour: now.hour() as u8,
        minute: now.minute() as u8,
        second: now.second() as u8,
        year2digit: (now.year() % 100) as u8,
        weekday: weekday_to_vendor_encoding(now.weekday()),
        month: now.month() as u8,
        date: now.day() as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Weekday;

    #[test]
    fn monday_maps_to_1() {
        assert_eq!(weekday_to_vendor_encoding(Weekday::Mon), 1);
    }

    #[test]
    fn sunday_maps_to_7_not_0() {
        // The exact case the plan called out explicitly: chrono's own
        // Weekday enum numbers Sunday differently depending on which method
        // you call (num_days_from_sunday() vs num_days_from_monday()) --
        // this locks in the vendor's specific convention regardless.
        assert_eq!(weekday_to_vendor_encoding(Weekday::Sun), 7);
    }

    #[test]
    fn every_day_of_week_maps_1_through_7_with_no_zero() {
        let days = [
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
            Weekday::Sat,
            Weekday::Sun,
        ];
        let expected = [1u8, 2, 3, 4, 5, 6, 7];
        for (day, exp) in days.iter().zip(expected.iter()) {
            assert_eq!(
                weekday_to_vendor_encoding(*day),
                *exp,
                "{day:?} should map to {exp}"
            );
        }
    }

    #[test]
    fn snapshot_local_produces_fields_in_valid_ranges() {
        // Can't pin exact values (this genuinely reads the real clock), but
        // every field must be in its documented valid range -- this is the
        // one test in this module that exercises the real `Local::now()`
        // path rather than a fixed calendar date.
        let f = snapshot_local();
        assert!(f.hour <= 23);
        assert!(f.minute <= 59);
        assert!(f.second <= 59);
        assert!((1..=7).contains(&f.weekday));
        assert!((1..=12).contains(&f.month));
        assert!((1..=31).contains(&f.date));
    }
}
