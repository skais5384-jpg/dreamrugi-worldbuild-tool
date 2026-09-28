use super::*;

/// 값 슬롯과 별개인 Template 소유 제목. 확장 정보는 같은 ID를 편집할 때 보존한다.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Section {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) before_field: Option<FieldId>,
    #[serde(flatten)]
    extra: ExtraFields,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SectionInput {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) before_field: Option<String>,
}
impl Section {
    pub(crate) fn input(&self) -> SectionInput {
        SectionInput {
            id: self.id.clone(),
            title: self.title.clone(),
            before_field: self.before_field.map(|f| f.to_string()),
        }
    }
}
pub(super) fn assemble(
    source: &[Section],
    inputs: Vec<SectionInput>,
) -> Result<Vec<Section>, ArtifactValidationError> {
    inputs
        .into_iter()
        .map(|input| {
            Ok(Section {
                before_field: input
                    .before_field
                    .map(|s| {
                        s.parse()
                            .map_err(|_| ArtifactValidationError::field_order_mismatch())
                    })
                    .transpose()?,
                extra: source
                    .iter()
                    .find(|s| s.id == input.id)
                    .map(|s| s.extra.clone())
                    .unwrap_or_default(),
                id: input.id,
                title: input.title,
            })
        })
        .collect()
}
pub(super) fn validate(
    sections: &[Section],
    fields: &[FieldId],
) -> Result<(), ArtifactValidationError> {
    let mut seen = BTreeSet::new();
    for section in sections {
        if !uuid::Uuid::parse_str(&section.id)
            .is_ok_and(|id| !id.is_nil() && id.to_string() == section.id)
            || !seen.insert(&section.id)
            || section.title.trim().is_empty()
            || section.before_field.is_some_and(|id| !fields.contains(&id))
        {
            return Err(ArtifactValidationError::field_order_mismatch());
        }
        validate_extra_keys(&section.extra, &["id", "title", "beforeField"])?;
    }
    Ok(())
}
