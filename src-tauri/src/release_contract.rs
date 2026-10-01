pub(crate) fn channel_label(channel: &str, mode: &str) -> &'static str {
    match (channel, mode) {
        ("github", "test") => "GitHub 로컬 시험본",
        ("github", "release") => "GitHub 배포판",
        ("store", "test") => "Store 로컬 시험본",
        ("store", "release") => "Microsoft Store 배포판",
        _ => "개발 빌드",
    }
}

fn version(value: &str) -> Option<[u32; 4]> {
    let parts = value
        .split('.')
        .map(|p| {
            if p.is_empty()
                || !p.bytes().all(|c| c.is_ascii_digit())
                || (p.len() > 1 && p.starts_with('0'))
            {
                None
            } else {
                p.parse::<u32>().ok().filter(|v| *v <= 65535)
            }
        })
        .collect::<Option<Vec<_>>>()?;
    let result: [u32; 4] = parts.try_into().ok()?;
    (result[0] > 0).then_some(result)
}

pub(crate) fn validate_store_release(raw: &str, app_version: &str) -> Result<(), &'static str> {
    let identity: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| "Store identity JSON")?;
    if identity["source"] != "PartnerCenter"
        || identity["verified"] != true
        || identity["historyVerified"] != true
    {
        return Err("verified Partner Center identity/history required");
    }
    for field in [
        "name",
        "publisher",
        "publisherDisplayName",
        "storeId",
        "checkedUtc",
    ] {
        let value = identity[field]
            .as_str()
            .ok_or("Store identity field missing")?;
        let lower = value.to_ascii_lowercase();
        if value.is_empty()
            || value.trim() != value
            || value.contains(['\r', '\n', '\0'])
            || ["localtest", "local test", "placeholder", "{{", "example"]
                .iter()
                .any(|term| lower.contains(term))
        {
            return Err("Store identity field invalid/test-only");
        }
    }
    let name = identity["name"].as_str().unwrap();
    let id = identity["storeId"].as_str().unwrap();
    if !(3..=50).contains(&name.len())
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
        || !identity["publisher"].as_str().unwrap().starts_with("CN=")
        || id.len() != 12
        || !id
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return Err("Store identity format invalid");
    }
    let expected = format!("{app_version}.0");
    version(&expected).ok_or("formal app version required")?;
    let candidate = identity["packageVersion"]
        .as_str()
        .ok_or("package version missing")?;
    let current = version(candidate)
        .filter(|v| v[3] == 0)
        .ok_or("package version invalid")?;
    match identity["noPublishedPackages"].as_bool() {
        Some(true) if identity["latestPackageVersion"].is_null() && candidate == expected => Ok(()),
        Some(false) => {
            let prior = identity["latestPackageVersion"]
                .as_str()
                .and_then(version)
                .ok_or("same-family history missing")?;
            (current > prior)
                .then_some(())
                .ok_or("package version must exceed same-family history")
        }
        _ => Err("Store family history/version mismatch"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> serde_json::Value {
        serde_json::json!({"source":"PartnerCenter","verified":true,"historyVerified":true,
            "name":"Synthetic.Product","publisher":"CN=Synthetic","publisherDisplayName":"Synthetic",
            "storeId":"9SYNTHETIC01","checkedUtc":"2026-09-30T00:00:00Z",
            "packageVersion":"1.0.0.0","noPublishedPackages":true,"latestPackageVersion":null})
    }

    #[test]
    fn labels_distinguish_channels_and_modes() {
        assert_eq!(channel_label("store", "release"), "Microsoft Store 배포판");
        assert_eq!(channel_label("store", "test"), "Store 로컬 시험본");
        assert_eq!(channel_label("github", "release"), "GitHub 배포판");
        assert_eq!(channel_label("dev", "dev"), "개발 빌드");
    }

    #[test]
    fn identity_missing_or_test_only_is_rejected() {
        assert!(validate_store_release("{}", "1.0.0").is_err());
        let mut i = identity();
        assert!(validate_store_release(&i.to_string(), "1.0.0").is_ok());
        i["name"] = "Dreamrugi.WorldbuildTool.ELocalTest".into();
        assert!(validate_store_release(&i.to_string(), "1.0.0").is_err());
    }

    #[test]
    fn same_family_version_cannot_regress() {
        let mut i = identity();
        i["noPublishedPackages"] = false.into();
        i["latestPackageVersion"] = "2.0.0.5".into();
        assert!(validate_store_release(&i.to_string(), "1.0.0").is_err());
        i["packageVersion"] = "2.0.1.0".into();
        assert!(validate_store_release(&i.to_string(), "1.0.0").is_ok());
        i["packageVersion"] = "2.0.1.1".into();
        assert!(validate_store_release(&i.to_string(), "1.0.0").is_err());
    }
}
