use super::*;
use std::collections::BTreeSet;
use std::ffi::OsString;

// SVN leaves these exact sidecars beside a text-conflicted document. They are
// not canonical artifacts; the WC conflict itself remains visible to SVN and
// blocks edits until the user resolves it.
fn svn_document_conflict_sidecar(name: &str) -> bool {
    let stem = name.strip_suffix(".mine").or_else(|| {
        let (stem, revision) = name.rsplit_once(".r")?;
        (!revision.is_empty() && revision.bytes().all(|b| b.is_ascii_digit())).then_some(stem)
    });
    let Some(raw_id) = stem.and_then(|stem| stem.strip_suffix(".json")) else {
        return false;
    };
    raw_id
        .parse::<DocumentId>()
        .is_ok_and(|id| raw_id == id.to_string())
}

pub(crate) enum DocumentScanVisitError<E> {
    Repository(RepositoryError),
    Visitor(E),
}

/// Read-only rows deliberately carry no canonical artifact or source token.
pub(crate) struct TemplateDisplay {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) revision: String,
    pub(crate) lifecycle: String,
    pub(crate) glossary_excluded: bool,
}
pub(crate) struct DocumentDisplay {
    pub(crate) id: DocumentId,
    pub(crate) template: String,
    pub(crate) name: String,
    pub(crate) english_name: String,
    pub(crate) glossary_summary: String,
    pub(crate) glossary_excluded: bool,
    pub(crate) available: bool,
}

impl ArtifactRepository<'_, '_> {
    /// Display recovery remains possible for known damaged artifacts. Hard I/O,
    /// namespace and custody failures never become permission to skip a target.
    pub(crate) fn policy_transition_sources(&self) -> Result<Vec<SourceToken>, RepositoryError> {
        let mut result = Vec::new();
        for (kind, operation, current) in [
            (
                ArtifactType::Template,
                RepositoryOperation::ScanTemplates,
                artifact::TEMPLATE_SCHEMA_VERSION,
            ),
            (
                ArtifactType::Document,
                RepositoryOperation::ScanDocuments,
                artifact::DOCUMENT_SCHEMA_VERSION,
            ),
        ] {
            if let Some(directory) = self.namespace(kind, operation)? {
                for id in self.entries(&directory, kind, operation)? {
                    let source = match id {
                        ArtifactSourceId::Template(id) => self
                            .read_template(id, &directory, operation, true)
                            .map(|v| v.source),
                        ArtifactSourceId::Document(id) => self
                            .read_document(id, &directory, operation, true)
                            .map(|v| v.source),
                        ArtifactSourceId::DocumentLayout => unreachable!(),
                    };
                    match source {
                        Ok(source) if source.schema().get() < current.get() => result.push(source),
                        Ok(_) => (),
                        Err(error)
                            if matches!(
                                error.diagnostic().category,
                                RepositoryCategory::CodecRejected | RepositoryCategory::IdMismatch
                            ) =>
                        {
                            ()
                        }
                        Err(error) => return Err(error),
                    }
                }
                self.finish_namespace(&directory, operation, kind)?;
            }
        }
        Ok(result)
    }
    /// 이미 full decode한 scan의 이름 집합을 final prepare 직전에 재대조한다.
    /// 열거 결과만으로 새 완전성 증거를 발급하지 않는다.
    pub(crate) fn confirm_document_membership(
        &self,
        expected: &BTreeSet<ArtifactSourceId>,
    ) -> Result<(), RepositoryError> {
        let kind = ArtifactType::Document;
        let operation = RepositoryOperation::PrepareWrite;
        let actual = match self.namespace(kind, operation)? {
            Some(directory) => {
                let ids = self
                    .entries(&directory, kind, operation)?
                    .into_iter()
                    .collect();
                self.finish_namespace(&directory, operation, kind)?;
                ids
            }
            None => BTreeSet::new(),
        };
        if &actual != expected {
            return Err(RepositoryError::new(
                RepositoryCategory::SourceMismatch,
                operation,
                RepositoryStage::Complete,
                Some(kind),
            ));
        }
        Ok(())
    }

    pub(crate) fn display_templates(&self) -> io::Result<Vec<TemplateDisplay>> {
        let mut ids = versions::targets(self)?
            .into_iter()
            .filter(|id| matches!(id, ArtifactSourceId::Template(_)))
            .collect::<BTreeSet<_>>();
        if let Some(directory) = self
            .namespace(ArtifactType::Template, RepositoryOperation::ScanTemplates)
            .map_err(io::Error::other)?
        {
            ids.extend(
                self.entries(
                    &directory,
                    ArtifactType::Template,
                    RepositoryOperation::ScanTemplates,
                )
                .map_err(io::Error::other)?,
            );
        }
        let mut records = Vec::new();
        for id in ids {
            let ArtifactSourceId::Template(template) = id else {
                continue;
            };
            let artifact = match self.load_template(template) {
                Ok(loaded) => Some(loaded.into_artifact()),
                Err(_) => versions::newest_snapshot(self, id)?
                    .map(|bytes| artifact::decode_template(&bytes).map_err(io::Error::other))
                    .transpose()?,
            };
            records.push(match artifact {
                Some(value) => TemplateDisplay {
                    id: template.to_string(),
                    name: value.name().into(),
                    revision: value.revision().get().to_string(),
                    lifecycle: format!("{:?}", value.lifecycle()),
                    glossary_excluded: value.glossary_excluded(),
                },
                None => TemplateDisplay {
                    id: template.to_string(),
                    name: "내용을 읽을 수 없는 템플릿".into(),
                    revision: String::new(),
                    lifecycle: "Active".into(),
                    glossary_excluded: false,
                },
            });
        }
        Ok(records)
    }
    pub(crate) fn display_documents(&self) -> io::Result<Vec<DocumentDisplay>> {
        let mut ids = versions::targets(self)?
            .into_iter()
            .filter(|id| matches!(id, ArtifactSourceId::Document(_)))
            .collect::<BTreeSet<_>>();
        if let Some(directory) = self
            .namespace(ArtifactType::Document, RepositoryOperation::ScanDocuments)
            .map_err(io::Error::other)?
        {
            ids.extend(
                self.entries(
                    &directory,
                    ArtifactType::Document,
                    RepositoryOperation::ScanDocuments,
                )
                .map_err(io::Error::other)?,
            );
        }
        // A verified layout can retain the identity of a missing item even when no
        // usable versions remain. Discovery is display-only, never a complete scan.
        if let Ok(Some(layout)) = self.load_layout() {
            ids.extend(
                layout
                    .artifact()
                    .nodes
                    .keys()
                    .copied()
                    .map(ArtifactSourceId::Document),
            );
        }
        let mut records = Vec::new();
        for id in ids {
            let ArtifactSourceId::Document(document) = id else {
                continue;
            };
            let loaded = self.load_document(document).ok();
            let available = loaded.is_some();
            let artifact = match loaded {
                Some(loaded) => Some(loaded.into_artifact()),
                None => versions::newest_snapshot(self, id)?
                    .map(|bytes| artifact::decode_document(&bytes).map_err(io::Error::other))
                    .transpose()?,
            };
            records.push(match artifact {
                Some(value) => DocumentDisplay {
                    id: document,
                    template: value.template_id().to_string(),
                    name: value.name().into(),
                    english_name: value.english_name().into(),
                    glossary_summary: value.glossary_summary().into(),
                    glossary_excluded: value.glossary_excluded(),
                    available,
                },
                None => DocumentDisplay {
                    id: document,
                    template: String::new(),
                    name: "내용을 읽을 수 없는 문서".into(),
                    english_name: String::new(),
                    glossary_summary: String::new(),
                    glossary_excluded: false,
                    available: false,
                },
            });
        }
        Ok(records)
    }

    pub(crate) fn scan_templates(&self) -> Result<CompleteTemplateScan, RepositoryError> {
        #[cfg(test)]
        test_support::scan(RepositoryOperation::ScanTemplates);
        let operation = RepositoryOperation::ScanTemplates;
        let kind = ArtifactType::Template;
        let mut records = Vec::new();
        if let Some(directory) = self.namespace(kind, operation)? {
            for id in self.entries(&directory, kind, operation)? {
                let ArtifactSourceId::Template(id) = id else {
                    unreachable!("entry kind is chosen by the scanner")
                };
                let loaded = self.read_template(id, &directory, operation, true)?;
                records.push(TemplateRecord {
                    lifecycle: loaded.artifact.lifecycle(),
                    name: loaded.artifact.name().to_owned(),
                    glossary_excluded: loaded.artifact.glossary_excluded(),
                    source: loaded.source,
                });
            }
            self.finish_namespace(&directory, operation, kind)?;
        }
        Ok(CompleteTemplateScan {
            project: self.identity.clone(),
            records,
        })
    }

    pub(crate) fn scan_documents(&self) -> Result<CompleteDocumentScan, RepositoryError> {
        #[cfg(test)]
        test_support::scan(RepositoryOperation::ScanDocuments);
        let operation = RepositoryOperation::ScanDocuments;
        let kind = ArtifactType::Document;
        let mut records = Vec::new();
        if let Some(directory) = self.namespace(kind, operation)? {
            for id in self.entries(&directory, kind, operation)? {
                let ArtifactSourceId::Document(id) = id else {
                    unreachable!("entry kind is chosen by the scanner")
                };
                progress::checkpoint(false)?;
                let loaded = self.read_document(id, &directory, operation, true)?;
                progress::checkpoint(true)?;
                records.push(DocumentRecord {
                    template_id: loaded.artifact.template_id(),
                    name: loaded.artifact.name().into(),
                    english_name: loaded.artifact.english_name().into(),
                    glossary_summary: loaded.artifact.glossary_summary().into(),
                    glossary_excluded: loaded.artifact.glossary_excluded(),
                    source: loaded.source,
                });
            }
            self.finish_namespace(&directory, operation, kind)?;
        }
        progress::checkpoint(false)?;
        // iterator 정상 종료 + 모든 source/codec admission + guard 확인 뒤에만 constructor에 도달한다.
        Ok(CompleteDocumentScan {
            project: self.identity.clone(),
            records,
        })
    }

    /// 검색 같은 파생 읽기가 한 문서씩 처리하고 즉시 해제할 수 있게 한다. 전체 namespace의
    /// codec admission과 방문이 끝난 경우에만 결과를 반환하며, 쓰기 completeness 증거는 아니다.
    pub(crate) fn scan_documents_with<T, E>(
        &self,
        mut visitor: impl FnMut(&DocumentArtifact) -> Result<T, E>,
    ) -> Result<Vec<T>, DocumentScanVisitError<E>> {
        #[cfg(test)]
        let scan_started = std::time::Instant::now();
        #[cfg(test)]
        let mut read_ns = 0_u128;
        #[cfg(test)]
        let mut visit_ns = 0_u128;
        #[cfg(test)]
        test_support::scan(RepositoryOperation::ScanDocuments);
        let operation = RepositoryOperation::ScanDocuments;
        let kind = ArtifactType::Document;
        let mut values = Vec::new();
        let directory = self
            .namespace(kind, operation)
            .map_err(DocumentScanVisitError::Repository)?;
        if let Some(directory) = directory {
            let ids = self
                .entries(&directory, kind, operation)
                .map_err(DocumentScanVisitError::Repository)?;
            #[cfg(test)]
            let enumeration_ns = scan_started.elapsed().as_nanos();
            // A cached namespace is faster with serial reads. Sample a bounded
            // prefix before choosing parallel I/O for a slow namespace.
            let prefix_len = ids.len().min(64);
            let prefix_started = std::time::Instant::now();
            for source in &ids[..prefix_len] {
                let ArtifactSourceId::Document(id) = *source else {
                    unreachable!("entry kind is chosen by the scanner")
                };
                progress::checkpoint(false).map_err(DocumentScanVisitError::Repository)?;
                #[cfg(test)]
                let read_started = std::time::Instant::now();
                let loaded = self
                    .read_document(id, &directory, operation, true)
                    .map_err(DocumentScanVisitError::Repository)?;
                #[cfg(test)]
                {
                    read_ns += read_started.elapsed().as_nanos();
                }
                #[cfg(test)]
                let visit_started = std::time::Instant::now();
                values.push(visitor(loaded.artifact()).map_err(DocumentScanVisitError::Visitor)?);
                #[cfg(test)]
                {
                    visit_ns += visit_started.elapsed().as_nanos();
                }
                progress::checkpoint(true).map_err(DocumentScanVisitError::Repository)?;
            }
            let prefix_elapsed = prefix_started.elapsed();
            let parallel =
                ids.len() > prefix_len && prefix_elapsed >= std::time::Duration::from_millis(500);
            // For a large projection, keep at most one bounded batch of strict
            // reads in flight. Visitors still run in namespace order on this
            // worker, and no result is published before the final guard check.
            if parallel {
                for batch in ids[prefix_len..].chunks(8) {
                    progress::checkpoint(false).map_err(DocumentScanVisitError::Repository)?;
                    #[cfg(test)]
                    let read_started = std::time::Instant::now();
                    let loaded = std::thread::scope(|scope| {
                        let guarded_directory = &directory;
                        let handles = batch
                            .iter()
                            .map(|source| {
                                let ArtifactSourceId::Document(id) = *source else {
                                    unreachable!("entry kind is chosen by the scanner")
                                };
                                scope.spawn(move || {
                                    self.read_document(id, guarded_directory, operation, true)
                                })
                            })
                            .collect::<Vec<_>>();
                        handles
                            .into_iter()
                            .map(|handle| {
                                handle.join().unwrap_or_else(|_| {
                                    Err(RepositoryError::new(
                                        RepositoryCategory::ReadFailed,
                                        operation,
                                        RepositoryStage::ReadFile,
                                        Some(kind),
                                    ))
                                })
                            })
                            .collect::<Vec<_>>()
                    });
                    #[cfg(test)]
                    {
                        read_ns += read_started.elapsed().as_nanos();
                    }
                    for result in loaded {
                        let loaded = result.map_err(DocumentScanVisitError::Repository)?;
                        #[cfg(test)]
                        let visit_started = std::time::Instant::now();
                        values.push(
                            visitor(loaded.artifact()).map_err(DocumentScanVisitError::Visitor)?,
                        );
                        #[cfg(test)]
                        {
                            visit_ns += visit_started.elapsed().as_nanos();
                        }
                        progress::checkpoint(true).map_err(DocumentScanVisitError::Repository)?;
                    }
                }
            } else {
                for source in &ids[prefix_len..] {
                    let ArtifactSourceId::Document(id) = *source else {
                        unreachable!("entry kind is chosen by the scanner")
                    };
                    progress::checkpoint(false).map_err(DocumentScanVisitError::Repository)?;
                    #[cfg(test)]
                    let read_started = std::time::Instant::now();
                    let loaded = self
                        .read_document(id, &directory, operation, true)
                        .map_err(DocumentScanVisitError::Repository)?;
                    #[cfg(test)]
                    {
                        read_ns += read_started.elapsed().as_nanos();
                    }
                    #[cfg(test)]
                    let visit_started = std::time::Instant::now();
                    values
                        .push(visitor(loaded.artifact()).map_err(DocumentScanVisitError::Visitor)?);
                    #[cfg(test)]
                    {
                        visit_ns += visit_started.elapsed().as_nanos();
                    }
                    progress::checkpoint(true).map_err(DocumentScanVisitError::Repository)?;
                }
            }
            self.finish_namespace(&directory, operation, kind)
                .map_err(DocumentScanVisitError::Repository)?;
            #[cfg(test)]
            if std::env::var_os("M56_SEARCH_TIMING").is_some() {
                println!(
                    "M56_SEARCH_PHASE {}",
                    serde_json::json!({
                        "enumeration_ms": enumeration_ns as f64 / 1_000_000.0,
                        "read_ms": read_ns as f64 / 1_000_000.0,
                        "projection_ms": visit_ns as f64 / 1_000_000.0,
                        "scan_total_ms": scan_started.elapsed().as_secs_f64() * 1000.0,
                        "documents": values.len(),
                        "prefix_ms": prefix_elapsed.as_secs_f64() * 1000.0,
                        "parallel_batch": parallel
                    })
                );
            }
        }
        progress::checkpoint(false).map_err(DocumentScanVisitError::Repository)?;
        Ok(values)
    }

    fn entries(
        &self,
        directory: &ProjectDirectory,
        kind: ArtifactType,
        operation: RepositoryOperation,
    ) -> Result<Vec<ArtifactSourceId>, RepositoryError> {
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        #[cfg(test)]
        test_support::point(
            RepositoryStage::ReadDirectory,
            self.access.locked_project().canonical_root(),
        )
        .map_err(|e| {
            RepositoryError::io(
                RepositoryCategory::IterationFailed,
                operation,
                RepositoryStage::ReadDirectory,
                Some(kind),
                e,
            )
        })?;
        let entries = directory.read_dir().map_err(|e| {
            RepositoryError::io(
                RepositoryCategory::IterationFailed,
                operation,
                RepositoryStage::ReadDirectory,
                Some(kind),
                e,
            )
        })?;
        for entry in entries {
            progress::checkpoint(false)?;
            #[cfg(test)]
            test_support::point(
                RepositoryStage::Iterate,
                self.access.locked_project().canonical_root(),
            )
            .map_err(|e| {
                RepositoryError::io(
                    RepositoryCategory::IterationFailed,
                    operation,
                    RepositoryStage::Iterate,
                    Some(kind),
                    e,
                )
            })?;
            let entry = entry.map_err(|e| {
                RepositoryError::io(
                    RepositoryCategory::IterationFailed,
                    operation,
                    RepositoryStage::Iterate,
                    Some(kind),
                    e,
                )
            })?;
            let name = entry.file_name();
            if kind == ArtifactType::Document
                && name.to_str().is_some_and(svn_document_conflict_sidecar)
            {
                continue;
            }
            self.admit_name(name.clone(), kind, operation, &mut ids, &mut paths)?;
            #[cfg(test)]
            if test_support::repeat_entry() {
                self.admit_name(name, kind, operation, &mut ids, &mut paths)?;
            }
        }
        self.finish_namespace(directory, operation, kind)?;
        Ok(ids.into_iter().collect())
    }

    fn admit_name(
        &self,
        name: OsString,
        kind: ArtifactType,
        operation: RepositoryOperation,
        ids: &mut BTreeSet<ArtifactSourceId>,
        paths: &mut BTreeSet<ProjectRelativePath>,
    ) -> Result<(), RepositoryError> {
        let invalid = || {
            RepositoryError::new(
                RepositoryCategory::InvalidEntry,
                operation,
                RepositoryStage::EntryName,
                Some(kind),
            )
        };
        let name = name.to_str().ok_or_else(invalid)?;
        let raw_id = name.strip_suffix(".json").ok_or_else(invalid)?;
        let id = match kind {
            ArtifactType::DocumentLayout => return Err(invalid()),
            ArtifactType::Template => {
                ArtifactSourceId::Template(raw_id.parse().map_err(|_| invalid())?)
            }
            ArtifactType::Document => {
                ArtifactSourceId::Document(raw_id.parse().map_err(|_| invalid())?)
            }
        };
        let path = id.path().map_err(|_| invalid())?;
        if !paths.insert(path) {
            return Err(RepositoryError::new(
                RepositoryCategory::DuplicateSource,
                operation,
                RepositoryStage::BindSource,
                Some(kind),
            ));
        }
        if !ids.insert(id) {
            return Err(RepositoryError::new(
                RepositoryCategory::DuplicateId,
                operation,
                RepositoryStage::BindSource,
                Some(kind),
            ));
        }
        Ok(())
    }
}
