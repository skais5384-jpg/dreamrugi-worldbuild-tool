use std::{cmp::Ordering, fmt, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScalarValueErrorCategory {
    InvalidDecimalSyntax,
    NonCanonicalDecimal,
    InvalidCalendarDate,
    InvalidLocalTime,
    InvalidDurationSyntax,
    DurationOutOfRange,
    NonCanonicalEmptyText,
    MultilineSingleLineText,
    InvalidNumberConstraintOrder,
}

/// 입력 본문을 소유하지 않는 scalar 오류다. 실패한 값은 Display, Debug, source chain에
/// 들어가지 않으므로 문서 내용이나 credential처럼 보이는 문자열도 진단으로 새지 않는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScalarValueError {
    category: ScalarValueErrorCategory,
    detail: &'static str,
}

impl ScalarValueError {
    pub(crate) const fn category(self) -> ScalarValueErrorCategory {
        self.category
    }

    const fn new(category: ScalarValueErrorCategory, detail: &'static str) -> Self {
        Self { category, detail }
    }

    const fn invalid_decimal_syntax() -> Self {
        Self::new(
            ScalarValueErrorCategory::InvalidDecimalSyntax,
            "decimal must use the canonical ASCII base-10 shape",
        )
    }

    const fn noncanonical_decimal() -> Self {
        Self::new(
            ScalarValueErrorCategory::NonCanonicalDecimal,
            "decimal has a noncanonical zero or digit representation",
        )
    }

    const fn invalid_calendar_date() -> Self {
        Self::new(
            ScalarValueErrorCategory::InvalidCalendarDate,
            "date must be an existing Gregorian day in YYYY-MM-DD form",
        )
    }

    const fn invalid_local_time() -> Self {
        Self::new(
            ScalarValueErrorCategory::InvalidLocalTime,
            "time must use 24-hour HH:mm:ss.SSS form",
        )
    }

    const fn invalid_duration_syntax() -> Self {
        Self::new(
            ScalarValueErrorCategory::InvalidDurationSyntax,
            "duration must use canonical signed integer milliseconds",
        )
    }

    const fn duration_out_of_range() -> Self {
        Self::new(
            ScalarValueErrorCategory::DurationOutOfRange,
            "duration milliseconds are outside the signed 64-bit range",
        )
    }

    const fn multiline_single_line_text() -> Self {
        Self::new(
            ScalarValueErrorCategory::MultilineSingleLineText,
            "single-line text must not contain hard line separators",
        )
    }

    const fn noncanonical_empty_text() -> Self {
        Self::new(
            ScalarValueErrorCategory::NonCanonicalEmptyText,
            "empty text is noncanonical and must use unset",
        )
    }

    const fn invalid_number_constraint_order() -> Self {
        Self::new(
            ScalarValueErrorCategory::InvalidNumberConstraintOrder,
            "number minimum must not exceed number maximum",
        )
    }
}

impl fmt::Display for ScalarValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "scalar value validation failed ({:?}): {}",
            self.category, self.detail
        )
    }
}

impl std::error::Error for ScalarValueError {}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct CanonicalDecimal {
    canonical: String,
}

impl CanonicalDecimal {
    pub(crate) fn parse(value: &str) -> Result<Self, ScalarValueError> {
        validate_decimal_syntax(value)?;
        Ok(Self {
            canonical: value.to_owned(),
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.canonical
    }

    pub(crate) fn is_negative(&self) -> bool {
        self.canonical.starts_with('-')
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.canonical == "0"
    }

    fn magnitude_cmp(&self, other: &Self) -> Ordering {
        let (left_integer, left_fraction) = decimal_parts(&self.canonical);
        let (right_integer, right_fraction) = decimal_parts(&other.canonical);

        left_integer
            .len()
            .cmp(&right_integer.len())
            .then_with(|| left_integer.cmp(right_integer))
            .then_with(|| compare_fractional_digits(left_fraction, right_fraction))
    }
}

impl fmt::Debug for CanonicalDecimal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalDecimal")
            .field(
                "sign",
                &if self.is_negative() {
                    "negative"
                } else {
                    "nonnegative"
                },
            )
            .field("is_zero", &self.is_zero())
            .finish_non_exhaustive()
    }
}

impl FromStr for CanonicalDecimal {
    type Err = ScalarValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl PartialOrd for CanonicalDecimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CanonicalDecimal {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.is_negative(), other.is_negative()) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (true, true) => self.magnitude_cmp(other).reverse(),
            (false, false) => self.magnitude_cmp(other),
        }
    }
}

fn validate_decimal_syntax(value: &str) -> Result<(), ScalarValueError> {
    let bytes = value.as_bytes();
    if bytes.is_empty() || !value.is_ascii() {
        return Err(ScalarValueError::invalid_decimal_syntax());
    }

    let (negative, unsigned) = match bytes.first() {
        Some(b'-') => (true, &bytes[1..]),
        Some(b'+') => return Err(ScalarValueError::invalid_decimal_syntax()),
        Some(_) => (false, bytes),
        None => return Err(ScalarValueError::invalid_decimal_syntax()),
    };
    if unsigned.is_empty() {
        return Err(ScalarValueError::invalid_decimal_syntax());
    }

    let mut decimal_point = None;
    for (index, byte) in unsigned.iter().copied().enumerate() {
        match byte {
            b'0'..=b'9' => {}
            b'.' if decimal_point.is_none() => decimal_point = Some(index),
            _ => return Err(ScalarValueError::invalid_decimal_syntax()),
        }
    }

    let integer_end = decimal_point.unwrap_or(unsigned.len());
    if integer_end == 0 || decimal_point == Some(unsigned.len() - 1) {
        return Err(ScalarValueError::invalid_decimal_syntax());
    }
    if unsigned[0] == b'0' && integer_end > 1 {
        return Err(ScalarValueError::noncanonical_decimal());
    }
    if decimal_point.is_some() && unsigned.last() == Some(&b'0') {
        return Err(ScalarValueError::noncanonical_decimal());
    }
    if negative && unsigned.iter().all(|byte| matches!(byte, b'0' | b'.')) {
        return Err(ScalarValueError::noncanonical_decimal());
    }
    Ok(())
}

fn decimal_parts(value: &str) -> (&[u8], &[u8]) {
    let unsigned = value.strip_prefix('-').unwrap_or(value).as_bytes();
    match unsigned.iter().position(|byte| *byte == b'.') {
        Some(index) => (&unsigned[..index], &unsigned[index + 1..]),
        None => (unsigned, &[]),
    }
}

fn compare_fractional_digits(left: &[u8], right: &[u8]) -> Ordering {
    let width = left.len().max(right.len());
    for index in 0..width {
        let left_digit = left.get(index).copied().unwrap_or(b'0');
        let right_digit = right.get(index).copied().unwrap_or(b'0');
        match left_digit.cmp(&right_digit) {
            Ordering::Equal => {}
            ordering => return ordering,
        }
    }
    Ordering::Equal
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct CalendarDate {
    year: u16,
    month: u8,
    day: u8,
    canonical: String,
}

impl CalendarDate {
    pub(crate) fn parse(value: &str) -> Result<Self, ScalarValueError> {
        let bytes = value.as_bytes();
        if bytes.len() != 10
            || bytes[4] != b'-'
            || bytes[7] != b'-'
            || !bytes
                .iter()
                .enumerate()
                .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
        {
            return Err(ScalarValueError::invalid_calendar_date());
        }

        let year = decimal_digits(&bytes[0..4]) as u16;
        let month = decimal_digits(&bytes[5..7]) as u8;
        let day = decimal_digits(&bytes[8..10]) as u8;
        if year == 0
            || !(1..=12).contains(&month)
            || !(1..=days_in_month(year, month)).contains(&day)
        {
            return Err(ScalarValueError::invalid_calendar_date());
        }

        Ok(Self {
            year,
            month,
            day,
            canonical: value.to_owned(),
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.canonical
    }

    pub(crate) const fn year(&self) -> u16 {
        self.year
    }

    pub(crate) const fn month(&self) -> u8 {
        self.month
    }

    pub(crate) const fn day(&self) -> u8 {
        self.day
    }
}

impl fmt::Debug for CalendarDate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CalendarDate")
            .field("year", &self.year)
            .field("month", &self.month)
            .field("day", &self.day)
            .finish()
    }
}

impl FromStr for CalendarDate {
    type Err = ScalarValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl PartialOrd for CalendarDate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CalendarDate {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.year, self.month, self.day).cmp(&(other.year, other.month, other.day))
    }
}

fn is_gregorian_leap_year(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_gregorian_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct LocalTime {
    hour: u8,
    minute: u8,
    second: u8,
    millisecond: u16,
    millisecond_of_day: u32,
    canonical: String,
}

impl LocalTime {
    pub(crate) fn parse(value: &str) -> Result<Self, ScalarValueError> {
        let bytes = value.as_bytes();
        if bytes.len() != 12
            || bytes[2] != b':'
            || bytes[5] != b':'
            || bytes[8] != b'.'
            || !bytes.iter().enumerate().all(|(index, byte)| {
                matches!(index, 2 | 5) && *byte == b':'
                    || index == 8 && *byte == b'.'
                    || !matches!(index, 2 | 5 | 8) && byte.is_ascii_digit()
            })
        {
            return Err(ScalarValueError::invalid_local_time());
        }

        let hour = decimal_digits(&bytes[0..2]) as u8;
        let minute = decimal_digits(&bytes[3..5]) as u8;
        let second = decimal_digits(&bytes[6..8]) as u8;
        let millisecond = decimal_digits(&bytes[9..12]) as u16;
        if hour > 23 || minute > 59 || second > 59 {
            return Err(ScalarValueError::invalid_local_time());
        }
        let millisecond_of_day =
            (((u32::from(hour) * 60 + u32::from(minute)) * 60 + u32::from(second)) * 1_000)
                + u32::from(millisecond);

        Ok(Self {
            hour,
            minute,
            second,
            millisecond,
            millisecond_of_day,
            canonical: value.to_owned(),
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.canonical
    }

    pub(crate) const fn hour(&self) -> u8 {
        self.hour
    }

    pub(crate) const fn minute(&self) -> u8 {
        self.minute
    }

    pub(crate) const fn second(&self) -> u8 {
        self.second
    }

    pub(crate) const fn millisecond(&self) -> u16 {
        self.millisecond
    }

    pub(crate) const fn millisecond_of_day(&self) -> u32 {
        self.millisecond_of_day
    }
}

impl fmt::Debug for LocalTime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalTime")
            .field("hour", &self.hour)
            .field("minute", &self.minute)
            .field("second", &self.second)
            .field("millisecond", &self.millisecond)
            .finish()
    }
}

impl FromStr for LocalTime {
    type Err = ScalarValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl PartialOrd for LocalTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LocalTime {
    fn cmp(&self, other: &Self) -> Ordering {
        self.millisecond_of_day.cmp(&other.millisecond_of_day)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct DurationMilliseconds {
    milliseconds: i64,
    canonical: String,
}

impl DurationMilliseconds {
    pub(crate) fn parse(value: &str) -> Result<Self, ScalarValueError> {
        let bytes = value.as_bytes();
        if bytes.is_empty() || !value.is_ascii() {
            return Err(ScalarValueError::invalid_duration_syntax());
        }

        let (negative, digits) = match bytes.first() {
            Some(b'-') => (true, &bytes[1..]),
            Some(b'+') => return Err(ScalarValueError::invalid_duration_syntax()),
            Some(_) => (false, bytes),
            None => return Err(ScalarValueError::invalid_duration_syntax()),
        };
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return Err(ScalarValueError::invalid_duration_syntax());
        }
        if (digits.len() > 1 && digits[0] == b'0') || (negative && digits == b"0") {
            return Err(ScalarValueError::invalid_duration_syntax());
        }

        let magnitude = digits.iter().try_fold(0_u64, |value, digit| {
            value
                .checked_mul(10)
                .and_then(|value| value.checked_add(u64::from(*digit - b'0')))
        });
        let magnitude = magnitude.ok_or_else(ScalarValueError::duration_out_of_range)?;
        let negative_limit = i64::MAX as u64 + 1;
        let milliseconds = if negative {
            if magnitude > negative_limit {
                return Err(ScalarValueError::duration_out_of_range());
            }
            if magnitude == negative_limit {
                i64::MIN
            } else {
                -(magnitude as i64)
            }
        } else {
            if magnitude > i64::MAX as u64 {
                return Err(ScalarValueError::duration_out_of_range());
            }
            magnitude as i64
        };

        Ok(Self {
            milliseconds,
            canonical: value.to_owned(),
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.canonical
    }

    pub(crate) const fn milliseconds(&self) -> i64 {
        self.milliseconds
    }
}

impl fmt::Debug for DurationMilliseconds {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DurationMilliseconds")
            .field("milliseconds", &self.milliseconds)
            .finish()
    }
}

impl FromStr for DurationMilliseconds {
    type Err = ScalarValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl PartialOrd for DurationMilliseconds {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DurationMilliseconds {
    fn cmp(&self, other: &Self) -> Ordering {
        self.milliseconds.cmp(&other.milliseconds)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct SingleLineText {
    value: String,
}

impl SingleLineText {
    pub(crate) fn parse(value: &str) -> Result<Self, ScalarValueError> {
        if value.is_empty() {
            return Err(ScalarValueError::noncanonical_empty_text());
        }
        if value.chars().any(is_hard_line_separator) {
            return Err(ScalarValueError::multiline_single_line_text());
        }
        Ok(Self {
            value: value.to_owned(),
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.value
    }
}

pub(crate) fn validate_optional_single_line_text(value: &str) -> Result<(), ScalarValueError> {
    if value.is_empty() {
        return Ok(());
    }
    SingleLineText::parse(value).map(|_| ())
}

impl fmt::Debug for SingleLineText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SingleLineText")
            .field("character_count", &self.value.chars().count())
            .finish()
    }
}

/// 플랫폼의 줄 분리 규칙에 기대지 않고 v1 single-line에서 금지할 문자 집합을 고정한다.
fn is_hard_line_separator(character: char) -> bool {
    matches!(
        character,
        '\u{000A}' | '\u{000B}' | '\u{000C}' | '\u{000D}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

impl FromStr for SingleLineText {
    type Err = ScalarValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// 현재 number wire configuration에는 min/max가 없지만, 두 canonical 경계가 생기는
/// configuration에서도 같은 exact ordering을 재사용할 수 있게 순서 규칙을 고정한다.
pub(crate) fn validate_number_constraint_order(
    minimum: Option<&CanonicalDecimal>,
    maximum: Option<&CanonicalDecimal>,
) -> Result<(), ScalarValueError> {
    if minimum.zip(maximum).is_some_and(|(min, max)| min > max) {
        return Err(ScalarValueError::invalid_number_constraint_order());
    }
    Ok(())
}

fn decimal_digits(digits: &[u8]) -> u32 {
    digits
        .iter()
        .fold(0, |value, digit| value * 10 + u32::from(*digit - b'0'))
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, error::Error};

    use super::*;

    fn assert_redacted(error: ScalarValueError, raw: &str) {
        assert!(!error.to_string().contains(raw));
        assert!(!format!("{error:?}").contains(raw));
        assert!(error.source().is_none());
    }

    #[test]
    fn decimal_accepts_the_v1_canonical_table_and_arbitrary_length() {
        let very_long_integer = "1234567890".repeat(1_000);
        let very_small = format!("0.{}1", "0".repeat(4_000));
        for value in [
            "0",
            "1",
            "-1",
            "0.5",
            "-0.5",
            "123456789012345678901234567890",
            "0.000000000000000000000000000001",
            &very_long_integer,
            &very_small,
        ] {
            let parsed = CanonicalDecimal::parse(value).expect("canonical decimal should parse");
            assert_eq!(parsed.as_str(), value);
        }
    }

    #[test]
    fn decimal_rejects_invalid_and_noncanonical_tables() {
        for value in [
            "", "+1", ".5", "1.", "1e3", "1E3", "NaN", "Infinity", " 1", "1 ", "１",
        ] {
            assert_eq!(
                CanonicalDecimal::parse(value)
                    .expect_err("syntax must fail")
                    .category(),
                ScalarValueErrorCategory::InvalidDecimalSyntax,
                "{value:?}"
            );
        }
        for value in ["01", "-01", "1.0", "1.2300", "-0", "-0.0"] {
            assert_eq!(
                CanonicalDecimal::parse(value)
                    .expect_err("noncanonical decimal must fail")
                    .category(),
                ScalarValueErrorCategory::NonCanonicalDecimal,
                "{value:?}"
            );
        }
    }

    #[test]
    fn decimal_orders_sign_integer_width_and_zero_padded_fractions_exactly() {
        let values = [
            "-100000000000000000000000000000000000000",
            "-1",
            "-0.11",
            "-0.101",
            "0",
            "0.001",
            "0.01",
            "0.1",
            "0.101",
            "0.11",
            "1",
            "100000000000000000000000000000000000000",
        ];
        let parsed: Vec<_> = values
            .iter()
            .map(|value| CanonicalDecimal::parse(value).expect("fixture is canonical"))
            .collect();
        assert!(parsed.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(CanonicalDecimal::parse("1.2").unwrap() > CanonicalDecimal::parse("1.19").unwrap());
        assert!(
            CanonicalDecimal::parse("-1.2").unwrap() < CanonicalDecimal::parse("-1.19").unwrap()
        );

        for (left, right) in [
            ("-10", "-2"),
            ("-1.1", "-1.01"),
            ("1.01", "1.1"),
            ("1.2", "1.21"),
            ("99999999999999999999", "100000000000000000000"),
        ] {
            let left = CanonicalDecimal::parse(left).unwrap();
            let right = CanonicalDecimal::parse(right).unwrap();
            assert!(left < right);
            assert!(right > left);
            assert_eq!(left.cmp(&right), Ordering::Less);
            assert_ne!(left, right);
        }
    }

    #[test]
    fn decimal_equality_hash_and_sign_zero_accessors_are_canonical() {
        let first = CanonicalDecimal::parse("123.45").unwrap();
        let second = CanonicalDecimal::parse("123.45").unwrap();
        let set = HashSet::from([first.clone(), second.clone()]);
        assert_eq!(first, second);
        assert_eq!(set.len(), 1);
        assert!(!first.is_negative());
        assert!(!first.is_zero());
        assert!(CanonicalDecimal::parse("-1").unwrap().is_negative());
        assert!(CanonicalDecimal::parse("0").unwrap().is_zero());
    }

    #[test]
    fn decimal_errors_do_not_echo_raw_values() {
        let raw = "credential=decimal-secret";
        assert_redacted(CanonicalDecimal::parse(raw).unwrap_err(), raw);
    }

    #[test]
    fn calendar_date_accepts_boundaries_and_gregorian_leap_days() {
        for value in ["0001-01-01", "2000-02-29", "2026-09-04", "9999-12-31"] {
            let parsed = CalendarDate::parse(value).expect("valid date should parse");
            assert_eq!(parsed.as_str(), value);
        }
        let date = CalendarDate::parse("2026-09-04").unwrap();
        assert_eq!((date.year(), date.month(), date.day()), (2026, 9, 4));
    }

    #[test]
    fn calendar_date_rejects_invalid_shapes_days_and_years() {
        for value in [
            "0000-01-01",
            "1900-02-29",
            "2026-02-29",
            "2026-13-01",
            "2026-04-31",
            "2026-9-4",
            "2026/09/04",
            "2026-09-04Z",
            " 2026-09-04",
            "２０２６-09-04",
        ] {
            assert_eq!(
                CalendarDate::parse(value).unwrap_err().category(),
                ScalarValueErrorCategory::InvalidCalendarDate,
                "{value:?}"
            );
        }
    }

    #[test]
    fn calendar_date_checks_each_month_and_orders_by_calendar() {
        for (month, last_day) in [
            (1, 31),
            (2, 28),
            (3, 31),
            (4, 30),
            (5, 31),
            (6, 30),
            (7, 31),
            (8, 31),
            (9, 30),
            (10, 31),
            (11, 30),
            (12, 31),
        ] {
            let valid = format!("2026-{month:02}-{last_day:02}");
            CalendarDate::parse(&valid).expect("last day should exist");
            let invalid = format!("2026-{month:02}-{:02}", last_day + 1);
            assert!(CalendarDate::parse(&invalid).is_err());
        }
        CalendarDate::parse("2024-02-29").expect("leap day should exist");
        assert!(
            CalendarDate::parse("2026-12-31").unwrap() < CalendarDate::parse("2027-01-01").unwrap()
        );
    }

    #[test]
    fn calendar_errors_do_not_echo_raw_values() {
        let raw = "credential=date-secret";
        assert_redacted(CalendarDate::parse(raw).unwrap_err(), raw);
    }

    #[test]
    fn local_time_accepts_fixed_millisecond_precision_and_orders_exactly() {
        for value in ["00:00:00.000", "09:30:00.000", "23:59:59.999"] {
            assert_eq!(LocalTime::parse(value).unwrap().as_str(), value);
        }
        let value = LocalTime::parse("09:30:00.001").unwrap();
        assert_eq!(
            (
                value.hour(),
                value.minute(),
                value.second(),
                value.millisecond()
            ),
            (9, 30, 0, 1)
        );
        assert_eq!(value.millisecond_of_day(), 34_200_001);
        assert!(LocalTime::parse("09:30:00.000").unwrap() < value);
    }

    #[test]
    fn local_time_rejects_abbreviations_ranges_offsets_and_unicode() {
        for value in [
            "9:30",
            "09:30",
            "09:30:00",
            "24:00:00.000",
            "23:60:00.000",
            "23:59:60.000",
            "09:30:00.000Z",
            "09:30:00.000+09:00",
            " 09:30:00.000",
            "０9:30:00.000",
        ] {
            assert_eq!(
                LocalTime::parse(value).unwrap_err().category(),
                ScalarValueErrorCategory::InvalidLocalTime,
                "{value:?}"
            );
        }
    }

    #[test]
    fn local_time_errors_do_not_echo_raw_values() {
        let raw = "credential=time-secret";
        assert_redacted(LocalTime::parse(raw).unwrap_err(), raw);
    }

    #[test]
    fn duration_accepts_i64_boundaries_and_orders_by_milliseconds() {
        for value in [
            "0",
            "1",
            "-1",
            "86400000",
            "9223372036854775807",
            "-9223372036854775808",
        ] {
            assert_eq!(DurationMilliseconds::parse(value).unwrap().as_str(), value);
        }
        assert_eq!(
            DurationMilliseconds::parse("9223372036854775807")
                .unwrap()
                .milliseconds(),
            i64::MAX
        );
        assert_eq!(
            DurationMilliseconds::parse("-9223372036854775808")
                .unwrap()
                .milliseconds(),
            i64::MIN
        );
        assert!(
            DurationMilliseconds::parse("-1").unwrap() < DurationMilliseconds::parse("0").unwrap()
        );
    }

    #[test]
    fn duration_separates_syntax_from_range_failures() {
        for value in [
            "", "+1", "01", "-0", "1.0", "1e3", "1000ms", " 1", "1 ", "１",
        ] {
            assert_eq!(
                DurationMilliseconds::parse(value).unwrap_err().category(),
                ScalarValueErrorCategory::InvalidDurationSyntax,
                "{value:?}"
            );
        }
        for value in [
            "9223372036854775808",
            "-9223372036854775809",
            "999999999999999999999999999999999",
        ] {
            assert_eq!(
                DurationMilliseconds::parse(value).unwrap_err().category(),
                ScalarValueErrorCategory::DurationOutOfRange,
                "{value:?}"
            );
        }
    }

    #[test]
    fn duration_errors_do_not_echo_raw_values() {
        let raw = "credential=duration-secret";
        assert_redacted(DurationMilliseconds::parse(raw).unwrap_err(), raw);
    }

    #[test]
    fn single_line_text_rejects_empty_but_preserves_whitespace_and_unicode() {
        let empty_error = SingleLineText::parse("").unwrap_err();
        assert_eq!(
            empty_error.category(),
            ScalarValueErrorCategory::NonCanonicalEmptyText
        );
        assert!(empty_error.source().is_none());

        for value in [" ", "  ", "  앞뒤 공백  ", "\t", "e\u{301}", "é", "한글🙂"] {
            let parsed =
                SingleLineText::parse(value).expect("single-line text should be preserved");
            assert_eq!(parsed.as_str(), value);
        }
    }

    #[test]
    fn single_line_text_rejects_every_hard_line_separator_without_echoing_content() {
        for separator in [
            '\u{000A}', '\u{000B}', '\u{000C}', '\u{000D}', '\u{0085}', '\u{2028}', '\u{2029}',
        ] {
            for value in [separator.to_string(), format!("a{separator}b")] {
                let error = SingleLineText::parse(&value).unwrap_err();
                assert_eq!(
                    error.category(),
                    ScalarValueErrorCategory::MultilineSingleLineText
                );
                assert_redacted(error, &value);
            }
        }
    }

    #[test]
    fn optional_single_line_text_accepts_empty_and_uses_the_complete_separator_contract() {
        for value in ["", " ", "\t", "  한글\t🙂  "] {
            validate_optional_single_line_text(value)
                .expect("optional single-line text should preserve an allowed value");
        }

        for separator in [
            '\u{000A}', '\u{000B}', '\u{000C}', '\u{000D}', '\u{0085}', '\u{2028}', '\u{2029}',
        ] {
            let value = format!("앞{separator}뒤");
            let error = validate_optional_single_line_text(&value)
                .expect_err("every hard line separator must be rejected");
            assert_eq!(
                error.category(),
                ScalarValueErrorCategory::MultilineSingleLineText
            );
            assert_redacted(error, &value);
        }
    }

    #[test]
    fn from_str_uses_the_same_strict_validation_as_parse_for_every_scalar_type() {
        macro_rules! assert_same_parse_contract {
            ($type:ty, $valid:expr, $invalid:expr) => {{
                let parsed = <$type>::parse($valid).expect("valid parse fixture should succeed");
                let from_str = $valid
                    .parse::<$type>()
                    .expect("valid FromStr fixture should succeed");
                assert_eq!(from_str, parsed);

                let parse_category = <$type>::parse($invalid)
                    .expect_err("invalid parse fixture should fail")
                    .category();
                let from_str_category = $invalid
                    .parse::<$type>()
                    .expect_err("invalid FromStr fixture should fail")
                    .category();
                assert_eq!(from_str_category, parse_category);
            }};
        }

        assert_same_parse_contract!(CanonicalDecimal, "123.45", "1e3");
        assert_same_parse_contract!(CalendarDate, "2024-02-29", "1900-02-29");
        assert_same_parse_contract!(LocalTime, "23:59:59.999", "24:00:00.000");
        assert_same_parse_contract!(DurationMilliseconds, "-1", "+1");
        assert_same_parse_contract!(SingleLineText, " 한글\t🙂 ", "");
    }

    #[test]
    fn number_constraint_order_uses_exact_decimal_ordering() {
        let minimum = CanonicalDecimal::parse("999999999999999999999999999999.01").unwrap();
        let maximum = CanonicalDecimal::parse("999999999999999999999999999999.1").unwrap();
        validate_number_constraint_order(Some(&minimum), Some(&maximum)).unwrap();
        validate_number_constraint_order(None, Some(&maximum)).unwrap();
        assert_eq!(
            validate_number_constraint_order(Some(&maximum), Some(&minimum))
                .unwrap_err()
                .category(),
            ScalarValueErrorCategory::InvalidNumberConstraintOrder
        );
    }
}
