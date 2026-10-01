use semver::Version;
use serde_json::Value;
use url::Url;

pub(super) const REPOSITORY: &str = "skais5384-jpg/dreamrugi-worldbuild-tool";
pub(super) const TARGET: &str = "windows-x86_64";
pub(super) const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub(super) const MAX_BYTES: u64 = 512 * 1024 * 1024;

pub(super) fn distribution(channel: &str, mode: &str, packaged: Option<bool>) -> &'static str {
    match (channel, mode, packaged) {
        ("store", _, _) | (_, _, Some(true)) => "store",
        ("github", "release", Some(false)) => "github",
        _ => "dev",
    }
}
pub(super) fn release_url(version: &str) -> Result<String, &'static str> {
    let version = Version::parse(version).map_err(|_| "target")?;
    Ok(format!(
        "https://github.com/{REPOSITORY}/releases/tag/v{version}"
    ))
}
pub(super) fn secure(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.fragment().is_none()
}
pub(super) fn download_url(url: &Url, version: &Version) -> bool {
    let prefix = format!("/{REPOSITORY}/releases/download/v{version}/");
    secure(url)
        && url.host_str() == Some("github.com")
        && url.query().is_none()
        && url.path().starts_with(&prefix)
        && url.path().ends_with("_x64-setup.exe")
        && !url.path()[prefix.len()..].contains('/')
}
pub(super) fn redirect(url: &Url) -> bool {
    secure(url)
        && matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
                    | "raw.githubusercontent.com"
            )
        )
}

pub(super) fn metadata(
    current: &Version,
    version: &str,
    url: &Url,
    raw: &Value,
    testing: bool,
) -> Result<bool, &'static str> {
    let next = Version::parse(version).map_err(|_| "metadata")?;
    let declared = raw["channel"].as_str().ok_or("metadata")?;
    let prerelease = raw["prerelease"].as_bool().ok_or("metadata")?;
    if raw["repository"].as_str() != Some(REPOSITORY)
        || !download_url(url, &next)
        || declared != if testing { "test" } else { "stable" }
        || prerelease != testing
        || (!testing && (next.major < 1 || !next.pre.is_empty()))
        || raw["notes"]
            .as_str()
            .is_some_and(|s| s.len() > 64 * 1024 || s.contains('\0'))
        || raw["platforms"]
            .as_object()
            .is_none_or(|p| p.len() != 1 || !p.contains_key(TARGET))
    {
        return Err("target");
    }
    // build metadata는 precedence를 올리지 않는다. 동일·과거 후보를 설치할 수 없다.
    Ok(next.cmp_precedence(current).is_gt())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn data(version: &str) -> (Url, Value) {
        (Url::parse(&format!("https://github.com/{REPOSITORY}/releases/download/v{version}/Dreamrugi.Worldbuild.Tool_{version}_x64-setup.exe")).unwrap(),
         json!({"repository":REPOSITORY,"channel":"stable","prerelease":false,"platforms":{TARGET:{"signature":"signed"}},"notes":"긴 한글 변경 내용"}))
    }
    #[test]
    fn channel_and_native_package_fail_closed() {
        assert_eq!(distribution("github", "release", Some(false)), "github");
        for packaged in [Some(true), None] {
            assert_ne!(distribution("github", "release", packaged), "github");
        }
        assert_eq!(distribution("store", "test", Some(false)), "store");
        assert_eq!(distribution("github", "test", Some(false)), "dev");
    }
    #[test]
    fn stable_only_and_semver_precedence() {
        for (current, next, expected) in [
            ("0.2.0", "1.0.0", Ok(true)),
            ("1.0.0", "1.1.0-beta.1", Err("target")),
            ("0.2.0", "0.2.0", Err("target")),
            ("0.2.0", "0.1.0", Err("target")),
            ("1.0.0", "1.0.0+new", Ok(false)),
        ] {
            let (url, raw) = data(next);
            assert_eq!(
                metadata(&Version::parse(current).unwrap(), next, &url, &raw, false),
                expected
            );
        }
    }
    #[test]
    fn rejects_wrong_host_target_and_prerelease() {
        let (mut url, mut raw) = data("1.0.0");
        let current = Version::parse("0.2.0").unwrap();
        assert!(metadata(&current, "1.0.0", &url, &raw, false).unwrap());
        raw["prerelease"] = json!(true);
        assert!(metadata(&current, "1.0.0", &url, &raw, false).is_err());
        raw["prerelease"] = json!(false);
        url.set_host(Some("evil.example")).unwrap();
        assert!(metadata(&current, "1.0.0", &url, &raw, false).is_err());
        for value in [
            "http://github.com/x",
            "https://github.com.evil.example/x",
            "https://user@github.com/x",
            "https://github.com:444/x",
        ] {
            assert!(!redirect(&Url::parse(value).unwrap()));
        }
        assert!(redirect(
            &Url::parse("https://release-assets.githubusercontent.com/x?token=temporary").unwrap()
        ));
    }
    #[test]
    fn isolated_test_policy_is_not_operating_stable() {
        let current = Version::parse("0.2.0").unwrap();
        let (url, mut raw) = data("0.3.0");
        raw["channel"] = json!("test");
        raw["prerelease"] = json!(true);
        assert_eq!(metadata(&current, "0.3.0", &url, &raw, true), Ok(true));
        assert!(metadata(&current, "0.3.0", &url, &raw, false).is_err());
    }
}
