use time::{macros::format_description, OffsetDateTime, PrimitiveDateTime};

const UTC_MILLISECONDS: &[time::format_description::FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");

/// 저장 규약에 맞는 UTC 시각을 로캘과 운영체제 시간대에 독립적으로 만든다.
pub fn now_utc_milliseconds() -> Result<String, time::error::Format> {
    OffsetDateTime::now_utc().format(UTC_MILLISECONDS)
}

/// 외부 journal의 시각이 로캘 독립적인 저장 규약과 정확히 일치하는지 확인한다.
pub fn is_utc_milliseconds(value: &str) -> bool {
    value.len() == 24
        && value.as_bytes().get(19) == Some(&b'.')
        && value.ends_with('Z')
        && PrimitiveDateTime::parse(value, UTC_MILLISECONDS).is_ok()
}
