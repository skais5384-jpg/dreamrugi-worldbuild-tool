use super::*;
use sha2::{Digest, Sha256};

pub(super) fn write_template(root: &Path) -> PathBuf {
    let id = uuid::Uuid::new_v4();
    let value = serde_json::json!({
        "artifactType":"template", "schemaVersion":1, "templateId":id,
        "revision":1, "name":"FIX001 owned Template", "lifecycle":"active",
        "presentation":{}, "fieldOrder":[], "fields":{},
        "createdAtUtc":"2026-09-03T01:02:03.004Z",
        "updatedAtUtc":"2026-09-03T01:02:03.004Z"
    });
    fs::create_dir_all(root.join("templates")).unwrap();
    let file = root.join(format!("templates/{id}.json"));
    fs::write(&file, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    crate::data::artifact::decode_template(&fs::read(&file).unwrap()).unwrap();
    file
}

struct Fixture {
    base: PathBuf,
    cli: PathBuf,
    url: String,
}
impl Fixture {
    fn new(name: &str) -> Self {
        let parent = std::env::var_os("M8_B_FIX_POLICY_FIXTURE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("m8-b-fix-{}", uuid::Uuid::new_v4()))
            });
        let base = parent.join(name);
        assert!(!base.exists(), "preserve previous fixture");
        fs::create_dir_all(&base).unwrap();
        let cli = PathBuf::from("C:/Program Files/TortoiseSVN/bin/svn.exe");
        let repo = base.join("repo");
        command(
            &cli.with_file_name("svnadmin.exe"),
            &["create", repo.to_str().unwrap()],
        );
        Self {
            base,
            cli,
            url: Url::from_directory_path(repo).unwrap().to_string(),
        }
    }
    fn checkout(&self, name: &str) -> PathBuf {
        let url = format!("{}{name}", self.url);
        self.run(&["mkdir", &url, "-m", "owned test root"]);
        let wc = self.base.join(name);
        self.run(&["checkout", &url, wc.to_str().unwrap()]);
        wc
    }
    fn manager(&self, name: &str) -> Manager {
        let manager =
            Manager::new_with_version(self.base.join(format!("app-{name}")), Version::new(1, 0, 0));
        assert!(
            probe(
                &manager,
                Some(self.cli.to_string_lossy().into_owned()),
                None
            )
            .installed
        );
        manager
    }
    fn run(&self, args: &[&str]) -> String {
        command(&self.cli, args)
    }
    fn revision(&self) -> String {
        command(
            &self.cli.with_file_name("svnlook.exe"),
            &["youngest", self.base.join("repo").to_str().unwrap()],
        )
        .trim()
        .to_owned()
    }
    fn reject_unchanged(&self, wc: &Path) {
        let manager = self.manager(wc.file_name().unwrap().to_str().unwrap());
        let config = manager.inner.config_root.clone();
        let service = SvnLockService::new(manager, wc.to_owned());
        let revision = self.revision();
        let before = tree(wc);
        let status = self.run(&["status", wc.to_str().unwrap()]);
        assert_eq!(
            service
                .policy_initialize(&uuid::Uuid::new_v4().to_string(), "1.0.0")
                .err()
                .as_deref(),
            Some("svn_policy_project_invalid")
        );
        assert_eq!(self.revision(), revision);
        assert_eq!(tree(wc), before);
        assert_eq!(self.run(&["status", wc.to_str().unwrap()]), status);
        assert!(!wc.join(FILE).exists());
        assert!(!config.join("policy-copy-history").exists());
        assert!(!fs::read_dir(config).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("policy-init-")));
    }
}
fn command(cli: &Path, args: &[&str]) -> String {
    let output = Command::new(cli)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "owned SVN failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn tree(root: &Path) -> std::collections::BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, entries: &mut std::collections::BTreeMap<String, String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                entries.insert(relative, "directory".into());
                walk(root, &path, entries);
            } else {
                entries.insert(
                    relative,
                    format!("{:x}", Sha256::digest(fs::read(path).unwrap())),
                );
            }
        }
    }
    let mut entries = Default::default();
    walk(root, root, &mut entries);
    entries
}

#[test]
fn production_initializer_rejects_non_project_wrong_root_and_invalid_artifacts_without_writes() {
    let f = Fixture::new("reject");
    let empty = f.checkout("empty-non-project");
    f.reject_unchanged(&empty);
    let general = f.checkout("general");
    fs::write(general.join("readme.txt"), "ordinary SVN project").unwrap();
    f.reject_unchanged(&general);
    let lookalike = f.checkout("lookalike");
    for dir in ["templates", "documents", "workspace", "assets"] {
        fs::create_dir(lookalike.join(dir)).unwrap();
    }
    f.reject_unchanged(&lookalike);
    let parent = f.checkout("parent");
    write_template(&parent.join("real-project"));
    f.reject_unchanged(&parent);
    for name in ["corrupt", "future", "wrong-id", "invalid-filename"] {
        let wc = f.checkout(name);
        let file = write_template(&wc);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        match name {
            "corrupt" => {
                fs::write(&file, b"{broken").unwrap();
            }
            "future" => {
                value["schemaVersion"] = 999.into();
                fs::write(&file, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            "wrong-id" => {
                value["templateId"] = uuid::Uuid::new_v4().to_string().into();
                fs::write(&file, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            _ => {
                fs::rename(file, wc.join("templates/not-an-id.json")).unwrap();
            }
        }
        f.reject_unchanged(&wc);
    }
    fs::write(f.base.join("rejection-receipt.json"), serde_json::to_vec_pretty(&serde_json::json!({"appVersion":"1.0.0", "compatibilityTest":cfg!(feature="compatibility-test"), "rejected":8, "serverAndWorkingCopiesUnchanged":true})).unwrap()).unwrap();
}

#[test]
fn production_initializer_preserves_valid_lazy_and_registered_empty_projects() {
    let f = Fixture::new("normal");
    let wc = f.checkout("lazy");
    let template = write_template(&wc);
    let original = fs::read(&template).unwrap();
    assert!(!wc.join("documents").exists());
    assert!(!wc.join("assets").exists());
    let service = SvnLockService::new(f.manager("lazy"), wc.clone());
    let rev = f.revision().parse::<u32>().unwrap();
    service
        .policy_initialize(&uuid::Uuid::new_v4().to_string(), "1.0.0")
        .unwrap();
    assert_eq!(f.revision().parse::<u32>().unwrap(), rev + 1);
    assert_eq!(fs::read(template).unwrap(), original);
    assert!(!wc.join("documents").exists());
    assert!(!wc.join("assets").exists());
    let log = f.run(&[
        "log",
        "--xml",
        "--verbose",
        "-r",
        &(rev + 1).to_string(),
        &f.url,
    ]);
    assert_eq!(log.matches("<path").count(), 2); // paths container + one changed path
    assert!(log.contains("/lazy/worldbuild-policy.json"));
    fs::write(f.base.join("lazy-initialization-log.xml"), log).unwrap();
    let empty = f.checkout("registered-empty");
    for dir in ["templates", "documents", "workspace", "assets"] {
        fs::create_dir(empty.join(dir)).unwrap();
        f.run(&["add", empty.join(dir).to_str().unwrap()]);
    }
    f.run(&[
        "propset",
        "svn:ignore",
        ".worldbuild\n.git\nlogs\ncache",
        empty.to_str().unwrap(),
    ]);
    f.run(&[
        "commit",
        empty.to_str().unwrap(),
        "-m",
        "existing app registration metadata",
    ]);
    let before = tree(&empty)
        .into_iter()
        .filter(|(name, _)| !name.starts_with(".svn"))
        .collect::<std::collections::BTreeMap<_, _>>();
    SvnLockService::new(f.manager("empty"), empty.clone())
        .policy_initialize(&uuid::Uuid::new_v4().to_string(), "1.0.0")
        .unwrap();
    let mut after = tree(&empty)
        .into_iter()
        .filter(|(name, _)| !name.starts_with(".svn"))
        .collect::<std::collections::BTreeMap<_, _>>();
    after.remove(FILE);
    assert_eq!(before, after);
    fs::write(
        f.base.join("normal-receipt.json"),
        "normal lazy and app-registered empty projects initialized; no new manifest",
    )
    .unwrap();
}

#[test]
fn direct_initialization_ipc_rejects_empty_non_project() {
    use tauri::{
        ipc::{CallbackFn, InvokeBody},
        webview::InvokeRequest,
    };
    let f = Fixture::new("ipc");
    let wc = f.checkout("empty");
    let revision = f.revision();
    let before = tree(&wc);
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let app =
        crate::commands::register(tauri::test::mock_builder().plugin(tauri_plugin_dialog::init()))
            .manage(f.manager("ipc"))
            .build(context)
            .unwrap();
    let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let response = tauri::test::get_ipc_response(
        &main,
        InvokeRequest {
            cmd: "svn_policy_initialize".into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: InvokeBody::Json(
                serde_json::json!({"request":uuid::Uuid::new_v4().to_string(),"path":wc,"minimum":"1.0.0"}),
            ),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.into(),
        },
    );
    assert_eq!(
        response.err(),
        Some(serde_json::json!("svn_policy_project_invalid"))
    );
    assert_eq!(f.revision(), revision);
    assert_eq!(tree(&wc), before);
}

#[test]
fn production_empty_project_created_by_native_registration_initializes() {
    let f = Fixture::new("native-registration");
    let root = f.base.join("empty-personal");
    fs::create_dir(&root).unwrap();
    let inventory = crate::svn_shared::registration_inventory(&root).unwrap();
    let fingerprint = inventory.fingerprint.clone();
    let manager = f.manager("registration");
    let identity = Identity {
        origin: "file://".into(),
        url: f.url.clone(),
        username: "fix-owner".into(),
        config: manager.inner.config_root.join("owned-profile"),
        password: None,
        remember: false,
    };
    fs::create_dir_all(&identity.config).unwrap();
    let url = Url::parse(&format!("{}empty-app-project", f.url)).unwrap();
    let cancel = AtomicBool::new(false);
    let attempt = RegistrationAttempt {
        root: root.clone(),
        url: url.to_string(),
        fingerprint: fingerprint.clone(),
        username: identity.username.clone(),
        setup: RegistrationSetup::Pending,
    };
    let revision =
        registration_setup(&manager, &f.cli, &identity, &url, attempt, false, &cancel).unwrap();
    registration_finish(
        &manager,
        &f.cli,
        identity,
        url,
        root.clone(),
        inventory,
        fingerprint,
        revision,
        "FIX001 actual native empty registration".into(),
        &cancel,
    )
    .unwrap();
    let before = tree(&root)
        .into_iter()
        .filter(|(p, _)| !p.starts_with(".svn"))
        .collect::<std::collections::BTreeMap<_, _>>();
    let rev = f.revision().parse::<u32>().unwrap();
    SvnLockService::new(manager, root.clone())
        .policy_initialize(&uuid::Uuid::new_v4().to_string(), "1.0.0")
        .unwrap();
    assert_eq!(f.revision().parse::<u32>().unwrap(), rev + 1);
    let mut after = tree(&root)
        .into_iter()
        .filter(|(p, _)| !p.starts_with(".svn"))
        .collect::<std::collections::BTreeMap<_, _>>();
    after.remove(FILE);
    assert_eq!(before, after);
    fs::write(f.base.join("native-registration-receipt.json"),serde_json::to_vec_pretty(&serde_json::json!({"actualNativeRegistration":true,"appVersion":"1.0.0","policyOnlyRevision":rev+1,"emptyProjectContentsUnchanged":true})).unwrap()).unwrap();
}

#[test]
fn production_initialization_commits_only_policy_and_preserves_document_template_asset() {
    let f = Fixture::new("populated");
    let root = f.checkout("project");
    let template = write_template(&root);
    let template_id = template.file_stem().unwrap().to_str().unwrap();
    fs::create_dir(root.join("documents")).unwrap();
    let id = uuid::Uuid::new_v4();
    let document = serde_json::json!({
        "artifactType":"document","schemaVersion":1,"documentId":id,
        "templateId":template_id,"templateRevision":1,"name":"FIX001 preserved document",
        "fieldValues":{},"orphanedFieldDefinitions":{},
        "createdAtUtc":"2026-09-03T01:02:03.004Z","updatedAtUtc":"2026-09-03T01:02:03.004Z"
    });
    let document = serde_json::to_vec_pretty(&document).unwrap();
    crate::data::artifact::decode_document(&document).unwrap();
    fs::write(root.join(format!("documents/{id}.json")), document).unwrap();
    let source = f.base.join("resource.txt");
    fs::write(&source, "FIX001 owned resource; preserve exact bytes").unwrap();
    crate::data::assets::Store::open(&root, true)
        .unwrap()
        .import(&source, false, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    let before = tree(&root)
        .into_iter()
        .filter(|(p, _)| !p.starts_with(".svn"))
        .collect::<std::collections::BTreeMap<_, _>>();
    let rev = f.revision().parse::<u32>().unwrap();
    SvnLockService::new(f.manager("populated"), root.clone())
        .policy_initialize(&uuid::Uuid::new_v4().to_string(), "1.0.0")
        .unwrap();
    let mut after = tree(&root)
        .into_iter()
        .filter(|(p, _)| !p.starts_with(".svn"))
        .collect::<std::collections::BTreeMap<_, _>>();
    after.remove(FILE);
    assert_eq!(before, after);
    let log = f.run(&[
        "log",
        "--xml",
        "--verbose",
        "-r",
        &(rev + 1).to_string(),
        &f.url,
    ]);
    assert_eq!(log.matches("<path").count(), 2);
    assert!(log.contains("/project/worldbuild-policy.json"));
    fs::write(f.base.join("policy-only-log.xml"), log).unwrap();
}

#[test]
fn production_unreadable_project_is_rejected_without_initialization() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = Fixture::new("unreadable");
    let root = f.checkout("project");
    let template = write_template(&root);
    let before = tree(&root);
    let rev = f.revision();
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(template)
        .unwrap();
    let manager = f.manager("unreadable");
    let config = manager.inner.config_root.clone();
    assert_eq!(
        SvnLockService::new(manager, root.clone())
            .policy_initialize(&uuid::Uuid::new_v4().to_string(), "1.0.0")
            .err()
            .as_deref(),
        Some("svn_policy_project_invalid")
    );
    drop(file);
    assert_eq!(tree(&root), before);
    assert_eq!(f.revision(), rev);
    assert!(!fs::read_dir(config).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("policy-init-")));
}
