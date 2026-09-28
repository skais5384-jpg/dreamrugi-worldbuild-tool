use super::*;
#[test]
fn b003_owned_space_exceeds_legacy_budget_for_large_staged_payload() {
    use test_support::{with_storage_response, TestStorageResponse};
    for unit in [512, 4096, 65536] {
        for backup in [None, Some(17)] {
            let stage = 64 * 1024 * 1024 + 7;
            let input = TransactionStorageInput {
                staged_sizes: &[stage],
                backup_sizes: &[backup],
                target_name_bytes: &[51],
                minimum_required_bytes: 0,
            };
            let legacy = estimate_peak(unit, input).unwrap().required_peak_bytes;
            let available = legacy + stage / 2;
            let (normal, _) = with_storage_response(
                TestStorageResponse::Available {
                    available_bytes: available,
                    allocation_unit_bytes: unit,
                },
                || admit_transaction_storage(Path::new("journal"), &[], input),
            );
            assert!(normal.is_ok());
            let (owned, _) = with_storage_response(
                TestStorageResponse::Available {
                    available_bytes: available,
                    allocation_unit_bytes: unit,
                },
                || admit_owned_transaction_storage(Path::new("journal"), &[], input),
            );
            let Err(StorageAdmissionError::Insufficient(admission)) = owned else {
                panic!("owned replacement must reserve an additional large file");
            };
            assert!(admission.required_peak_bytes() >= legacy + stage);
            // stages가 남은 상태의 새 target과 임시 파일을 기존 largest-backup reserve로 숨기지 않는다.
            println!("B003_STORAGE unit={unit} staged={stage} backup={backup:?} legacy={legacy} available={available} owned={}",admission.required_peak_bytes());
        }
    }
}
