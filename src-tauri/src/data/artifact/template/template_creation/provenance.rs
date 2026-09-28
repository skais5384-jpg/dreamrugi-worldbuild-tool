use std::collections::BTreeMap;

use serde::{de::DeserializeOwned, Serialize};
use serde_json::value::{to_raw_value, RawValue};

use super::{
    FieldId, IdentityMapping, OptionId, TemplateArtifact, TemplateCreationError,
    TemplateCreationErrorCategory, TemplateCreationLocation, TemplateCreationStage,
};
use crate::data::{
    artifact::{encode_template, ArtifactCodecErrorCategory, ArtifactCodecStage},
    json::{parse_strict_lossless_json_object, LosslessJsonValue, StrictJsonErrorCategory},
};

type Object = BTreeMap<String, Box<RawValue>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Failure {
    SourceEncoding(ArtifactCodecErrorCategory, ArtifactCodecStage),
    MissingOwnedMember,
    DecodeOwnedMember(serde_json::error::Category),
    EncodeOwnedMember(serde_json::error::Category),
    StrictSource(StrictJsonErrorCategory),
}

fn failure(location: TemplateCreationLocation, cause: Failure) -> TemplateCreationError {
    let mut error = TemplateCreationError::new(
        TemplateCreationErrorCategory::ProvenanceFailure,
        TemplateCreationStage::Provenance,
        location,
    );
    error.provenance = Some(cause);
    error
}

fn take<T: DeserializeOwned>(
    object: &mut Object,
    key: &'static str,
    location: TemplateCreationLocation,
) -> Result<T, TemplateCreationError> {
    let raw = object
        .remove(key)
        .ok_or_else(|| failure(location, Failure::MissingOwnedMember))?;
    serde_json::from_str(raw.get())
        .map_err(|error| failure(location, Failure::DecodeOwnedMember(error.classify())))
}

fn put<T: Serialize>(
    object: &mut Object,
    key: &'static str,
    value: &T,
    location: TemplateCreationLocation,
) -> Result<(), TemplateCreationError> {
    let raw = to_raw_value(value)
        .map_err(|error| failure(location, Failure::EncodeOwnedMember(error.classify())))?;
    object.insert(key.to_owned(), raw);
    Ok(())
}

/// 이 함수는 외부 JSON을 받지 않는다. 검증된 source의 canonical bytes에서 알려진 소유 map만 옮긴다.
/// 나머지 member는 RawValue 그대로 운반하므로 unknown 숫자/UUID 문자열/AST를 해석하거나 재작성하지 않는다.
/// P1의 source carrier와 JSON API를 확장하지 않고, 완전한 typed mapping으로만 provenance 위치를 바꾼다.
pub(super) fn relocate(
    source: &TemplateArtifact,
    mapping: &IdentityMapping,
) -> Result<LosslessJsonValue, TemplateCreationError> {
    let root_location = TemplateCreationLocation::Template;
    let bytes = encode_template(source).map_err(|error| {
        failure(
            root_location,
            Failure::SourceEncoding(error.category(), error.stage()),
        )
    })?;
    let mut root: Object = serde_json::from_slice(&bytes)
        .map_err(|error| failure(root_location, Failure::DecodeOwnedMember(error.classify())))?;
    let fields: BTreeMap<FieldId, Box<RawValue>> = take(&mut root, "fields", root_location)?;
    let mut relocated_fields = BTreeMap::new();
    for (id, raw_field) in fields {
        let location = TemplateCreationLocation::Field(id);
        let mut field: Object = serde_json::from_str(raw_field.get())
            .map_err(|error| failure(location, Failure::DecodeOwnedMember(error.classify())))?;
        let definition = source
            .fields
            .get(&id)
            .ok_or_else(|| failure(location, Failure::MissingOwnedMember))?;
        if let Some((_, members)) = definition.configuration.members() {
            let mut configuration: Object = take(&mut field, "configuration", location)?;
            let raw_members: BTreeMap<FieldId, Box<RawValue>> =
                take(&mut configuration, "members", location)?;
            if raw_members.len() != members.len() {
                return Err(failure(location, Failure::MissingOwnedMember));
            }
            let moved = raw_members
                .into_iter()
                .map(|(id, raw)| Ok((mapping.field(id)?, raw)))
                .collect::<Result<BTreeMap<_, _>, TemplateCreationError>>()?;
            put(&mut configuration, "members", &moved, location)?;
            put(&mut field, "configuration", &configuration, location)?;
        }
        if definition.configuration.options().is_some() {
            let mut configuration: Object = take(&mut field, "configuration", location)?;
            let options: BTreeMap<OptionId, Box<RawValue>> =
                take(&mut configuration, "options", location)?;
            let relocated_options = options
                .into_iter()
                .map(|(id, option)| Ok((mapping.option(id)?, option)))
                .collect::<Result<BTreeMap<_, _>, TemplateCreationError>>()?;
            put(&mut configuration, "options", &relocated_options, location)?;
            put(&mut field, "configuration", &configuration, location)?;
        }
        relocated_fields.insert(mapping.field(id)?, field);
    }
    put(&mut root, "fields", &relocated_fields, root_location)?;
    let bytes = serde_json::to_vec(&root)
        .map_err(|error| failure(root_location, Failure::EncodeOwnedMember(error.classify())))?;
    // 임시 carrier의 known ID/reference는 아직 source 값이다. caller의 typed candidate rebase가 이를
    // 교체한 뒤에만 artifact를 반환한다. strict 검사를 생략하는 raw source 부착 경로는 만들지 않는다.
    parse_strict_lossless_json_object(&bytes)
        .map_err(|error| failure(root_location, Failure::StrictSource(error.category())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn duplication_provenance_errors_preserve_safe_cause_and_location() {
        let location = TemplateCreationLocation::Template;
        let mut object = Object::new();
        let error = take::<Object>(&mut object, "fields", location).unwrap_err();
        assert_eq!(error.provenance, Some(Failure::MissingOwnedMember));
        object.insert(
            "fields".to_owned(),
            to_raw_value("credential=private-source").unwrap(),
        );
        let error = take::<Object>(&mut object, "fields", location).unwrap_err();
        assert_eq!(
            error.provenance,
            Some(Failure::DecodeOwnedMember(
                serde_json::error::Category::Data
            ))
        );
        assert_eq!(error.location(), location);
        assert_eq!(error.stage(), TemplateCreationStage::Provenance);
        assert!(!format!("{error} {error:?}").contains("credential=private-source"));
        assert!(error.source().is_none());
    }
}
