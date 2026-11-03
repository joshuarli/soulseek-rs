//! A small wall-clock timestamp.
//!
//! Stored as Unix seconds. Serialized as RFC 3339 so state files written by
//! older builds still load, and displayed in local time.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde::de::{self, Visitor};
use std::fmt;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    unix: i64,
}

impl Timestamp {
    #[must_use]
    pub fn now() -> Self {
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| {
                i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
            });
        Self { unix }
    }

    #[must_use]
    pub const fn from_unix(unix: i64) -> Self {
        Self { unix }
    }

    #[must_use]
    pub const fn unix(self) -> i64 {
        self.unix
    }

    /// The local wall-clock hour and minute, as the chat panes show them.
    #[must_use]
    pub fn hhmm(self) -> String {
        local_hhmm(self.unix)
    }

    #[must_use]
    pub fn to_rfc3339(self) -> String {
        let days = self.unix.div_euclid(86_400);
        let seconds = self.unix.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        let hour = seconds / 3_600;
        let minute = (seconds % 3_600) / 60;
        let second = seconds % 60;
        format!(
            "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
        )
    }

    #[must_use]
    pub fn parse_rfc3339(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() < 20
            || bytes.get(4) != Some(&b'-')
            || bytes.get(7) != Some(&b'-')
            || !matches!(bytes.get(10), Some(b'T' | b't' | b' '))
            || bytes.get(13) != Some(&b':')
            || bytes.get(16) != Some(&b':')
        {
            return None;
        }
        let year = text.get(0..4)?.parse::<i64>().ok()?;
        let month = text.get(5..7)?.parse::<u32>().ok()?;
        let day = text.get(8..10)?.parse::<u32>().ok()?;
        let hour = text.get(11..13)?.parse::<i64>().ok()?;
        let minute = text.get(14..16)?.parse::<i64>().ok()?;
        let second = text.get(17..19)?.parse::<i64>().ok()?;
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || !(0..=23).contains(&hour)
            || !(0..=59).contains(&minute)
            || !(0..=60).contains(&second)
        {
            return None;
        }

        let mut index = 19;
        if bytes.get(index) == Some(&b'.') {
            index += 1;
            while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                index += 1;
            }
        }
        let offset = match bytes.get(index)? {
            b'Z' | b'z' => {
                index += 1;
                0
            }
            sign @ (b'+' | b'-') => {
                if bytes.get(index + 3) != Some(&b':') {
                    return None;
                }
                let hours = text.get(index + 1..index + 3)?.parse::<i64>().ok()?;
                let minutes =
                    text.get(index + 4..index + 6)?.parse::<i64>().ok()?;
                if !(0..=23).contains(&hours)
                    || !(0..=59).contains(&minutes)
                {
                    return None;
                }
                index += 6;
                let seconds = hours * 3_600 + minutes * 60;
                if *sign == b'+' { seconds } else { -seconds }
            }
            _ => return None,
        };
        if bytes[index..]
            .iter()
            .any(|byte| !byte.is_ascii_whitespace())
        {
            return None;
        }

        let days = days_from_civil(year, month, day);
        Some(Self {
            unix: days * 86_400 + hour * 3_600 + minute * 60 + second - offset,
        })
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

impl Serialize for Timestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_rfc3339())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct TimestampVisitor;

        impl Visitor<'_> for TimestampVisitor {
            type Value = Timestamp;

            fn expecting(
                &self,
                formatter: &mut fmt::Formatter<'_>,
            ) -> fmt::Result {
                formatter.write_str("a Unix timestamp or an RFC 3339 string")
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(Timestamp::from_unix(value))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                i64::try_from(value)
                    .map(Timestamp::from_unix)
                    .map_err(|_| E::custom("timestamp out of range"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Timestamp::parse_rfc3339(value)
                    .ok_or_else(|| E::custom("invalid RFC 3339 timestamp"))
            }
        }

        deserializer.deserialize_any(TimestampVisitor)
    }
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = i64::from(if month > 2 { month - 3 } else { month + 9 });
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era =
        year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

const fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524
            - day_of_era / 146_096)
            / 365;
    let year = year_of_era + era * 400;
    let day_of_year =
        day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month as u32, day as u32)
}

fn utc_hhmm(unix: i64) -> String {
    let seconds = unix.rem_euclid(86_400);
    format!("{:02}:{:02} ", seconds / 3_600, (seconds % 3_600) / 60)
}

#[cfg(unix)]
fn local_hhmm(unix: i64) -> String {
    let time = unix as libc::time_t;
    let mut broken_down = std::mem::MaybeUninit::<libc::tm>::zeroed();
    // SAFETY: localtime_r writes into the supplied tm and reads a time_t.
    let result = unsafe {
        libc::localtime_r(&raw const time, broken_down.as_mut_ptr())
    };
    if result.is_null() {
        return utc_hhmm(unix);
    }
    // SAFETY: localtime_r returned non-null, so it initialized the struct.
    let broken_down = unsafe { broken_down.assume_init() };
    format!("{:02}:{:02} ", broken_down.tm_hour, broken_down.tm_min)
}

#[cfg(windows)]
fn local_hhmm(unix: i64) -> String {
    windows::local_hhmm(unix).unwrap_or_else(|| utc_hhmm(unix))
}

#[cfg(not(any(unix, windows)))]
fn local_hhmm(unix: i64) -> String {
    utc_hhmm(unix)
}

#[cfg(windows)]
mod windows {
    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[repr(C)]
    struct WinSystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn FileTimeToLocalFileTime(
            file_time: *const FileTime,
            local_file_time: *mut FileTime,
        ) -> i32;
        fn FileTimeToSystemTime(
            file_time: *const FileTime,
            system_time: *mut WinSystemTime,
        ) -> i32;
    }

    pub fn local_hhmm(unix: i64) -> Option<String> {
        let intervals = unix
            .checked_add(11_644_473_600)?
            .checked_mul(10_000_000)?;
        let file_time = FileTime {
            low: intervals as u32,
            high: (intervals >> 32) as u32,
        };
        let mut local = FileTime { low: 0, high: 0 };
        let mut system = WinSystemTime {
            year: 0,
            month: 0,
            day_of_week: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            milliseconds: 0,
        };
        // SAFETY: both calls write into structs this function owns.
        unsafe {
            if FileTimeToLocalFileTime(&raw const file_time, &raw mut local) == 0
                || FileTimeToSystemTime(&raw const local, &raw mut system) == 0
            {
                return None;
            }
        }
        Some(format!("{:02}:{:02} ", system.hour, system.minute))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_seconds_round_trip_through_rfc3339() {
        for unix in [0, 1_700_000_000, -86_400, 2_000_000_000] {
            let timestamp = Timestamp::from_unix(unix);
            let text = timestamp.to_rfc3339();
            assert_eq!(
                Timestamp::parse_rfc3339(&text),
                Some(timestamp),
                "{text}"
            );
        }
    }

    #[test]
    fn an_old_local_offset_timestamp_is_understood() {
        assert_eq!(
            Timestamp::parse_rfc3339("1970-01-01T01:00:00+01:00"),
            Some(Timestamp::from_unix(0))
        );
        assert_eq!(
            Timestamp::parse_rfc3339("2023-11-14T22:13:20.123456789Z"),
            Some(Timestamp::from_unix(1_700_000_000))
        );
    }

    #[test]
    fn a_bad_timestamp_is_not_guessed_at() {
        assert_eq!(Timestamp::parse_rfc3339("not a time"), None);
        assert_eq!(Timestamp::parse_rfc3339("2023-13-01T00:00:00Z"), None);
    }
}
