//! 외부 주소와 자산 식별자는 저장값과 실행 권한을 분리해 검증한다.
pub(crate) fn valid_id(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|id| !id.is_nil() && id.to_string() == s)
}
/// 직접 미디어만 허용한다. iframe/원격 script/native 권한은 제공하지 않는다.
pub(crate) fn media_url(raw: &str) -> Result<&'static str, ()> {
    if raw.len() > 4096 || raw.trim() != raw || raw.chars().any(|c| c.is_control() || c == '\\') {
        return Err(());
    }
    let url = tauri::Url::parse(raw).map_err(|_| ())?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return Err(());
    }
    let host = url.host_str().ok_or(())?;
    if !host.contains('.')
        || host.ends_with('.')
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.parse::<std::net::IpAddr>().is_ok()
        || host.starts_with('[')
    {
        return Err(());
    }
    if youtube_video(&url).is_some() {
        return Ok("youtube");
    }
    let path = url.path().to_ascii_lowercase();
    if [
        ".png", ".apng", ".jpg", ".jpeg", ".jpe", ".jfif", ".webp", ".gif", ".bmp",
    ]
    .iter()
    .any(|e| path.ends_with(e))
    {
        Ok("image")
    } else if [".mp4", ".webm"].iter().any(|e| path.ends_with(e)) {
        Ok("video")
    } else {
        Err(())
    }
}

fn youtube_video(url: &tauri::Url) -> Option<(String, Option<u32>)> {
    let host = url.host_str()?.to_ascii_lowercase();
    let parts = url
        .path()
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let id = if host == "youtu.be" {
        match parts.as_slice() {
            [id] => *id,
            _ => return None,
        }
    } else if matches!(
        host.as_str(),
        "youtube.com" | "www.youtube.com" | "m.youtube.com"
    ) {
        match parts.as_slice() {
            ["watch"] => {
                let values = url
                    .query_pairs()
                    .filter(|(key, _)| key == "v")
                    .map(|(_, value)| value.into_owned())
                    .collect::<Vec<_>>();
                if values.len() != 1 {
                    return None;
                }
                return youtube_id(&values[0])
                    .then(|| youtube_start(url).map(|start| (values[0].clone(), start)))?;
            }
            ["shorts", id] | ["embed", id] => id,
            _ => return None,
        }
    } else {
        return None;
    };
    youtube_id(id).then(|| youtube_start(url).map(|start| (id.to_owned(), start)))?
}

fn youtube_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn youtube_start(url: &tauri::Url) -> Option<Option<u32>> {
    let values = url
        .query_pairs()
        .filter(|(key, _)| key == "t" || key == "start")
        .map(|(_, value)| value.into_owned())
        .collect::<Vec<_>>();
    match values.as_slice() {
        [] => Some(None),
        [value] => parse_youtube_time(value).map(Some),
        _ => None,
    }
}

fn parse_youtube_time(raw: &str) -> Option<u32> {
    if !raw.is_empty() && raw.bytes().all(|byte| byte.is_ascii_digit()) {
        let seconds = raw.parse::<u32>().ok()?;
        return (seconds <= 86_400).then_some(seconds);
    }
    let bytes = raw.as_bytes();
    let mut index = 0;
    let mut total = 0_u32;
    let mut previous = 0_u8;
    while index < bytes.len() {
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if start == index || index >= bytes.len() {
            return None;
        }
        let value = raw[start..index].parse::<u32>().ok()?;
        let unit = bytes[index];
        index += 1;
        let rank = match unit {
            b'h' => 1,
            b'm' => 2,
            b's' => 3,
            _ => return None,
        };
        if rank <= previous {
            return None;
        }
        previous = rank;
        total = total.checked_add(match unit {
            b'h' => value.checked_mul(3600)?,
            b'm' => value.checked_mul(60)?,
            b's' => value,
            _ => unreachable!(),
        })?;
    }
    (previous != 0 && total <= 86_400).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_media_boundary_rejects_active_local_and_credential_urls() {
        for raw in [
            "http://example.com/a.mp4",
            "file:///C:/a.png",
            "javascript:alert(1)",
            "https://name:secret@example.com/a.mp4",
            "https://127.0.0.1/a.png",
            "https://[::1]/a.png",
            "https://2130706433/a.png",
            "https://host.local/a.png",
            "https://host.localhost/a.png",
            "https://localhost/a.png",
            "https://example.com:8443/a.mp4",
            "https://example.com/a.svg",
            "https://youtube.com/watch?v=abc",
            "https://youtu.be",
            "https://youtu.be/",
            "https://youtu.be///?t=5",
            "https://youtu.be/invalid",
            "https://youtu.be/dQw4w9WgXcQ?t=%2B5",
            "https://youtu.be/dQw4w9WgXcQ?t=86401",
            "https://example.com/a.png#",
            "https://youtube.example/watch?v=dQw4w9WgXcQ",
            "https://youtube.com.example/watch?v=dQw4w9WgXcQ",
            "https://youtube.com/watch?v=dQw4w9WgXcQ&v=aqz-KE-bpKQ",
            "https://youtube.com/watch?v=dQw4w9WgXcQ&t=forever",
            "https://example.com/a.png#fragment",
            "https://example.com/a.png\n",
            "https://example.com\\a.png",
        ] {
            assert!(media_url(raw).is_err(), "{raw}");
        }
        assert!(media_url(&format!("https://example.com/a.mp4?q={}", "x".repeat(4096))).is_err());
        assert_eq!(media_url("https://example.com/a.PNG?q=1"), Ok("image"));
        assert_eq!(media_url("https://example.com/a.mp4"), Ok("video"));
        assert_eq!(media_url("https://example.com/a.webm"), Ok("video"));
        assert_eq!(
            media_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1m30s"),
            Ok("youtube")
        );
        assert_eq!(
            media_url("https://youtu.be/dQw4w9WgXcQ?si=tracking"),
            Ok("youtube")
        );
        assert_eq!(
            media_url("https://youtube.com/shorts/dQw4w9WgXcQ?start=90"),
            Ok("youtube")
        );
        assert_eq!(
            media_url("https://m.youtube.com/embed/dQw4w9WgXcQ"),
            Ok("youtube")
        );
        for extension in ["gif", "apng", "bmp", "jfif"] {
            assert_eq!(
                media_url(&format!("https://example.com/image.{extension}")),
                Ok("image")
            );
        }
    }
}
