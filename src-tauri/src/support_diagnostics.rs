//! 선택 기능의 시작 실패를 사용자 콘텐츠 없이 지원 화면에 전달한다.
use serde::Serialize;
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SupportDiagnostic {
    pub(crate) feature: &'static str,
    pub(crate) stage: &'static str,
    pub(crate) category: &'static str,
    pub(crate) cause_id: Option<String>,
    pub(crate) observed_at_utc: Option<String>,
}

static DIAGNOSTICS: Mutex<Vec<SupportDiagnostic>> = Mutex::new(Vec::new());

pub(crate) fn record(
    feature: &'static str,
    stage: &'static str,
    category: &'static str,
    cause_id: Option<String>,
) -> bool {
    let diagnostic = SupportDiagnostic {
        feature,
        stage,
        category,
        cause_id,
        observed_at_utc: crate::data::utc_time::now_utc_milliseconds().ok(),
    };
    let mut diagnostics = DIAGNOSTICS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    diagnostics.retain(|item| item.feature != feature);
    diagnostics.push(diagnostic);
    true
}

pub(crate) fn clear(feature: &str) -> bool {
    let mut diagnostics = DIAGNOSTICS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let before = diagnostics.len();
    diagnostics.retain(|item| item.feature != feature);
    diagnostics.len() != before
}

pub(crate) fn snapshot() -> Vec<SupportDiagnostic> {
    DIAGNOSTICS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_diagnostic_exposes_only_bounded_identifiers() {
        const FEATURE: &str = "youtube_handler_test";
        clear(FEATURE);
        assert!(record(
            FEATURE,
            "request_filter",
            "webview2_com",
            Some("0x80070005".into()),
        ));
        let diagnostic = snapshot()
            .into_iter()
            .find(|item| item.feature == FEATURE)
            .expect("controlled failure projection");
        let encoded = serde_json::to_string(&diagnostic).unwrap();
        assert!(encoded.contains("request_filter"));
        assert!(encoded.contains("0x80070005"));
        assert!(encoded.contains("observedAtUtc"));
        assert!(!encoded.contains("C:\\"));
        assert!(!encoded.contains("https://"));
        assert!(clear(FEATURE));
    }
}
