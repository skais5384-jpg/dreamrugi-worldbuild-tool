//! 프로젝트 worker와 분리된 앱 로컬 설정 작업이다.
use std::sync::Arc;

use crate::{
    commands::{
        backend::Completed,
        dto::{Code, ErrorDiagnosticDto, ErrorDto, ResultDto, SettingsWriteOutcomeDto, Work},
    },
    data::atomic_file::{SaveError, SaveOutcome},
    data::project_settings::{ProjectSettings, SettingsError},
};

pub(crate) fn is_local(input: &Work) -> bool {
    matches!(
        input,
        Work::ProjectSettingsRead {} | Work::ProjectSettingsWrite { .. }
    )
}

pub(crate) fn run(input: Arc<Work>, settings: Arc<ProjectSettings>) -> Completed {
    match &*input {
        Work::ProjectSettingsRead {} => {
            let original = settings.read();
            let dto = match &original {
                Ok(default_root) => Ok(ResultDto::ProjectSettings {
                    default_root: default_root.clone(),
                }),
                Err(SettingsError::Read(_) | SettingsError::Invalid | SettingsError::Write(_)) => {
                    Ok(ResultDto::Rejected {
                        error: Code::SettingsReadFailed.into(),
                        input_retained: false,
                    })
                }
            };
            Completed::new(original, dto)
        }
        Work::ProjectSettingsWrite { default_root } => {
            let write = settings.write(default_root.clone());
            let readback = write.as_ref().err().map(|_| settings.read());
            let dto = match &write {
                Ok(default_root) => ResultDto::ProjectSettingsWrite {
                    outcome: SettingsWriteOutcomeDto::Applied,
                    observed: true,
                    default_root: default_root.clone(),
                    error: None,
                    readback_error: None,
                },
                Err(error) => {
                    let outcome = write_outcome(error);
                    let (observed, default_root, readback_error) = match readback
                        .as_ref()
                        .expect("failed settings write has readback")
                    {
                        Ok(default_root) => (true, default_root.clone(), None),
                        Err(_) => (false, None, Some(Code::SettingsReadFailed.into())),
                    };
                    ResultDto::ProjectSettingsWrite {
                        outcome,
                        observed,
                        default_root,
                        error: Some(settings_write_error(error, outcome)),
                        readback_error,
                    }
                }
            };
            // transport 인수 전까지 최초 write와 부가 readback 원 결과를 함께 소유한다.
            Completed::new((write, readback), Ok(dto))
        }
        _ => Completed::new(
            input,
            Ok(ResultDto::Rejected {
                error: Code::InvalidInput.into(),
                input_retained: false,
            }),
        ),
    }
}

fn write_outcome(error: &SettingsError) -> SettingsWriteOutcomeDto {
    match error {
        SettingsError::Write(SaveError::AtomicWrite(error))
            if error.outcome == SaveOutcome::AppliedDurabilityUncertain =>
        {
            SettingsWriteOutcomeDto::AppliedDurabilityUncertain
        }
        SettingsError::Read(_) | SettingsError::Invalid | SettingsError::Write(_) => {
            SettingsWriteOutcomeDto::NotApplied
        }
    }
}

fn settings_write_error(error: &SettingsError, outcome: SettingsWriteOutcomeDto) -> ErrorDto {
    let (stage, category, io_kind, os_code, cleanup_io_kind, cleanup_os_code) = match error {
        SettingsError::Read(source) => (
            "prepare".to_owned(),
            "io".to_owned(),
            Some(format!("{:?}", source.kind())),
            source.raw_os_error(),
            None,
            None,
        ),
        SettingsError::Invalid => (
            "validate".to_owned(),
            "invalid".to_owned(),
            None,
            None,
            None,
            None,
        ),
        SettingsError::Write(SaveError::Serialize(_)) => (
            "serialize".to_owned(),
            "serialization".to_owned(),
            None,
            None,
            None,
            None,
        ),
        SettingsError::Write(SaveError::AtomicWrite(source)) => (
            format!("{:?}", source.stage),
            "io".to_owned(),
            Some(format!("{:?}", source.source.kind())),
            source.source.raw_os_error(),
            source
                .cleanup_error
                .as_ref()
                .map(|error| format!("{:?}", error.kind())),
            source
                .cleanup_error
                .as_ref()
                .and_then(std::io::Error::raw_os_error),
        ),
    };
    ErrorDto {
        code: Code::SettingsWriteFailed,
        next_action: "현재 설정을 다시 읽고 결과를 확인한 뒤 저장을 명시적으로 다시 시도하세요",
        diagnostic: Some(ErrorDiagnosticDto {
            stage,
            category,
            outcome: Some(format!("{outcome:?}")),
            io_kind,
            os_code,
            cleanup_outcome: cleanup_io_kind.as_ref().map(|_| "failed".to_owned()),
            cleanup_io_kind,
            cleanup_os_code,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::project_settings::TestWriteFault;
    use std::{
        fs, io,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> io::Result<Self> {
            for _ in 0..128 {
                let path = std::env::temp_dir().join(format!(
                    "worldbuild-settings-backend-{}-{}",
                    std::process::id(),
                    COUNTER.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Ok(Self(path)),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "test collision",
            ))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn execute(settings: ProjectSettings, requested: Option<String>) -> ResultDto {
        run(
            Arc::new(Work::ProjectSettingsWrite {
                default_root: requested,
            }),
            Arc::new(settings),
        )
        .dto
        .expect("settings DTO")
    }

    fn projects(temp: &Temp) -> io::Result<(String, String)> {
        let old = temp.0.join("old");
        let new = temp.0.join("new");
        fs::create_dir(&old)?;
        fs::create_dir(&new)?;
        Ok((
            old.to_string_lossy().into_owned(),
            new.to_string_lossy().into_owned(),
        ))
    }

    #[test]
    fn settings_set_and_clear_not_applied_keep_observed_file(
    ) -> Result<(), Box<dyn std::error::Error>> {
        for clear in [false, true] {
            let temp = Temp::new()?;
            let directory = temp.0.join("settings");
            let (old, new) = projects(&temp)?;
            let seed = ProjectSettings::new(directory.clone());
            let previous = seed.write(Some(old))?;
            let result = execute(
                ProjectSettings::with_test_write_fault(
                    directory.clone(),
                    TestWriteFault::NotApplied,
                ),
                (!clear).then_some(new),
            );
            let ResultDto::ProjectSettingsWrite {
                outcome,
                observed,
                default_root,
                error,
                readback_error,
            } = result
            else {
                panic!("expected typed settings write result")
            };
            assert_eq!(outcome, SettingsWriteOutcomeDto::NotApplied);
            assert!(observed);
            assert_eq!(default_root, previous);
            let diagnostic = error.unwrap().diagnostic.unwrap();
            assert_eq!(diagnostic.outcome.as_deref(), Some("NotApplied"));
            assert!(readback_error.is_none());
            assert_eq!(seed.read()?, previous);
        }
        Ok(())
    }

    #[test]
    fn settings_set_and_clear_uncertain_use_actual_readback(
    ) -> Result<(), Box<dyn std::error::Error>> {
        for clear in [false, true] {
            let temp = Temp::new()?;
            let directory = temp.0.join("settings");
            let (old, new) = projects(&temp)?;
            ProjectSettings::new(directory.clone()).write(Some(old))?;
            let requested = (!clear).then_some(new);
            let result = execute(
                ProjectSettings::with_test_write_fault(
                    directory.clone(),
                    TestWriteFault::AppliedDurabilityUncertain,
                ),
                requested,
            );
            let ResultDto::ProjectSettingsWrite {
                outcome,
                observed,
                default_root,
                error,
                readback_error,
            } = result
            else {
                panic!("expected typed settings write result")
            };
            assert_eq!(outcome, SettingsWriteOutcomeDto::AppliedDurabilityUncertain);
            assert!(observed);
            let diagnostic = error.unwrap().diagnostic.unwrap();
            assert_eq!(
                diagnostic.outcome.as_deref(),
                Some("AppliedDurabilityUncertain")
            );
            assert!(readback_error.is_none());
            assert_eq!(ProjectSettings::new(directory).read()?, default_root);
            assert_eq!(default_root.is_none(), clear);
        }
        Ok(())
    }

    #[test]
    fn uncertain_set_and_clear_preserve_write_and_readback_failures(
    ) -> Result<(), Box<dyn std::error::Error>> {
        for clear in [false, true] {
            let temp = Temp::new()?;
            let directory = temp.0.join("settings");
            let (old, new) = projects(&temp)?;
            ProjectSettings::new(directory.clone()).write(Some(old))?;
            let result = execute(
                ProjectSettings::with_test_write_fault(
                    directory.clone(),
                    TestWriteFault::AppliedDurabilityUncertainUnreadable,
                ),
                (!clear).then_some(new),
            );
            let ResultDto::ProjectSettingsWrite {
                outcome,
                observed,
                default_root,
                error,
                readback_error,
            } = result
            else {
                panic!("expected typed settings write result")
            };
            assert_eq!(outcome, SettingsWriteOutcomeDto::AppliedDurabilityUncertain);
            assert!(!observed);
            assert!(default_root.is_none());
            let diagnostic = error.unwrap().diagnostic.unwrap();
            assert_eq!(
                diagnostic.outcome.as_deref(),
                Some("AppliedDurabilityUncertain")
            );
            assert!(readback_error.is_some());
            assert!(matches!(
                ProjectSettings::new(directory).read(),
                Err(SettingsError::Invalid)
            ));
        }
        Ok(())
    }
}
