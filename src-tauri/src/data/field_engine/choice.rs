use std::{
    collections::{BTreeMap, HashSet},
    fmt,
};

use crate::data::artifact::OptionId;

/// Choice 판정에 필요한 Option의 최소 수명주기 view다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChoiceOptionLifecycle {
    Active,
    Archived,
}

/// 같은 선택 값도 생성 시점과 보존 목적에 따라 archived Option 정책이 다르다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChoiceMembershipContext {
    ActiveFieldCurrentDefault,
    HistoricalInitialDefault,
    ArchivedFieldPreservedDefault,
    NewDocumentOrNewSelection,
    ExistingDocumentValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChoiceValidationErrorCategory {
    EmptyMultiChoiceMustUseUnset,
    DuplicateSelectedOption,
    NonCanonicalSelectedOptionOrder,
    UnknownSelectedOption,
    ArchivedOptionNotSelectable,
    DuplicateOptionIdInLookup,
}

/// label이나 전체 선택 배열을 소유하지 않는 안전한 choice 오류다.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChoiceValidationError {
    category: ChoiceValidationErrorCategory,
    option_id: Option<OptionId>,
    detail: &'static str,
}

impl ChoiceValidationError {
    pub(crate) const fn category(self) -> ChoiceValidationErrorCategory {
        self.category
    }

    pub(crate) const fn option_id(self) -> Option<OptionId> {
        self.option_id
    }

    const fn new(
        category: ChoiceValidationErrorCategory,
        option_id: Option<OptionId>,
        detail: &'static str,
    ) -> Self {
        Self {
            category,
            option_id,
            detail,
        }
    }

    const fn empty_multi_choice() -> Self {
        Self::new(
            ChoiceValidationErrorCategory::EmptyMultiChoiceMustUseUnset,
            None,
            "empty multi-choice selection is noncanonical and must use unset",
        )
    }

    const fn duplicate_selected_option(option_id: OptionId) -> Self {
        Self::new(
            ChoiceValidationErrorCategory::DuplicateSelectedOption,
            Some(option_id),
            "multi-choice selection contains a duplicate OptionId",
        )
    }

    const fn noncanonical_selected_option_order(option_id: OptionId) -> Self {
        Self::new(
            ChoiceValidationErrorCategory::NonCanonicalSelectedOptionOrder,
            Some(option_id),
            "multi-choice selection must use strictly increasing OptionId order",
        )
    }

    const fn unknown_selected_option(option_id: OptionId) -> Self {
        Self::new(
            ChoiceValidationErrorCategory::UnknownSelectedOption,
            Some(option_id),
            "selected OptionId is not present in the choice lookup",
        )
    }

    const fn archived_option_not_selectable(option_id: OptionId) -> Self {
        Self::new(
            ChoiceValidationErrorCategory::ArchivedOptionNotSelectable,
            Some(option_id),
            "archived OptionId cannot be used for a current or new selection",
        )
    }

    const fn duplicate_lookup_option(option_id: OptionId) -> Self {
        Self::new(
            ChoiceValidationErrorCategory::DuplicateOptionIdInLookup,
            Some(option_id),
            "choice lookup contains a duplicate OptionId",
        )
    }
}

impl fmt::Debug for ChoiceValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChoiceValidationError")
            .field("category", &self.category)
            .field("option_id", &self.option_id)
            .field("detail", &self.detail)
            .finish()
    }
}

impl fmt::Display for ChoiceValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "choice validation failed ({:?}): {}",
            self.category, self.detail
        )?;
        if let Some(option_id) = self.option_id {
            write!(formatter, " (OptionId {option_id})")?;
        }
        Ok(())
    }
}

impl std::error::Error for ChoiceValidationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChoiceMembershipDiagnosticCategory {
    ExistingSelectionContainsArchivedOption,
}

/// 기존 Document의 archived 선택은 손상이 아니라 보존 가능한 구조화 진단이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChoiceMembershipDiagnostic {
    category: ChoiceMembershipDiagnosticCategory,
    archived_option_count: usize,
}

impl ChoiceMembershipDiagnostic {
    pub(crate) const fn category(self) -> ChoiceMembershipDiagnosticCategory {
        self.category
    }

    pub(crate) const fn archived_option_count(self) -> usize {
        self.archived_option_count
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChoiceMembershipOutcome {
    Valid,
    ValidWithDiagnostic(ChoiceMembershipDiagnostic),
}

/// Choice engine은 label, optionOrder, 전체 definition을 보지 않고 이 immutable lookup만 사용한다.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ChoiceOptionLookup {
    options: BTreeMap<OptionId, ChoiceOptionLifecycle>,
}

impl ChoiceOptionLookup {
    pub(crate) fn try_from_options(
        options: impl IntoIterator<Item = (OptionId, ChoiceOptionLifecycle)>,
    ) -> Result<Self, ChoiceValidationError> {
        let mut lookup = BTreeMap::new();
        for (option_id, lifecycle) in options {
            if lookup.insert(option_id, lifecycle).is_some() {
                return Err(ChoiceValidationError::duplicate_lookup_option(option_id));
            }
        }
        Ok(Self { options: lookup })
    }

    fn lifecycle(&self, option_id: OptionId) -> Option<ChoiceOptionLifecycle> {
        self.options.get(&option_id).copied()
    }
}

impl fmt::Debug for ChoiceOptionLookup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChoiceOptionLookup")
            .field("option_count", &self.options.len())
            .finish()
    }
}

/// Persistent membership 판정 입력이다. Multi variant는 먼저 canonical 검증을 받는다.
#[derive(Clone, Copy)]
pub(crate) enum ChoiceSelection<'a> {
    Unset,
    Single(OptionId),
    Multi(&'a [OptionId]),
}

/// 정규화된 selected variant가 빈 Vec을 가질 수 없도록 생성 경로를 이 모듈에 가둔다.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CanonicalSelectedOptionIds {
    option_ids: Vec<OptionId>,
}

impl CanonicalSelectedOptionIds {
    pub(crate) fn as_slice(&self) -> &[OptionId] {
        &self.option_ids
    }
}

impl fmt::Debug for CanonicalSelectedOptionIds {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalSelectedOptionIds")
            .field("option_count", &self.option_ids.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NormalizedMultiChoice {
    Unset,
    Selected(CanonicalSelectedOptionIds),
}

/// 편집 입력은 정렬·중복 제거하고, 빈 결과를 유일한 canonical empty인 unset으로 바꾼다.
pub(crate) fn normalize_multi_choice(
    option_ids: impl IntoIterator<Item = OptionId>,
) -> NormalizedMultiChoice {
    let mut option_ids: Vec<_> = option_ids.into_iter().collect();
    option_ids.sort_unstable();
    option_ids.dedup();
    if option_ids.is_empty() {
        NormalizedMultiChoice::Unset
    } else {
        NormalizedMultiChoice::Selected(CanonicalSelectedOptionIds { option_ids })
    }
}

/// 저장된 multi-choice 배열을 변경하지 않고 sorted unique non-empty인지 O(n)에 확인한다.
pub(crate) fn validate_persistent_multi_choice(
    option_ids: &[OptionId],
) -> Result<(), ChoiceValidationError> {
    if option_ids.is_empty() {
        return Err(ChoiceValidationError::empty_multi_choice());
    }

    let mut seen = HashSet::with_capacity(option_ids.len());
    let mut duplicate = None;
    let mut first_out_of_order = None;
    let mut previous = None;
    for option_id in option_ids.iter().copied() {
        if !seen.insert(option_id) && duplicate.is_none() {
            duplicate = Some(option_id);
        }
        if previous.is_some_and(|previous| previous >= option_id) && first_out_of_order.is_none() {
            first_out_of_order = Some(option_id);
        }
        previous = Some(option_id);
    }

    if let Some(option_id) = duplicate {
        return Err(ChoiceValidationError::duplicate_selected_option(option_id));
    }
    if let Some(option_id) = first_out_of_order {
        return Err(ChoiceValidationError::noncanonical_selected_option_order(
            option_id,
        ));
    }
    Ok(())
}

/// Persistent shape와 context별 membership을 함께 판정하되 어떤 입력도 수정하지 않는다.
pub(crate) fn validate_choice_membership(
    selection: ChoiceSelection<'_>,
    lookup: &ChoiceOptionLookup,
    context: ChoiceMembershipContext,
) -> Result<ChoiceMembershipOutcome, ChoiceValidationError> {
    let selected: &[OptionId] = match selection {
        ChoiceSelection::Unset => return Ok(ChoiceMembershipOutcome::Valid),
        ChoiceSelection::Single(ref option_id) => std::slice::from_ref(option_id),
        ChoiceSelection::Multi(option_ids) => {
            validate_persistent_multi_choice(option_ids)?;
            option_ids
        }
    };

    let archived_policy = match context {
        ChoiceMembershipContext::ActiveFieldCurrentDefault
        | ChoiceMembershipContext::NewDocumentOrNewSelection => ArchivedOptionPolicy::Reject,
        ChoiceMembershipContext::HistoricalInitialDefault
        | ChoiceMembershipContext::ArchivedFieldPreservedDefault => ArchivedOptionPolicy::Allow,
        ChoiceMembershipContext::ExistingDocumentValue => ArchivedOptionPolicy::Diagnose,
    };

    // archived와 unknown이 함께 있을 때 archived 진단/거부가 unknown 손상을 가리지 않게
    // membership 존재 여부를 먼저 끝까지 확인한다.
    for option_id in selected.iter().copied() {
        if lookup.lifecycle(option_id).is_none() {
            return Err(ChoiceValidationError::unknown_selected_option(option_id));
        }
    }

    let mut archived_option_count = 0usize;
    for option_id in selected.iter().copied() {
        match lookup.lifecycle(option_id) {
            None => return Err(ChoiceValidationError::unknown_selected_option(option_id)),
            Some(ChoiceOptionLifecycle::Active) => {}
            Some(ChoiceOptionLifecycle::Archived) => match archived_policy {
                ArchivedOptionPolicy::Reject => {
                    return Err(ChoiceValidationError::archived_option_not_selectable(
                        option_id,
                    ));
                }
                ArchivedOptionPolicy::Allow => {}
                ArchivedOptionPolicy::Diagnose => {
                    archived_option_count = archived_option_count.saturating_add(1);
                }
            },
        }
    }

    if archived_option_count == 0 {
        Ok(ChoiceMembershipOutcome::Valid)
    } else {
        Ok(ChoiceMembershipOutcome::ValidWithDiagnostic(
            ChoiceMembershipDiagnostic {
                category:
                    ChoiceMembershipDiagnosticCategory::ExistingSelectionContainsArchivedOption,
                archived_option_count,
            },
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchivedOptionPolicy {
    Reject,
    Allow,
    Diagnose,
}

#[cfg(test)]
mod tests {
    use std::{error::Error, str::FromStr};

    use super::*;

    const ACTIVE: &str = "11111111-1111-4111-8111-111111111111";
    const ACTIVE_SECOND: &str = "12121212-1212-4212-8212-121212121212";
    const UNKNOWN_BEFORE: &str = "00000000-0000-4000-8000-000000000001";
    const UNKNOWN_MIDDLE: &str = "20202020-2020-4020-8020-202020202020";
    const ARCHIVED: &str = "22222222-2222-4222-8222-222222222222";
    const ARCHIVED_SECOND: &str = "23232323-2323-4323-8323-232323232323";
    const UNKNOWN: &str = "33333333-3333-4333-8333-333333333333";

    fn option(value: &str) -> OptionId {
        OptionId::from_str(value).expect("test OptionId should be canonical UUID v4")
    }

    fn lookup() -> ChoiceOptionLookup {
        ChoiceOptionLookup::try_from_options([
            (option(ACTIVE), ChoiceOptionLifecycle::Active),
            (option(ACTIVE_SECOND), ChoiceOptionLifecycle::Active),
            (option(ARCHIVED), ChoiceOptionLifecycle::Archived),
            (option(ARCHIVED_SECOND), ChoiceOptionLifecycle::Archived),
        ])
        .expect("fixture lookup should be unique")
    }

    #[test]
    fn persistent_multi_choice_accepts_only_sorted_unique_non_empty_ids() {
        let active = option(ACTIVE);
        let archived = option(ARCHIVED);
        validate_persistent_multi_choice(&[active]).unwrap();
        validate_persistent_multi_choice(&[active, archived]).unwrap();

        assert_eq!(
            validate_persistent_multi_choice(&[])
                .unwrap_err()
                .category(),
            ChoiceValidationErrorCategory::EmptyMultiChoiceMustUseUnset
        );
        assert_eq!(
            validate_persistent_multi_choice(&[active, active])
                .unwrap_err()
                .category(),
            ChoiceValidationErrorCategory::DuplicateSelectedOption
        );
        assert_eq!(
            validate_persistent_multi_choice(&[archived, active])
                .unwrap_err()
                .category(),
            ChoiceValidationErrorCategory::NonCanonicalSelectedOptionOrder
        );
        assert_eq!(
            validate_persistent_multi_choice(&[active, option(UNKNOWN), archived])
                .unwrap_err()
                .category(),
            ChoiceValidationErrorCategory::NonCanonicalSelectedOptionOrder
        );
    }

    #[test]
    fn persistent_multi_choice_detects_non_adjacent_duplicates_without_mutation() {
        let original = [option(ACTIVE), option(UNKNOWN), option(ACTIVE)];
        let before = original;
        assert_eq!(
            validate_persistent_multi_choice(&original)
                .unwrap_err()
                .category(),
            ChoiceValidationErrorCategory::DuplicateSelectedOption
        );
        assert_eq!(original, before);
    }

    #[test]
    fn persistent_multi_choice_handles_large_sorted_input_linearly() {
        let option_ids: Vec<_> = (1..=20_000)
            .map(|index| option(&format!("00000000-0000-4000-8000-{index:012x}")))
            .collect();
        validate_persistent_multi_choice(&option_ids).unwrap();
    }

    #[test]
    fn normalization_sorts_deduplicates_maps_empty_to_unset_and_is_idempotent() {
        let active = option(ACTIVE);
        let archived = option(ARCHIVED);
        assert_eq!(
            normalize_multi_choice(std::iter::empty()),
            NormalizedMultiChoice::Unset
        );

        let normalized = normalize_multi_choice([archived, active, archived]);
        let NormalizedMultiChoice::Selected(selected) = &normalized else {
            panic!("non-empty input should remain selected");
        };
        assert_eq!(selected.as_slice(), &[active, archived]);
        assert_eq!(
            normalize_multi_choice(selected.as_slice().iter().copied()),
            normalized
        );

        let unknown = option(UNKNOWN);
        let NormalizedMultiChoice::Selected(selected) = normalize_multi_choice([unknown, archived])
        else {
            panic!("unknown and archived IDs must not be discarded");
        };
        assert_eq!(selected.as_slice(), &[archived, unknown]);
    }

    #[test]
    fn lookup_rejects_duplicate_ids_without_labels_or_order_data() {
        let active = option(ACTIVE);
        let error = ChoiceOptionLookup::try_from_options([
            (active, ChoiceOptionLifecycle::Active),
            (active, ChoiceOptionLifecycle::Archived),
        ])
        .unwrap_err();
        assert_eq!(
            error.category(),
            ChoiceValidationErrorCategory::DuplicateOptionIdInLookup
        );
        assert_eq!(error.option_id(), Some(active));
    }

    #[test]
    fn active_current_default_accepts_active_and_rejects_archived_or_unknown() {
        let lookup = lookup();
        assert_eq!(
            validate_choice_membership(
                ChoiceSelection::Single(option(ACTIVE)),
                &lookup,
                ChoiceMembershipContext::ActiveFieldCurrentDefault,
            )
            .unwrap(),
            ChoiceMembershipOutcome::Valid
        );
        assert_eq!(
            validate_choice_membership(
                ChoiceSelection::Single(option(ARCHIVED)),
                &lookup,
                ChoiceMembershipContext::ActiveFieldCurrentDefault,
            )
            .unwrap_err()
            .category(),
            ChoiceValidationErrorCategory::ArchivedOptionNotSelectable
        );
        assert_eq!(
            validate_choice_membership(
                ChoiceSelection::Single(option(UNKNOWN)),
                &lookup,
                ChoiceMembershipContext::ActiveFieldCurrentDefault,
            )
            .unwrap_err()
            .category(),
            ChoiceValidationErrorCategory::UnknownSelectedOption
        );
    }

    #[test]
    fn historical_and_archived_field_defaults_preserve_archived_options() {
        let lookup = lookup();
        for context in [
            ChoiceMembershipContext::HistoricalInitialDefault,
            ChoiceMembershipContext::ArchivedFieldPreservedDefault,
        ] {
            for option_id in [option(ACTIVE), option(ARCHIVED)] {
                assert_eq!(
                    validate_choice_membership(
                        ChoiceSelection::Single(option_id),
                        &lookup,
                        context,
                    )
                    .unwrap(),
                    ChoiceMembershipOutcome::Valid
                );
            }
            assert_eq!(
                validate_choice_membership(
                    ChoiceSelection::Single(option(UNKNOWN)),
                    &lookup,
                    context,
                )
                .unwrap_err()
                .category(),
                ChoiceValidationErrorCategory::UnknownSelectedOption
            );
        }
    }

    #[test]
    fn new_selection_rejects_archived_and_normalized_empty_is_unset() {
        let lookup = lookup();
        assert_eq!(
            validate_choice_membership(
                ChoiceSelection::Single(option(ARCHIVED)),
                &lookup,
                ChoiceMembershipContext::NewDocumentOrNewSelection,
            )
            .unwrap_err()
            .category(),
            ChoiceValidationErrorCategory::ArchivedOptionNotSelectable
        );
        assert_eq!(
            validate_choice_membership(
                ChoiceSelection::Unset,
                &lookup,
                ChoiceMembershipContext::NewDocumentOrNewSelection,
            )
            .unwrap(),
            ChoiceMembershipOutcome::Valid
        );
    }

    #[test]
    fn existing_document_reports_archived_and_rejects_unknown_selections() {
        let lookup = lookup();
        let outcome = validate_choice_membership(
            ChoiceSelection::Multi(&[option(ACTIVE), option(ARCHIVED)]),
            &lookup,
            ChoiceMembershipContext::ExistingDocumentValue,
        )
        .unwrap();
        let ChoiceMembershipOutcome::ValidWithDiagnostic(diagnostic) = outcome else {
            panic!("archived existing value should produce a diagnostic");
        };
        assert_eq!(
            diagnostic.category(),
            ChoiceMembershipDiagnosticCategory::ExistingSelectionContainsArchivedOption
        );
        assert_eq!(diagnostic.archived_option_count(), 1);

        assert_eq!(
            validate_choice_membership(
                ChoiceSelection::Single(option(UNKNOWN)),
                &lookup,
                ChoiceMembershipContext::ExistingDocumentValue,
            )
            .unwrap_err()
            .category(),
            ChoiceValidationErrorCategory::UnknownSelectedOption
        );
    }

    #[test]
    fn membership_context_matrix_covers_every_single_selection_policy() {
        #[derive(Clone, Copy)]
        enum ArchivedExpectation {
            Reject,
            Allow,
            Diagnose,
        }

        let lookup = lookup();
        for (context, archived_expectation) in [
            (
                ChoiceMembershipContext::ActiveFieldCurrentDefault,
                ArchivedExpectation::Reject,
            ),
            (
                ChoiceMembershipContext::HistoricalInitialDefault,
                ArchivedExpectation::Allow,
            ),
            (
                ChoiceMembershipContext::ArchivedFieldPreservedDefault,
                ArchivedExpectation::Allow,
            ),
            (
                ChoiceMembershipContext::NewDocumentOrNewSelection,
                ArchivedExpectation::Reject,
            ),
            (
                ChoiceMembershipContext::ExistingDocumentValue,
                ArchivedExpectation::Diagnose,
            ),
        ] {
            assert_eq!(
                validate_choice_membership(ChoiceSelection::Unset, &lookup, context).unwrap(),
                ChoiceMembershipOutcome::Valid
            );
            assert_eq!(
                validate_choice_membership(
                    ChoiceSelection::Single(option(ACTIVE)),
                    &lookup,
                    context,
                )
                .unwrap(),
                ChoiceMembershipOutcome::Valid
            );
            assert_eq!(
                validate_choice_membership(
                    ChoiceSelection::Single(option(UNKNOWN)),
                    &lookup,
                    context,
                )
                .unwrap_err()
                .category(),
                ChoiceValidationErrorCategory::UnknownSelectedOption
            );

            let archived_result = validate_choice_membership(
                ChoiceSelection::Single(option(ARCHIVED)),
                &lookup,
                context,
            );
            match archived_expectation {
                ArchivedExpectation::Reject => assert_eq!(
                    archived_result.unwrap_err().category(),
                    ChoiceValidationErrorCategory::ArchivedOptionNotSelectable
                ),
                ArchivedExpectation::Allow => {
                    assert_eq!(archived_result.unwrap(), ChoiceMembershipOutcome::Valid);
                }
                ArchivedExpectation::Diagnose => {
                    let ChoiceMembershipOutcome::ValidWithDiagnostic(diagnostic) =
                        archived_result.unwrap()
                    else {
                        panic!("existing archived selection must produce a diagnostic");
                    };
                    assert_eq!(
                        diagnostic.category(),
                        ChoiceMembershipDiagnosticCategory::ExistingSelectionContainsArchivedOption
                    );
                    assert_eq!(diagnostic.archived_option_count(), 1);
                }
            }
        }
    }

    #[test]
    fn existing_document_multi_choice_counts_archived_and_never_hides_unknown() {
        let lookup = lookup();
        let active = option(ACTIVE);
        let active_second = option(ACTIVE_SECOND);
        let archived = option(ARCHIVED);
        let archived_second = option(ARCHIVED_SECOND);

        for (selection, expected_archived_count) in [
            (vec![active, active_second], None),
            (vec![archived], Some(1)),
            (vec![archived, archived_second], Some(2)),
            (vec![active, archived], Some(1)),
            (
                vec![active, active_second, archived, archived_second],
                Some(2),
            ),
        ] {
            let outcome = validate_choice_membership(
                ChoiceSelection::Multi(&selection),
                &lookup,
                ChoiceMembershipContext::ExistingDocumentValue,
            )
            .unwrap();
            match expected_archived_count {
                None => assert_eq!(outcome, ChoiceMembershipOutcome::Valid),
                Some(expected_count) => {
                    let ChoiceMembershipOutcome::ValidWithDiagnostic(diagnostic) = outcome else {
                        panic!("archived selections must produce a diagnostic");
                    };
                    assert_eq!(
                        diagnostic.category(),
                        ChoiceMembershipDiagnosticCategory::ExistingSelectionContainsArchivedOption
                    );
                    assert_eq!(diagnostic.archived_option_count(), expected_count);
                    let diagnostic_debug = format!("{diagnostic:?}");
                    for forbidden in [
                        ACTIVE,
                        ACTIVE_SECOND,
                        ARCHIVED,
                        ARCHIVED_SECOND,
                        "private option label",
                        "credential=choice-secret",
                        "C:\\Users\\audit\\choice.json",
                        "/home/audit/choice.json",
                    ] {
                        assert!(!diagnostic_debug.contains(forbidden));
                    }
                }
            }
        }

        for (selection, expected_unknown) in [
            (
                vec![option(UNKNOWN_BEFORE), archived],
                option(UNKNOWN_BEFORE),
            ),
            (
                vec![active, option(UNKNOWN_MIDDLE), archived],
                option(UNKNOWN_MIDDLE),
            ),
            (vec![active, archived, option(UNKNOWN)], option(UNKNOWN)),
        ] {
            let error = validate_choice_membership(
                ChoiceSelection::Multi(&selection),
                &lookup,
                ChoiceMembershipContext::ExistingDocumentValue,
            )
            .unwrap_err();
            assert_eq!(
                error.category(),
                ChoiceValidationErrorCategory::UnknownSelectedOption
            );
            assert_eq!(error.option_id(), Some(expected_unknown));
            assert!(error.source().is_none());
        }
    }

    #[test]
    fn unknown_membership_precedes_archived_rejection_in_current_and_new_contexts() {
        let lookup = lookup();
        let selection = [option(ARCHIVED), option(UNKNOWN)];
        for context in [
            ChoiceMembershipContext::ActiveFieldCurrentDefault,
            ChoiceMembershipContext::NewDocumentOrNewSelection,
        ] {
            let error =
                validate_choice_membership(ChoiceSelection::Multi(&selection), &lookup, context)
                    .unwrap_err();
            assert_eq!(
                error.category(),
                ChoiceValidationErrorCategory::UnknownSelectedOption
            );
            assert_eq!(error.option_id(), Some(option(UNKNOWN)));
        }
    }

    #[test]
    fn unset_is_valid_in_every_membership_context() {
        let lookup = lookup();
        for context in [
            ChoiceMembershipContext::ActiveFieldCurrentDefault,
            ChoiceMembershipContext::HistoricalInitialDefault,
            ChoiceMembershipContext::ArchivedFieldPreservedDefault,
            ChoiceMembershipContext::NewDocumentOrNewSelection,
            ChoiceMembershipContext::ExistingDocumentValue,
        ] {
            assert_eq!(
                validate_choice_membership(ChoiceSelection::Unset, &lookup, context).unwrap(),
                ChoiceMembershipOutcome::Valid
            );
        }
    }

    #[test]
    fn multi_membership_requires_canonical_shape_before_lookup() {
        let lookup = lookup();
        for (ids, category) in [
            (
                Vec::new(),
                ChoiceValidationErrorCategory::EmptyMultiChoiceMustUseUnset,
            ),
            (
                vec![option(ACTIVE), option(ACTIVE)],
                ChoiceValidationErrorCategory::DuplicateSelectedOption,
            ),
            (
                vec![option(ARCHIVED), option(ACTIVE)],
                ChoiceValidationErrorCategory::NonCanonicalSelectedOptionOrder,
            ),
        ] {
            assert_eq!(
                validate_choice_membership(
                    ChoiceSelection::Multi(&ids),
                    &lookup,
                    ChoiceMembershipContext::ExistingDocumentValue,
                )
                .unwrap_err()
                .category(),
                category
            );
        }
    }

    #[test]
    fn validation_is_independent_of_labels_and_template_display_order() {
        let active = option(ACTIVE);
        let archived = option(ARCHIVED);
        let first = ChoiceOptionLookup::try_from_options([
            (active, ChoiceOptionLifecycle::Active),
            (archived, ChoiceOptionLifecycle::Archived),
        ])
        .unwrap();
        let reordered = ChoiceOptionLookup::try_from_options([
            (archived, ChoiceOptionLifecycle::Archived),
            (active, ChoiceOptionLifecycle::Active),
        ])
        .unwrap();
        let selection = [active, archived];
        assert_eq!(
            validate_choice_membership(
                ChoiceSelection::Multi(&selection),
                &first,
                ChoiceMembershipContext::ExistingDocumentValue,
            ),
            validate_choice_membership(
                ChoiceSelection::Multi(&selection),
                &reordered,
                ChoiceMembershipContext::ExistingDocumentValue,
            )
        );
    }

    #[test]
    fn errors_and_diagnostics_do_not_expose_labels_arrays_credentials_or_paths() {
        let lookup = lookup();
        let error = validate_choice_membership(
            ChoiceSelection::Single(option(UNKNOWN)),
            &lookup,
            ChoiceMembershipContext::ExistingDocumentValue,
        )
        .unwrap_err();
        for forbidden in [
            "credential=choice-secret",
            "private option label",
            "C:\\Users\\audit\\choice.json",
            "/home/audit/choice.json",
            &format!("[{ACTIVE}, {ARCHIVED}]"),
        ] {
            assert!(!error.to_string().contains(forbidden));
            assert!(!format!("{error:?}").contains(forbidden));
        }
        assert!(error.source().is_none());
        assert_eq!(error.option_id(), Some(option(UNKNOWN)));
    }
}
