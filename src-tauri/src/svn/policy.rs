//! Shared compatibility policy. It is deliberately outside the document codec.
use super::*;
use semver::Version;

pub(crate) const FILE: &str = "worldbuild-policy.json";
const POLICY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Policy {
    pub(crate) format_version: u32,
    pub(crate) repository: String,
    pub(crate) repository_id: String,
    pub(crate) project: String,
    pub(crate) minimum_app_version: String,
    pub(crate) support_floor: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub(crate) current_app_version: String,
    pub(crate) server: Option<Policy>,
    pub(crate) local: Option<Policy>,
    pub(crate) local_modified: Option<bool>,
    pub(crate) revision: String,
    pub(crate) issue: Option<String>,
    pub(crate) available_versions: Vec<String>,
}
fn available(version: &Version) -> Vec<String> {
    if (version >= &Version::new(1, 0, 0) && version.pre.is_empty())
        || (cfg!(feature = "compatibility-test")
            && version >= &Version::new(0, 2, 0)
            && version.pre.is_empty())
    {
        vec![version.to_string()]
    } else {
        Vec::new()
    }
}
impl Policy {
    fn new(info: &Inspection, minimum: &str, version: &Version) -> Result<Self, String> {
        if !available(version).iter().any(|v| v == minimum) {
            return Err("svn_policy_version_unavailable".into());
        }
        Ok(Self {
            format_version: 1,
            repository: info.repository.clone(),
            repository_id: info.repository_id.clone(),
            project: info.url.clone(),
            minimum_app_version: minimum.into(),
            support_floor: if cfg!(feature = "compatibility-test") {
                "0.2.0"
            } else {
                "1.0.0"
            }
            .into(),
        })
    }
    fn decode(bytes: &[u8], info: &Inspection) -> Result<Self, String> {
        if bytes.len() > 16 * 1024 {
            return Err("svn_policy_invalid".into());
        }
        let policy: Self = serde_json::from_slice(bytes).map_err(|_| "svn_policy_invalid")?;
        if policy.format_version != 1 {
            return Err("svn_policy_format_unsupported".into());
        }
        if policy.repository != info.repository
            || policy.repository_id != info.repository_id
            || policy.project != info.url
        {
            return Err("svn_policy_scope_mismatch".into());
        }
        uuid::Uuid::parse_str(&policy.repository_id).map_err(|_| "svn_policy_invalid")?;
        let minimum =
            Version::parse(&policy.minimum_app_version).map_err(|_| "svn_policy_invalid")?;
        let floor = Version::parse(&policy.support_floor).map_err(|_| "svn_policy_invalid")?;
        if !minimum.pre.is_empty()
            || !minimum.build.is_empty()
            || !floor.pre.is_empty()
            || !floor.build.is_empty()
            || minimum < floor
            || floor
                != if cfg!(feature = "compatibility-test") {
                    Version::new(0, 2, 0)
                } else {
                    Version::new(1, 0, 0)
                }
        {
            return Err("svn_policy_invalid".into());
        }
        Ok(policy)
    }
    fn admit(&self, version: &Version) -> Result<(), String> {
        let minimum =
            Version::parse(&self.minimum_app_version).map_err(|_| "svn_policy_invalid")?;
        if version < &minimum {
            Err(format!(
                "svn_policy_app_too_old:{}",
                self.minimum_app_version
            ))
        } else {
            Ok(())
        }
    }
}
fn remote(
    manager: &Manager,
    identity: Option<&Identity>,
    command: &str,
    options: &[&str],
    url: &str,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let args = args_with_auth(manager, identity, command, options, url);
    let bytes = run_bytes_timeout(
        &manager.cli()?,
        &args,
        identity.and_then(|i| i.password.as_deref()),
        cancel,
        POLICY_TIMEOUT,
        None,
    )?;
    String::from_utf8(bytes).map_err(|_| "svn_policy_invalid".into())
}
fn has_entry(xml: &str, name: &str) -> Result<bool, String> {
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event().map_err(|_| "svn_xml_invalid")? {
            Event::Start(e) if e.name() == QName(b"name") => {
                if read_xml_text(&mut reader, b"name")? == name {
                    return Ok(true);
                }
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
    }
}
fn marker(manager: &Manager, info: &Inspection) -> PathBuf {
    manager
        .inner
        .config_root
        .join("known-policies")
        .join(format!(
            "{:x}",
            Sha256::digest(
                format!("{}\0{}\0{}", info.repository, info.repository_id, info.url).as_bytes()
            )
        ))
}
fn read_head(
    manager: &Manager,
    info: &Inspection,
    cancel: &AtomicBool,
) -> Result<(Option<Policy>, String), String> {
    let identity = manager.identity();
    let url = Url::parse(&info.url).map_err(|_| "svn_invalid_url")?;
    if !matches!(url.scheme(), "https" | "file")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("svn_invalid_url".into());
    }
    if url.scheme() == "https" {
        secure_url(&info.url)?;
    }
    if identity.as_ref().is_some_and(|i| origin(&url) != i.origin) {
        return Err("svn_other_server".into());
    }
    // Pin a server HEAD revision, then list/cat at that revision. No BASE/WC
    // bytes authorize access. Successful parent listing distinguishes absence.
    let head = remote(
        manager,
        identity.as_ref(),
        "info",
        &["--xml", "-r", "HEAD"],
        &info.url,
        cancel,
    )?;
    let remote_info = parse_remote_info(&head)?;
    if remote_info.repository_id != info.repository_id
        || remote_info.repository != info.repository
        || remote_info.url != info.url
    {
        return Err("svn_policy_scope_mismatch".into());
    }
    let revision = head_revision(&head)?;
    let list = remote(
        manager,
        identity.as_ref(),
        "list",
        &["--xml", "-r", &revision],
        &info.url,
        cancel,
    )?;
    if !has_entry(&list, FILE)? {
        if marker(manager, info).exists() {
            return Err("svn_policy_deleted".into());
        }
        // Revision zero is an empty repository with no change history.
        if revision == "0" {
            return Ok((None, revision));
        }
        let history = remote(
            manager,
            identity.as_ref(),
            "log",
            &["--xml", "--verbose", "-r", &format!("{revision}:1")],
            &info.url,
            cancel,
        )?;
        let expected = decode_path(&format!(
            "{}/{}",
            info.url
                .strip_prefix(&info.repository)
                .ok_or("svn_policy_scope_mismatch")?
                .trim_end_matches('/'),
            FILE
        ))?;
        if history_has_path(&history, &expected)? {
            return Err("svn_policy_deleted".into());
        }
        return Ok((None, revision));
    }
    let bytes = remote(
        manager,
        identity.as_ref(),
        "cat",
        &["-r", &revision],
        &format!("{}/{}", info.url.trim_end_matches('/'), FILE),
        cancel,
    )?;
    let policy = Policy::decode(bytes.as_bytes(), info)?;
    let known = marker(manager, info);
    fs::create_dir_all(known.parent().ok_or("svn_config_failed")?)
        .map_err(|_| "svn_config_failed")?;
    fs::write(known, revision.as_bytes()).map_err(|_| "svn_config_failed")?;
    if cancel.load(Ordering::Acquire) {
        return Err("svn_cancelled".into());
    }
    Ok((Some(policy), revision))
}
fn head_revision(xml: &str) -> Result<String, String> {
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event().map_err(|_| "svn_xml_invalid")? {
            Event::Start(e) if e.name() == QName(b"entry") => {
                for a in e.attributes() {
                    let a = a.map_err(|_| "svn_xml_invalid")?;
                    if a.key == QName(b"revision") {
                        let v = a
                            .normalized_value(XmlVersion::Explicit1_0)
                            .map_err(|_| "svn_xml_invalid")?
                            .into_owned();
                        if v.parse::<u64>().is_ok() {
                            return Ok(v);
                        }
                    }
                }
            }
            Event::Eof => return Err("svn_xml_invalid".into()),
            _ => {}
        }
    }
}
fn history_has_path(xml: &str, expected: &str) -> Result<bool, String> {
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event().map_err(|_| "svn_xml_invalid")? {
            Event::Start(e) if e.name() == QName(b"path") => {
                if read_xml_text(&mut reader, b"path")? == expected {
                    return Ok(true);
                }
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
    }
}
impl SvnLockService {
    pub(crate) fn policy_snapshot(&self) -> Result<Snapshot, String> {
        self.with_operation(|manager, cancel| {
            let info = inspect(manager, &self.project, cancel)?;
            let (server, revision) = read_head(manager, &info, cancel)?;
            let file = self.project.join(FILE);
            let local = match fs::read(&file) {
                Ok(bytes) => Some(Policy::decode(&bytes, &info)?),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => return Err("svn_policy_local_unreadable".into()),
            };
            // BASE is used only to describe the user's local edit. Admission
            // continues to use the pinned server HEAD above.
            let local_modified = local.as_ref().and_then(|local| {
                self.run_for(
                    manager,
                    "cat",
                    &["-r", "BASE"],
                    &self.project.join(FILE),
                    cancel,
                )
                .ok()
                .and_then(|base| Policy::decode(base.as_bytes(), &info).ok())
                .map(|base| local != &base)
            });
            let issue = server.as_ref().map_or_else(
                || Some("svn_policy_missing".into()),
                |p| p.admit(&manager.inner.app_version).err(),
            );
            Ok(Snapshot {
                current_app_version: manager.inner.app_version.to_string(),
                server,
                local,
                local_modified,
                revision,
                issue,
                available_versions: available(&manager.inner.app_version),
            })
        })
    }
    pub(crate) fn policy_admit(&self) -> Result<(), String> {
        let scope = SVN_JOB_SCOPE.get();
        let identity = self.manager.identity();
        let owner = identity
            .as_ref()
            .map(|i| {
                format!(
                    "{:x}",
                    Sha256::digest(
                        format!("{}\0{}\0{}\0{:?}", i.origin, i.url, i.username, i.config)
                            .as_bytes()
                    )
                )
            })
            .unwrap_or_default();
        if scope != 0 {
            if let Some((cached_scope, cached_owner, result)) = self
                .policy_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                if *cached_scope == scope && cached_owner == &owner {
                    return result.clone();
                }
            }
        }
        let result =
            self.with_operation(|manager, cancel| self.policy_admit_inner(manager, cancel));
        if scope != 0 {
            *self.policy_cache.lock().unwrap_or_else(|e| e.into_inner()) =
                Some((scope, owner, result.clone()));
        }
        result
    }
    pub(super) fn policy_admit_inner(
        &self,
        manager: &Manager,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let info = inspect(manager, &self.project, cancel)?;
        let (policy, _) = read_head(manager, &info, cancel)?;
        policy
            .ok_or("svn_policy_missing")?
            .admit(&manager.inner.app_version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn info() -> Inspection {
        Inspection {
            root: "local".into(),
            wc_root: "local".into(),
            url: "https://example.org/svn/team/project".into(),
            repository: "https://example.org/svn/team".into(),
            repository_id: "b726ba7c-6f1b-4cc3-98e1-316203527a4a".into(),
            revision: "1".into(),
        }
    }
    fn policy() -> Policy {
        Policy {
            format_version: 1,
            repository: info().repository,
            repository_id: info().repository_id,
            project: info().url,
            minimum_app_version: "1.2.10".into(),
            support_floor: if cfg!(feature = "compatibility-test") {
                "0.2.0"
            } else {
                "1.0.0"
            }
            .into(),
        }
    }
    #[test]
    fn semantic_order_and_equal_boundary() {
        let p = policy();
        assert!(p.admit(&Version::new(1, 2, 9)).is_err());
        assert!(p.admit(&Version::new(1, 2, 10)).is_ok());
        assert!(p.admit(&Version::new(1, 10, 0)).is_ok());
    }
    #[test]
    fn format_scope_corruption_and_support_floor_fail_closed() {
        let mut p = policy();
        assert!(Policy::decode(&serde_json::to_vec(&p).unwrap(), &info()).is_ok());
        p.format_version = 2;
        assert_eq!(
            Policy::decode(&serde_json::to_vec(&p).unwrap(), &info()).unwrap_err(),
            "svn_policy_format_unsupported"
        );
        p.format_version = 1;
        p.project.push_str("/other");
        assert!(Policy::decode(&serde_json::to_vec(&p).unwrap(), &info()).is_err());
        assert!(Policy::decode(b"{}", &info()).is_err());
        p = policy();
        p.minimum_app_version = "1.2.3-preview".into();
        assert!(Policy::decode(&serde_json::to_vec(&p).unwrap(), &info()).is_err());
        p = policy();
        p.support_floor = "9.0.0".into();
        assert!(Policy::decode(&serde_json::to_vec(&p).unwrap(), &info()).is_err());
    }
    #[test]
    fn future_unavailable_versions_cannot_be_selected() {
        assert!(Policy::new(&info(), "999.0.0", &Version::new(1, 0, 0)).is_err());
    }
    #[test]
    fn exact_history_and_listing_identity() {
        assert!(has_entry(
            "<lists><list><entry><name>worldbuild-policy.json</name></entry></list></lists>",
            FILE
        )
        .unwrap());
        assert!(!has_entry(
            "<lists><list><entry><name>other.json</name></entry></list></lists>",
            FILE
        )
        .unwrap());
        assert!(history_has_path("<log><logentry><paths><path action=\"D\">/project/worldbuild-policy.json</path></paths></logentry></log>","/project/worldbuild-policy.json").unwrap());
        assert!(!history_has_path("<log><logentry><paths><path>/other/worldbuild-policy.json</path></paths></logentry></log>","/project/worldbuild-policy.json").unwrap());
    }
}

/// Structural validation for the exact shared allowlist. Scope is additionally
/// bound to real SVN info when the policy is read/saved/committed.
pub(crate) fn validate_file(path: &Path) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|_| "svn_policy_local_unreadable")?;
    let p: Policy = serde_json::from_slice(&bytes).map_err(|_| "svn_policy_invalid")?;
    let info = Inspection {
        root: String::new(),
        wc_root: String::new(),
        url: p.project.clone(),
        repository: p.repository.clone(),
        repository_id: p.repository_id.clone(),
        revision: String::new(),
    };
    Policy::decode(&bytes, &info).map(|_| ())
}
impl SvnLockService {
    /// Registration already owns the codec/asset/dependency checks. Do not
    /// open the application repository here: admission must remain read-only.
    fn initialization_project(
        &self,
        manager: &Manager,
        info: &Inspection,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        let root = Path::new(&info.root);
        let inventory = crate::svn_shared::registration_inventory(root)
            .map_err(|_| "svn_policy_project_invalid")?;
        let (_, files) = crate::data::project_backup::snapshot_paths(root)
            .map_err(|_| "svn_policy_project_invalid")?;
        // Flat artifact namespaces must not silently discard malformed IDs or
        // nested project files as unrecognized registration extras.
        if files.iter().any(|path| {
            (path.starts_with("templates/") || path.starts_with("documents/"))
                && !inventory.files.contains(path)
        }) {
            return Err("svn_policy_project_invalid".into());
        }
        if inventory.files.iter().any(|path| {
            path.starts_with("templates/")
                || path.starts_with("documents/")
                || path == "workspace/document-layout.json"
        }) {
            return Ok(inventory.fingerprint);
        }
        // Empty projects have no required manifest. Their existing SVN
        // registration metadata is the evidence: app ignore rules and four
        // versioned creation parents, all bound to this exact root/URL/repo.
        // Directory names alone (including unversioned lookalikes) are not
        // enough. A parent merely containing another project is not eligible.
        for entry in fs::read_dir(root).map_err(|_| "svn_policy_project_invalid")? {
            let entry = entry.map_err(|_| "svn_policy_project_invalid")?;
            let name = entry.file_name();
            if !matches!(
                name.to_str(),
                Some(
                    "assets"
                        | "documents"
                        | "templates"
                        | "workspace"
                        | ".svn"
                        | ".git"
                        | ".worldbuild"
                        | "logs"
                        | "cache"
                        | FILE
                )
            ) {
                return Err("svn_policy_project_invalid".into());
            }
        }
        let xml = self.run_for(manager, "proplist", &["--xml", "--verbose"], root, cancel)?;
        let mut reader = Reader::from_str(&xml);
        let mut ignore = None;
        loop {
            match reader.read_event().map_err(|_| "svn_xml_invalid")? {
                Event::Start(e) if e.name() == QName(b"property") => {
                    let is_ignore = e
                        .attributes()
                        .flatten()
                        .any(|a| a.key == QName(b"name") && a.value.as_ref() == b"svn:ignore");
                    if is_ignore {
                        ignore = Some(read_xml_text(&mut reader, b"property")?);
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        let ignore = ignore.ok_or("svn_policy_project_invalid")?;
        if [".worldbuild", ".git", "logs", "cache"]
            .iter()
            .any(|rule| !ignore.lines().any(|line| line == *rule))
        {
            return Err("svn_policy_project_invalid".into());
        }
        for name in ["assets", "documents", "templates", "workspace"] {
            let child = inspect(manager, &root.join(name), cancel)?;
            if child.root != root.join(name).to_string_lossy()
                || child.wc_root != info.wc_root
                || child.url != format!("{}/{}", info.url.trim_end_matches('/'), name)
                || child.repository != info.repository
                || child.repository_id != info.repository_id
            {
                return Err("svn_policy_project_invalid".into());
            }
        }
        Ok(inventory.fingerprint)
    }

    pub(crate) fn policy_initialize(
        &self,
        request: &str,
        minimum: &str,
    ) -> Result<Snapshot, String> {
        self.with_request(request, |manager, cancel| {
            let info = inspect(manager, &self.project, cancel)?;
            let (existing, _) = read_head(manager, &info, cancel)?;
            if existing.is_some() {
                return Err("svn_policy_already_initialized".into());
            }
            let root_guard = crate::data::project_file::directory::ProjectDirectory::open_root(
                Path::new(&info.root),
            )
            .map_err(|_| "svn_policy_project_invalid")?;
            let fingerprint = self.initialization_project(manager, &info, cancel)?;
            let policy = Policy::new(&info, minimum, &manager.inner.app_version)?;
            let local = self.project.join(FILE);
            let former_policy = if local.exists() {
                let metadata =
                    fs::symlink_metadata(&local).map_err(|_| "svn_policy_local_unreadable")?;
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err("svn_policy_local_unreadable".into());
                }
                let bytes = fs::read(&local).map_err(|_| "svn_policy_local_unreadable")?;
                validate_file(&local)?;
                // This explicit initializer is the only place that moves an
                // unversioned former-scope policy out of the working copy.
                // Preserve exact bytes first; failures leave the original intact.
                let archive = manager.inner.config_root.join("policy-copy-history");
                fs::create_dir_all(&archive).map_err(|_| "svn_config_failed")?;
                let saved = archive.join(format!("{}.json", uuid::Uuid::new_v4()));
                fs::write(&saved, &bytes).map_err(|_| "svn_config_failed")?;
                if fs::read(&saved).map_err(|_| "svn_config_failed")? != bytes {
                    return Err("svn_config_failed".into());
                }
                // Status must prove this file is unversioned. Deleted or
                // modified tracked policies must never be reinitialized.
                let (rows, _) =
                    parse_status(&self.run_for(manager, "status", &["--xml"], &local, cancel)?)?;
                if rows.len() != 1 || rows[0].local != "unversioned" {
                    return Err("svn_policy_changed".into());
                }
                Some(bytes)
            } else {
                None
            };
            root_guard
                .validate()
                .map_err(|_| "svn_policy_project_invalid")?;
            let observed = inspect(manager, &self.project, cancel)?;
            if observed.root != info.root
                || observed.wc_root != info.wc_root
                || observed.url != info.url
                || observed.repository != info.repository
                || observed.repository_id != info.repository_id
                || self.initialization_project(manager, &observed, cancel)? != fingerprint
            {
                return Err("svn_policy_changed".into());
            }
            let temporary = manager
                .inner
                .config_root
                .join(format!("policy-init-{}.json", uuid::Uuid::new_v4()));
            fs::write(
                &temporary,
                serde_json::to_vec_pretty(&policy).map_err(|_| "svn_policy_invalid")?,
            )
            .map_err(|_| "svn_config_failed")?;
            let identity = manager.identity();
            let url = format!("{}/{}", info.url.trim_end_matches('/'), FILE);
            let mut args = args_with_auth(
                manager,
                identity.as_ref(),
                "import",
                &[
                    "-m",
                    "Initialize project minimum app version",
                    "--no-auto-props",
                ],
                &url,
            );
            let delimiter = args
                .iter()
                .position(|arg| arg == "--")
                .ok_or("svn_invalid_request")?;
            // Keep the resolved app-owned source directory, but pass one
            // exact relative filename. SVN cannot parse verbatim Windows
            // prefixes, and @/Unicode in parent paths must not become a peg.
            let source_directory = ordinary_windows_path(&manager.inner.config_root);
            args.insert(
                delimiter + 1,
                temporary
                    .file_name()
                    .ok_or("svn_invalid_request")?
                    .to_string_lossy()
                    .into_owned(),
            );
            // import's destination is a URL without peg syntax. Appending @
            // here would create a different server child named policy.json@.
            *args.last_mut().ok_or("svn_invalid_request")? = url.clone();
            let result = run_bytes_timeout(
                &manager.cli()?,
                &args,
                identity.as_ref().and_then(|i| i.password.as_deref()),
                cancel,
                POLICY_TIMEOUT,
                Some(&source_directory),
            );
            let _ = fs::remove_file(&temporary);
            if let Err(reason) = result {
                let category = match reason.as_str() {
                    "svn_auth_failed" => "authentication",
                    "svn_tls_failed" => "tls",
                    "svn_path_exists" => "concurrent_initializer",
                    "svn_timeout" => "timeout",
                    _ => "command_failed",
                };
                crate::diagnostic_log::record(
                    "svn",
                    "policy_initialize",
                    category,
                    "unknown",
                    None,
                    None,
                );
                #[cfg(test)]
                eprintln!("owned policy initializer category: {category}");
                // Import creates one server child atomically and never replaces
                // a concurrent initializer. A failed receipt is not success.
                return Err("svn_policy_initialization_unverified".into());
            }
            let (applied, revision) = read_head(manager, &info, cancel)?;
            if applied.as_ref() != Some(&policy) {
                return Err("svn_policy_initialization_unverified".into());
            }
            // Protect the inspected root through the remote write, then let
            // SVN open it for the explicitly requested local policy update.
            drop(root_guard);
            // The prior copy remains intact until the server initialization
            // is confirmed, and a concurrent local edit must not be erased.
            if let Some(bytes) = former_policy {
                if fs::read(&local).map_err(|_| "svn_policy_local_unreadable")? != bytes {
                    return Err("svn_policy_initialization_unverified".into());
                }
                fs::remove_file(&local).map_err(|_| "svn_policy_initialization_unverified")?;
            }
            // Bring only this explicitly initialized policy into the WC.
            let local = self.project.join(FILE);
            self.run_for(manager, "update", &["--depth", "empty"], &local, cancel)?;
            Ok(Snapshot {
                current_app_version: manager.inner.app_version.to_string(),
                server: applied.clone(),
                local: applied,
                local_modified: Some(false),
                revision,
                issue: None,
                available_versions: available(&manager.inner.app_version),
            })
        })
    }
    pub(crate) fn policy_save(
        &self,
        minimum: &str,
        expected_revision: &str,
    ) -> Result<Snapshot, String> {
        self.with_operation(|manager, cancel| {
            let info = inspect(manager, &self.project, cancel)?;
            let (server, revision) = read_head(manager, &info, cancel)?;
            let server = server.ok_or("svn_policy_missing")?;
            server.admit(&manager.inner.app_version)?;
            if revision != expected_revision {
                return Err("svn_policy_changed".into());
            }
            let desired = Policy::new(&info, minimum, &manager.inner.app_version)?;
            let file = self.project.join(FILE);
            let canonical = file
                .canonicalize()
                .map_err(|_| "svn_policy_local_unreadable")?;
            if canonical.parent()
                != Some(
                    self.project
                        .canonicalize()
                        .map_err(|_| "svn_project_missing")?
                        .as_path(),
                )
            {
                return Err("svn_invalid_target".into());
            }
            let base = self.run_for(manager, "cat", &["-r", "BASE"], &file, cancel)?;
            if Policy::decode(base.as_bytes(), &info)? != server {
                return Err("svn_policy_changed".into());
            }
            let temporary = self
                .project
                .join(format!(".policy-save-{}", uuid::Uuid::new_v4()));
            let bytes = serde_json::to_vec_pretty(&desired).map_err(|_| "svn_policy_invalid")?;
            fs::write(&temporary, bytes).map_err(|_| "svn_policy_local_unreadable")?;
            let renamed = fs::rename(&temporary, &canonical);
            if renamed.is_err() {
                let _ = fs::remove_file(&temporary);
                return Err("svn_policy_local_unreadable".into());
            }
            Ok(Snapshot {
                current_app_version: manager.inner.app_version.to_string(),
                server: Some(server),
                local: Some(desired),
                local_modified: Some(true),
                revision,
                issue: None,
                available_versions: available(&manager.inner.app_version),
            })
        })
    }
}
#[tauri::command]
pub(crate) async fn svn_policy_snapshot<R: Runtime>(
    webview: Webview<R>,
    manager: State<'_, Manager>,
    path: String,
) -> Result<Snapshot, String> {
    main_only(&webview)?;
    let provider = SvnLockService::new(manager.inner().clone(), PathBuf::from(path));
    tauri::async_runtime::spawn_blocking(move || provider.policy_snapshot())
        .await
        .map_err(|_| "svn_task_failed".to_owned())?
}
#[tauri::command]
pub(crate) async fn svn_policy_initialize<R: Runtime>(
    webview: Webview<R>,
    manager: State<'_, Manager>,
    path: String,
    request: String,
    minimum: String,
) -> Result<Snapshot, String> {
    main_only(&webview)?;
    let manager = manager.inner().clone();
    let _custody = manager.result_custody()?;
    let correlation = request.clone();
    let target = path.clone();
    let requested_minimum = minimum.clone();
    let provider = SvnLockService::new(manager.clone(), PathBuf::from(path));
    let result = tauri::async_runtime::spawn_blocking(move || {
        provider.policy_initialize(&request, &minimum)
    })
    .await
    .unwrap_or_else(|_| Err("svn_task_failed".into()));
    if result.as_ref().err().is_some_and(|reason| {
        matches!(
            reason.as_str(),
            "svn_policy_initialization_unverified" | "svn_task_failed"
        )
    }) {
        let record = crate::updater::handoff::PendingSvn {
            operation_id: correlation.clone(),
            project_fingerprint: format!("{:x}", Sha256::digest(target.as_bytes())),
            requested_paths: vec![FILE.into()],
            outcome: "unknown".into(),
            observed: serde_json::json!({"action":"policy_initialize","minimumAppVersion":requested_minimum,"error":result.as_ref().err()}),
        };
        if manager.inner.pending.preserve(record).is_err() {
            crate::diagnostic_log::record(
                "svn",
                "policy_initialize",
                "preserve_failed",
                "error",
                Some(correlation),
                None,
            );
        }
    }
    result
}
#[tauri::command]
pub(crate) async fn svn_policy_save<R: Runtime>(
    webview: Webview<R>,
    app: State<'_, Arc<crate::state::AppState>>,
    path: String,
    minimum: String,
    revision: String,
) -> Result<Snapshot, String> {
    main_only(&webview)?;
    let provider = app.svn_commit_provider(Path::new(&path))?;
    let _custody = provider.manager.result_custody()?;
    tauri::async_runtime::spawn_blocking(move || provider.policy_save(&minimum, &revision))
        .await
        .map_err(|_| "svn_task_failed".to_owned())?
}

impl SvnLockService {
    pub(super) fn policy_verify_applied(
        &self,
        manager: &Manager,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let info = inspect(manager, &self.project, cancel)?;
        let (server, _) = read_head(manager, &info, cancel)?;
        let bytes = fs::read(self.project.join(FILE)).map_err(|_| "svn_policy_local_unreadable")?;
        let local = Policy::decode(&bytes, &info)?;
        if server.as_ref() != Some(&local) {
            return Err("svn_policy_commit_unverified".into());
        }
        Ok(())
    }
}

#[cfg(all(test, windows, feature = "compatibility-test"))]
mod owned_tests {
    use super::*;
    fn cli_run(cli: &Path, args: &[&str]) -> String {
        let output = Command::new(cli)
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "owned SVN command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
    #[test]
    fn owned_initialization_race_and_former_scope_copy_preserve_exact_metadata() {
        let source =
            PathBuf::from(std::env::var("M8_B_POLICY_FIXTURE").expect("owned fixture root"));
        let base = source.with_file_name(format!(
            "{}-race",
            source.file_name().unwrap().to_str().unwrap()
        ));
        assert!(!base.exists());
        fs::create_dir_all(&base).unwrap();
        let cli = PathBuf::from("C:/Program Files/TortoiseSVN/bin/svn.exe");
        let repository = base.join("repo");
        cli_run(
            &cli.with_file_name("svnadmin.exe"),
            &["create", repository.to_str().unwrap()],
        );
        let root_url = Url::from_directory_path(&repository).unwrap().to_string();
        let old_url = root_url.clone() + "old";
        let new_url = root_url.clone() + "%EC%83%88%20%ED%94%84%EB%A1%9C%EC%A0%9D%ED%8A%B8%25";
        cli_run(
            &cli,
            &["mkdir", &old_url, &new_url, "-m", "two owned projects"],
        );
        let old = base.join("old");
        let copy = base.join("copy");
        let peer = base.join("peer");
        cli_run(&cli, &["checkout", &old_url, old.to_str().unwrap()]);
        cli_run(&cli, &["checkout", &new_url, copy.to_str().unwrap()]);
        cli_run(&cli, &["checkout", &new_url, peer.to_str().unwrap()]);
        super::initialization_tests::write_template(&old);
        super::initialization_tests::write_template(&copy);
        let manager_for = |name: &str| {
            let config = base.join(name);
            fs::create_dir(&config).unwrap();
            let manager = Manager::new(config);
            assert!(probe(&manager, Some(cli.to_string_lossy().into_owned()), None).installed);
            manager
        };
        let old_service = SvnLockService::new(manager_for("old-app"), old.clone());
        old_service
            .policy_initialize(&uuid::Uuid::new_v4().to_string(), "0.2.0")
            .unwrap();
        let bytes = fs::read(old.join(FILE)).unwrap();
        fs::write(copy.join(FILE), &bytes).unwrap();
        assert!(crate::svn_shared::inventory(&copy)
            .unwrap()
            .files
            .iter()
            .any(|p| p == FILE));
        assert!(!crate::svn_shared::registration_inventory(&copy)
            .unwrap()
            .files
            .iter()
            .any(|p| p == FILE));
        let manager = manager_for("copy-app");
        let config = manager.inner.config_root.clone();
        let service = SvnLockService::new(manager, copy.clone());
        service
            .policy_initialize(&uuid::Uuid::new_v4().to_string(), "0.2.0")
            .unwrap();
        let archives: Vec<_> = fs::read_dir(config.join("policy-copy-history"))
            .unwrap()
            .collect();
        assert_eq!(archives.len(), 1);
        assert_eq!(
            fs::read(archives[0].as_ref().unwrap().path()).unwrap(),
            bytes
        );
        assert_eq!(fs::read(old.join(FILE)).unwrap(), bytes);
        let initialized = service.policy_snapshot().unwrap();
        assert_eq!(initialized.server.as_ref().unwrap().project, new_url);
        let other = SvnLockService::new(manager_for("peer-app"), peer);
        assert_eq!(
            other
                .policy_initialize(&uuid::Uuid::new_v4().to_string(), "0.2.0")
                .err()
                .as_deref(),
            Some("svn_policy_already_initialized")
        );
        let race_url = root_url + "race";
        cli_run(
            &cli,
            &["mkdir", &race_url, "-m", "owned concurrent initialization"],
        );
        let mut workers = Vec::new();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        for name in ["race-a", "race-b"] {
            let wc = base.join(name);
            cli_run(&cli, &["checkout", &race_url, wc.to_str().unwrap()]);
            super::initialization_tests::write_template(&wc);
            let service = SvnLockService::new(manager_for(&format!("{name}-app")), wc);
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                service.policy_initialize(&uuid::Uuid::new_v4().to_string(), "0.2.0")
            }));
        }
        let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert!(results
            .iter()
            .filter_map(|r| r.as_ref().err())
            .all(|e| matches!(
                e.as_str(),
                "svn_policy_already_initialized" | "svn_policy_initialization_unverified"
            )));
        let log = cli_run(&cli, &["log", "--xml", "--verbose", &race_url]);
        assert_eq!(log.matches("/race/worldbuild-policy.json").count(), 1);
        fs::write(base.join("initialization-race.xml"), log).unwrap();
        // A server-side corrupt policy is not absence and cannot be initialized over.
        fs::write(copy.join(FILE), b"{broken").unwrap();
        cli_run(
            &cli,
            &[
                "commit",
                "-m",
                "owned corrupt policy",
                copy.join(FILE).to_str().unwrap(),
            ],
        );
        assert_eq!(
            service.policy_admit().err().as_deref(),
            Some("svn_policy_invalid")
        );
        assert_eq!(
            service
                .policy_initialize(&uuid::Uuid::new_v4().to_string(), "0.2.0")
                .err()
                .as_deref(),
            Some("svn_policy_invalid")
        );
    }

    #[test]
    fn owned_server_head_blocks_without_wc_update_and_preserves_policy_history() {
        let base = PathBuf::from(
            std::env::var("M8_B_POLICY_FIXTURE").expect("explicit owned fixture root"),
        );
        assert!(
            !base.exists(),
            "fixture must be new; preserve previous evidence"
        );
        fs::create_dir_all(&base).unwrap();
        let cli = PathBuf::from("C:/Program Files/TortoiseSVN/bin/svn.exe");
        let admin = cli.with_file_name("svnadmin.exe");
        let repository = base.join("repo");
        cli_run(&admin, &["create", repository.to_str().unwrap()]);
        let url = Url::from_directory_path(&repository).unwrap().to_string() + "project";
        cli_run(
            &cli,
            &[
                "mkdir",
                &url,
                "-m",
                "owned project",
                "--username",
                "owner-a",
            ],
        );
        let a = base.join("a");
        let b = base.join("b");
        cli_run(&cli, &["checkout", &url, a.to_str().unwrap()]);
        cli_run(&cli, &["checkout", &url, b.to_str().unwrap()]);
        super::initialization_tests::write_template(&a);
        let config = base.join("app-data");
        fs::create_dir(&config).unwrap();
        let manager = Manager::new(config);
        probe(&manager, Some(cli.to_string_lossy().into_owned()), None);
        let first = SvnLockService::new(manager.clone(), a.clone());
        assert_eq!(
            first.policy_snapshot().unwrap().issue.as_deref(),
            Some("svn_policy_missing")
        );
        first
            .policy_initialize(&uuid::Uuid::new_v4().to_string(), "0.2.0")
            .unwrap();
        assert_eq!(
            first
                .policy_initialize(&uuid::Uuid::new_v4().to_string(), "0.2.0")
                .err()
                .as_deref(),
            Some("svn_policy_already_initialized")
        );
        cli_run(&cli, &["update", b.to_str().unwrap()]);
        let second = SvnLockService::new(manager.clone(), b.clone());
        let original = fs::read(b.join(FILE)).unwrap();
        let info_before = cli_run(&cli, &["info", "--xml", b.to_str().unwrap()]);
        let policy_info_before = cli_run(&cli, &["info", "--xml", b.join(FILE).to_str().unwrap()]);
        let mut uncommitted: Policy = serde_json::from_slice(&original).unwrap();
        uncommitted.minimum_app_version = "0.3.0".into();
        fs::write(
            b.join(FILE),
            serde_json::to_vec_pretty(&uncommitted).unwrap(),
        )
        .unwrap();
        assert!(
            second.policy_admit().is_ok(),
            "local uncommitted minimum does not become team policy"
        );
        assert_eq!(second.policy_snapshot().unwrap().local_modified, Some(true));
        fs::write(b.join(FILE), &original).unwrap();
        fs::write(
            a.join(FILE),
            serde_json::to_vec_pretty(&uncommitted).unwrap(),
        )
        .unwrap();
        cli_run(
            &cli,
            &[
                "commit",
                a.join(FILE).to_str().unwrap(),
                "-m",
                "owner-a raises minimum",
                "--username",
                "owner-a",
            ],
        );
        assert_eq!(
            second.policy_admit().err().as_deref(),
            Some("svn_policy_app_too_old:0.3.0")
        );
        assert_eq!(fs::read(b.join(FILE)).unwrap(), original);
        assert_eq!(
            second.policy_snapshot().unwrap().local_modified,
            Some(false)
        );
        let info_after = cli_run(&cli, &["info", "--xml", b.to_str().unwrap()]);
        let policy_info_after = cli_run(&cli, &["info", "--xml", b.join(FILE).to_str().unwrap()]);
        // SVN XML attribute order is not stable; compare actual WC revision
        // values, while policy file contents remain an exact byte assertion.
        assert_eq!(
            head_revision(&info_after).unwrap(),
            head_revision(&info_before).unwrap()
        );
        assert_eq!(
            head_revision(&policy_info_after).unwrap(),
            head_revision(&policy_info_before).unwrap()
        );
        fs::write(base.join("b-info-before.xml"), &info_before).unwrap();
        fs::write(base.join("b-info-after.xml"), &info_after).unwrap();
        let hash = format!("{:x}", Sha256::digest(&original));
        fs::write(base.join("receipt.json"),serde_json::to_vec_pretty(&serde_json::json!({"sample":"owned FSFS server HEAD","initialPolicy":"0.2.0","raisedPolicy":"0.3.0","currentApp":"0.2.0","wcUpdateDuringCheck":false,"wcRevisionUnchanged":true,"policyBytesUnchanged":true,"policySHA256":hash,"realInstaller":false,"authenticationServer":false})).unwrap()).unwrap();
        // A later, supported app cannot overwrite stale WC policy bytes.
        let high_config = base.join("high-app");
        fs::create_dir(&high_config).unwrap();
        let high = Manager::new_with_version(high_config, Version::new(0, 4, 0));
        probe(&high, Some(cli.to_string_lossy().into_owned()), None);
        let high_provider = SvnLockService::new(high.clone(), b.clone());
        let before = high_provider.policy_snapshot().unwrap();
        assert_eq!(
            high_provider
                .policy_save("0.4.0", &before.revision)
                .err()
                .as_deref(),
            Some("svn_policy_changed")
        );
        cli_run(&cli, &["update", b.join(FILE).to_str().unwrap()]);
        let before = high_provider.policy_snapshot().unwrap();
        let saved = high_provider
            .policy_save("0.4.0", &before.revision)
            .unwrap();
        assert_eq!(saved.server.unwrap().minimum_app_version, "0.3.0");
        assert_eq!(saved.local.unwrap().minimum_app_version, "0.4.0");
        let candidates = high_provider.candidates().unwrap();
        assert!(candidates.iter().any(|row| row.path == FILE
            && row.eligible
            && row.name.as_deref() == Some("프로젝트 설정 — 최소 앱 버전")));
        let committed = high_provider
            .commit(
                uuid::Uuid::new_v4().to_string(),
                vec![FILE.into()],
                "Explicit minimum app version".into(),
            )
            .unwrap();
        assert!(committed.verification_unknown.is_empty());
        assert_eq!(committed.paths, vec![FILE.to_owned()]);
        assert_eq!(
            high_provider
                .policy_snapshot()
                .unwrap()
                .server
                .unwrap()
                .minimum_app_version,
            "0.4.0"
        );
        let log = cli_run(
            &cli,
            &["log", "--xml", "--verbose", "-r", &committed.revision, &url],
        );
        fs::write(base.join("policy-commit.xml"), &log).unwrap();
        assert_eq!(
            log.matches("<path").count(),
            2,
            "one paths container plus one policy changed path"
        );
        // Editing the file externally cannot publish an unavailable future
        // minimum through the otherwise supported native commit command.
        let mut future =
            serde_json::from_slice::<Policy>(&fs::read(b.join(FILE)).unwrap()).unwrap();
        future.minimum_app_version = "9.0.0".into();
        fs::write(b.join(FILE), serde_json::to_vec_pretty(&future).unwrap()).unwrap();
        let rejected = high_provider.candidates().unwrap();
        assert!(rejected.iter().any(|row| row.path == FILE && !row.eligible));
        assert_eq!(
            high_provider
                .commit(
                    uuid::Uuid::new_v4().to_string(),
                    vec![FILE.into()],
                    "Rejected future minimum".into()
                )
                .err()
                .as_deref(),
            Some("svn_commit_selection_changed")
        );
        cli_run(&cli, &["update", a.join(FILE).to_str().unwrap()]);
        cli_run(&cli, &["delete", a.join(FILE).to_str().unwrap()]);
        cli_run(
            &cli,
            &[
                "commit",
                a.join(FILE).to_str().unwrap(),
                "-m",
                "controlled deletion",
                "--username",
                "owner-a",
            ],
        );
        assert_eq!(
            second.policy_admit().err().as_deref(),
            Some("svn_policy_deleted")
        );
        let unseen_config = base.join("unseen-app");
        fs::create_dir(&unseen_config).unwrap();
        let unseen = Manager::new(unseen_config);
        probe(&unseen, Some(cli.to_string_lossy().into_owned()), None);
        assert_eq!(
            SvnLockService::new(unseen, b)
                .policy_snapshot()
                .err()
                .as_deref(),
            Some("svn_policy_deleted")
        );
    }
}

impl SvnLockService {
    pub(super) fn policy_validate_candidate(
        &self,
        manager: &Manager,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let info = inspect(manager, &self.project, cancel)?;
        let (server, _) = read_head(manager, &info, cancel)?;
        let server = server.ok_or("svn_policy_missing")?;
        let bytes = fs::read(self.project.join(FILE)).map_err(|_| "svn_policy_local_unreadable")?;
        let local = Policy::decode(&bytes, &info)?;
        local.admit(&manager.inner.app_version)?;
        if local.minimum_app_version != server.minimum_app_version
            && !available(&manager.inner.app_version).contains(&local.minimum_app_version)
        {
            return Err("svn_policy_version_unavailable".into());
        }
        Ok(())
    }
}

fn decode_path(value: &str) -> Result<String, String> {
    let mut bytes = Vec::with_capacity(value.len());
    let raw = value.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' {
            if i + 2 >= raw.len() {
                return Err("svn_invalid_url".into());
            }
            let hex = std::str::from_utf8(&raw[i + 1..i + 3]).map_err(|_| "svn_invalid_url")?;
            bytes.push(u8::from_str_radix(hex, 16).map_err(|_| "svn_invalid_url")?);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(bytes).map_err(|_| "svn_invalid_url")?;
    if decoded.chars().any(|c| c.is_control()) {
        return Err("svn_invalid_url".into());
    }
    Ok(decoded)
}
#[cfg(test)]
mod path_scope_tests {
    use super::*;
    #[test]
    fn deleted_policy_history_matches_unicode_space_and_literal_percent_paths() {
        assert_eq!(
            decode_path("/%ED%94%84%EB%A1%9C%EC%A0%9D%ED%8A%B8%20A/worldbuild-policy.json")
                .unwrap(),
            "/프로젝트 A/worldbuild-policy.json"
        );
        assert_eq!(
            decode_path("/100%25/worldbuild-policy.json").unwrap(),
            "/100%/worldbuild-policy.json"
        );
        assert!(decode_path("/%00/policy").is_err());
        assert!(decode_path("/%ZZ/policy").is_err());
    }
}

#[cfg(all(test, windows, feature = "compatibility-test"))]
#[test]
fn owned_https_native_initializer_uses_remembered_profile() {
    let root = std::path::PathBuf::from(
        std::env::var("M8_B_HTTPS_FIXTURE").expect("owned HTTPS fixture required"),
    );
    let config = root.join("data/svn");
    let manager = Manager::new_with_version(config, Version::new(0, 2, 1));
    let provider = SvnLockService::new(manager, root.join("https-project-a"));
    let snapshot = provider.policy_snapshot().unwrap();
    assert_eq!(snapshot.issue.as_deref(), Some("svn_policy_missing"));
    let initialized = provider.policy_initialize(&uuid::Uuid::new_v4().to_string(), "0.2.1");
    assert!(
        initialized.is_ok(),
        "native owned initializer: {:?}",
        initialized.err()
    );
    let confirmed = provider.policy_snapshot().unwrap();
    assert!(confirmed.issue.is_none());
    assert_eq!(confirmed.server.unwrap().minimum_app_version, "0.2.1");
}

#[cfg(all(test, windows))]
#[test]
fn owned_windows_canonical_config_initializes_exact_policy_child() {
    let base =
        std::env::temp_dir().join(format!("worldbuild-m8-canonical-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(base.join("한글 @ config")).unwrap();
    let cli = PathBuf::from("C:/Program Files/TortoiseSVN/bin/svn.exe");
    let repository = base.join("repo");
    assert!(
        std::process::Command::new(cli.with_file_name("svnadmin.exe"))
            .args(["create", repository.to_str().unwrap()])
            .output()
            .unwrap()
            .status
            .success()
    );
    let url = Url::from_directory_path(&repository).unwrap().to_string();
    let project = base.join("project");
    assert!(std::process::Command::new(&cli)
        .args(["checkout", &url, project.to_str().unwrap()])
        .output()
        .unwrap()
        .status
        .success());
    initialization_tests::write_template(&project);
    let manager = Manager::new_with_version(
        base.join("한글 @ config").canonicalize().unwrap(),
        Version::new(1, 0, 0),
    );
    assert!(probe(&manager, Some(cli.to_string_lossy().into_owned()), None).installed);
    let provider = SvnLockService::new(manager, project.clone());
    let initialized = provider.policy_initialize(&uuid::Uuid::new_v4().to_string(), "1.0.0");
    assert!(
        initialized.is_ok(),
        "canonical config initializer: {:?}",
        initialized.err()
    );
    assert!(project.join(FILE).is_file());
    assert!(!project.join(format!("{FILE}@")).exists());
    let current = provider.policy_snapshot().unwrap();
    assert!(current.issue.is_none());
    assert_eq!(current.server.unwrap().minimum_app_version, "1.0.0");
}

#[cfg(all(test, windows))]
mod initialization_tests;
