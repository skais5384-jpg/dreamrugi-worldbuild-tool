use std::fmt;

use serde_json::{Map, Value};

use crate::data::json::{
    is_reserved_json_object_key, validate_json_object_for_storage, JsonStorageErrorCategory,
    MAX_JSON_NESTING_DEPTH,
};

pub(crate) const RICH_TEXT_SCHEMA_VERSION: u32 = 1;

const SEMANTIC_MEMBER_KEYS: &[&str] = &["kind", "children", "text", "marks", "level", "checked"];
const FORBIDDEN_ATTRIBUTE_KEYS: &[&str] = &[
    "type",
    "attrs",
    "style",
    "css",
    "class",
    "className",
    "fontFamily",
    "fontSize",
    "color",
    "alignment",
    "textAlign",
    "href",
    "url",
    "src",
    "target",
    "rel",
    "html",
    "rawHtml",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RichTextValidationErrorCategory {
    UnsupportedSchemaVersion,
    InvalidRoot,
    UnknownNodeType,
    UnknownMarkType,
    InvalidNodeMember,
    InvalidChildNode,
    InvalidAttribute,
    DuplicateMark,
    NonCanonicalMarkOrder,
    EmptyTextNode,
    AdjacentEquivalentText,
    SemanticEmpty,
    LossyNormalizationRequired,
    NestingDepthExceeded,
    ReservedExtraKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RichTextErrorLocation {
    Envelope,
    Root,
    Block,
    Inline,
    Mark,
    ExtraProperty,
}

/// 본문, discriminator 원문, unknown property를 소유하지 않는 안전한 오류다.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct RichTextValidationError {
    category: RichTextValidationErrorCategory,
    location: RichTextErrorLocation,
    node_depth: Option<usize>,
    detail: &'static str,
}

impl RichTextValidationError {
    pub(crate) const fn category(self) -> RichTextValidationErrorCategory {
        self.category
    }

    pub(crate) const fn location(self) -> RichTextErrorLocation {
        self.location
    }

    pub(crate) const fn node_depth(self) -> Option<usize> {
        self.node_depth
    }

    const fn new(
        category: RichTextValidationErrorCategory,
        location: RichTextErrorLocation,
        node_depth: Option<usize>,
        detail: &'static str,
    ) -> Self {
        Self {
            category,
            location,
            node_depth,
            detail,
        }
    }

    const fn at_node(
        category: RichTextValidationErrorCategory,
        location: RichTextErrorLocation,
        node_depth: usize,
        detail: &'static str,
    ) -> Self {
        Self::new(category, location, Some(node_depth), detail)
    }
}

impl fmt::Debug for RichTextValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RichTextValidationError")
            .field("category", &self.category)
            .field("location", &self.location)
            .field("node_depth", &self.node_depth)
            .field("detail", &self.detail)
            .finish()
    }
}

impl fmt::Display for RichTextValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "rich-text validation failed ({:?} at {:?})",
            self.category, self.location
        )?;
        if let Some(depth) = self.node_depth {
            write!(formatter, " at node depth {depth}")?;
        }
        write!(formatter, ": {}", self.detail)
    }
}

impl std::error::Error for RichTextValidationError {}

/// 정규화가 끝난 non-empty AST만 이 타입을 만들 수 있다.
#[derive(Clone, PartialEq)]
pub(crate) struct CanonicalRichText {
    content: Map<String, Value>,
}

impl CanonicalRichText {
    pub(crate) fn content(&self) -> &Map<String, Value> {
        &self.content
    }
}

impl fmt::Debug for CanonicalRichText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalRichText")
            .field("root_member_count", &self.content.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NormalizedRichText {
    Unset,
    RichText(CanonicalRichText),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    Root,
    Paragraph,
    Heading,
    Blockquote,
    BulletList,
    OrderedList,
    ListItem,
    TaskList,
    TaskItem,
    Text,
    HardBreak,
}

impl NodeKind {
    const fn wire_name(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Paragraph => "paragraph",
            Self::Heading => "heading",
            Self::Blockquote => "blockquote",
            Self::BulletList => "bulletList",
            Self::OrderedList => "orderedList",
            Self::ListItem => "listItem",
            Self::TaskList => "taskList",
            Self::TaskItem => "taskItem",
            Self::Text => "text",
            Self::HardBreak => "hardBreak",
        }
    }

    const fn location(self) -> RichTextErrorLocation {
        match self {
            Self::Root => RichTextErrorLocation::Root,
            Self::Text | Self::HardBreak => RichTextErrorLocation::Inline,
            _ => RichTextErrorLocation::Block,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Mark {
    Bold,
    Italic,
    Underline,
    Strikethrough,
}

impl Mark {
    const fn wire_name(self) -> &'static str {
        match self {
            Self::Bold => "bold",
            Self::Italic => "italic",
            Self::Underline => "underline",
            Self::Strikethrough => "strikethrough",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ChildRule {
    Root,
    Block,
    Inline,
    ListItem,
    TaskItem,
}

struct NodeOutput {
    kind: NodeKind,
    marks: Vec<Mark>,
    object: Map<String, Value>,
    has_non_empty_text: bool,
    first_unknown_extra_depth: Option<usize>,
}

/// Persistent envelope와 AST를 수정하지 않고 canonical 여부를 검사한다.
pub(crate) fn validate_persistent_rich_text(
    schema_version: u32,
    content: &Map<String, Value>,
) -> Result<(), RichTextValidationError> {
    validate_envelope(schema_version, content)?;
    let has_non_empty_text = validate_node(content, ChildRule::Root, 1)?;
    if !has_non_empty_text {
        return Err(RichTextValidationError::new(
            RichTextValidationErrorCategory::SemanticEmpty,
            RichTextErrorLocation::Root,
            Some(1),
            "rich-text without non-empty text must use unset",
        ));
    }
    Ok(())
}

/// Editor JSON을 복사해 canonicalize한다. 원본 값은 변경하지 않는다.
pub(crate) fn normalize_rich_text(
    schema_version: u32,
    content: &Map<String, Value>,
) -> Result<NormalizedRichText, RichTextValidationError> {
    validate_envelope(schema_version, content)?;
    let Some(normalized) = normalize_node(content, ChildRule::Root, 1)? else {
        return Ok(NormalizedRichText::Unset);
    };
    if !normalized.has_non_empty_text {
        if let Some(depth) = normalized.first_unknown_extra_depth {
            return Err(RichTextValidationError::at_node(
                RichTextValidationErrorCategory::LossyNormalizationRequired,
                RichTextErrorLocation::ExtraProperty,
                depth,
                "semantic-empty rich-text cannot become unset without discarding unknown data",
            ));
        }
        return Ok(NormalizedRichText::Unset);
    }
    Ok(NormalizedRichText::RichText(CanonicalRichText {
        content: normalized.object,
    }))
}

fn validate_envelope(
    schema_version: u32,
    content: &Map<String, Value>,
) -> Result<(), RichTextValidationError> {
    if schema_version != RICH_TEXT_SCHEMA_VERSION {
        return Err(RichTextValidationError::new(
            RichTextValidationErrorCategory::UnsupportedSchemaVersion,
            RichTextErrorLocation::Envelope,
            None,
            "rich-text schema version is unsupported",
        ));
    }
    validate_json_object_for_storage(content).map_err(|error| match error.category() {
        JsonStorageErrorCategory::NestingDepthExceeded => RichTextValidationError::new(
            RichTextValidationErrorCategory::NestingDepthExceeded,
            RichTextErrorLocation::ExtraProperty,
            None,
            "rich-text JSON exceeds the shared nesting depth",
        ),
        JsonStorageErrorCategory::ReservedObjectKey => RichTextValidationError::new(
            RichTextValidationErrorCategory::ReservedExtraKey,
            RichTextErrorLocation::ExtraProperty,
            None,
            "rich-text contains a reserved interoperability key",
        ),
        JsonStorageErrorCategory::NonFiniteFloat
        | JsonStorageErrorCategory::SerializationFailure => RichTextValidationError::new(
            RichTextValidationErrorCategory::InvalidAttribute,
            RichTextErrorLocation::ExtraProperty,
            None,
            "rich-text contains an unsafe JSON value",
        ),
    })
}

fn validate_node(
    object: &Map<String, Value>,
    expected: ChildRule,
    depth: usize,
) -> Result<bool, RichTextValidationError> {
    ensure_node_depth(depth)?;
    let kind = parse_node_kind(object, depth)?;
    ensure_child_kind(kind, expected, depth)?;
    ensure_allowed_members(object, kind, depth)?;

    match kind {
        NodeKind::Text => {
            let text = required_string(object, "text", kind, depth)?;
            if text.is_empty() {
                return Err(RichTextValidationError::at_node(
                    RichTextValidationErrorCategory::EmptyTextNode,
                    RichTextErrorLocation::Inline,
                    depth,
                    "empty text nodes are noncanonical",
                ));
            }
            validate_marks(object.get("marks"), depth)?;
            Ok(true)
        }
        // 줄바꿈은 실제 text가 있는 문서 안에서만 구조적 의미를 가진다.
        NodeKind::HardBreak => Ok(false),
        NodeKind::Heading => {
            validate_heading_level(object, depth)?;
            validate_children(object, kind, ChildRule::Inline, depth)
        }
        NodeKind::TaskItem => {
            required_bool(object, "checked", kind, depth)?;
            validate_children(object, kind, ChildRule::Block, depth)
        }
        NodeKind::Root => validate_children(object, kind, ChildRule::Block, depth),
        NodeKind::Paragraph => validate_children(object, kind, ChildRule::Inline, depth),
        NodeKind::Blockquote | NodeKind::ListItem => {
            validate_children(object, kind, ChildRule::Block, depth)
        }
        NodeKind::BulletList | NodeKind::OrderedList => {
            validate_children(object, kind, ChildRule::ListItem, depth)
        }
        NodeKind::TaskList => validate_children(object, kind, ChildRule::TaskItem, depth),
    }
}

fn normalize_node(
    object: &Map<String, Value>,
    expected: ChildRule,
    depth: usize,
) -> Result<Option<NodeOutput>, RichTextValidationError> {
    ensure_node_depth(depth)?;
    let kind = parse_node_kind(object, depth)?;
    ensure_child_kind(kind, expected, depth)?;
    ensure_allowed_members(object, kind, depth)?;

    let mut normalized = object.clone();
    normalized.insert(
        "kind".to_owned(),
        Value::String(kind.wire_name().to_owned()),
    );
    match kind {
        NodeKind::Text => {
            let text = required_string(object, "text", kind, depth)?;
            let marks = normalize_marks(object.get("marks"), depth)?;
            if text.is_empty() {
                if contains_unknown_extra(object, kind) {
                    return Err(RichTextValidationError::at_node(
                        RichTextValidationErrorCategory::LossyNormalizationRequired,
                        RichTextErrorLocation::ExtraProperty,
                        depth,
                        "an empty text node with extras cannot be removed losslessly",
                    ));
                }
                return Ok(None);
            }
            if marks.is_empty() {
                normalized.remove("marks");
            } else {
                normalized.insert(
                    "marks".to_owned(),
                    Value::Array(
                        marks
                            .iter()
                            .map(|mark| Value::String(mark.wire_name().to_owned()))
                            .collect(),
                    ),
                );
            }
            Ok(Some(NodeOutput {
                kind,
                marks,
                object: normalized,
                has_non_empty_text: true,
                first_unknown_extra_depth: contains_unknown_extra(object, kind).then_some(depth),
            }))
        }
        NodeKind::HardBreak => Ok(Some(NodeOutput {
            kind,
            marks: Vec::new(),
            object: normalized,
            has_non_empty_text: false,
            first_unknown_extra_depth: contains_unknown_extra(object, kind).then_some(depth),
        })),
        NodeKind::Heading => {
            validate_heading_level(object, depth)?;
            normalize_container(normalized, kind, ChildRule::Inline, depth)
        }
        NodeKind::TaskItem => {
            required_bool(object, "checked", kind, depth)?;
            normalize_container(normalized, kind, ChildRule::Block, depth)
        }
        NodeKind::Root => normalize_container(normalized, kind, ChildRule::Block, depth),
        NodeKind::Paragraph => normalize_container(normalized, kind, ChildRule::Inline, depth),
        NodeKind::Blockquote | NodeKind::ListItem => {
            normalize_container(normalized, kind, ChildRule::Block, depth)
        }
        NodeKind::BulletList | NodeKind::OrderedList => {
            normalize_container(normalized, kind, ChildRule::ListItem, depth)
        }
        NodeKind::TaskList => normalize_container(normalized, kind, ChildRule::TaskItem, depth),
    }
}

fn validate_children(
    object: &Map<String, Value>,
    parent: NodeKind,
    child_rule: ChildRule,
    depth: usize,
) -> Result<bool, RichTextValidationError> {
    let children = required_children(object, parent, depth)?;
    let mut has_non_empty_text = false;
    let mut previous_text: Option<(&Map<String, Value>, Vec<Mark>)> = None;
    for child in children {
        let child_object = child.as_object().ok_or_else(|| {
            RichTextValidationError::at_node(
                RichTextValidationErrorCategory::InvalidChildNode,
                parent.location(),
                depth,
                "child nodes must be objects",
            )
        })?;
        let child_kind = parse_node_kind(child_object, depth + 1)?;
        let child_has_non_empty_text = validate_node(child_object, child_rule, depth + 1)?;
        has_non_empty_text |= child_has_non_empty_text;

        if child_kind == NodeKind::Text {
            let marks = validate_marks(child_object.get("marks"), depth + 1)?;
            if let Some((previous_object, previous_marks)) = previous_text.as_ref() {
                if previous_marks == &marks
                    && text_has_no_unknown_extras(previous_object)
                    && text_has_no_unknown_extras(child_object)
                {
                    return Err(RichTextValidationError::at_node(
                        RichTextValidationErrorCategory::AdjacentEquivalentText,
                        RichTextErrorLocation::Inline,
                        depth + 1,
                        "adjacent text nodes with equal marks and no extras must be merged",
                    ));
                }
            }
            previous_text = Some((child_object, marks));
        } else {
            previous_text = None;
        }
    }
    Ok(has_non_empty_text)
}

fn normalize_container(
    mut object: Map<String, Value>,
    kind: NodeKind,
    child_rule: ChildRule,
    depth: usize,
) -> Result<Option<NodeOutput>, RichTextValidationError> {
    let children = required_children(&object, kind, depth)?;
    let mut first_unknown_extra_depth = contains_unknown_extra(&object, kind).then_some(depth);
    let mut normalized_children: Vec<NodeOutput> = Vec::with_capacity(children.len());
    for child in children {
        let child_object = child.as_object().ok_or_else(|| {
            RichTextValidationError::at_node(
                RichTextValidationErrorCategory::InvalidChildNode,
                kind.location(),
                depth,
                "child nodes must be objects",
            )
        })?;
        let Some(child) = normalize_node(child_object, child_rule, depth + 1)? else {
            continue;
        };
        if child.kind == NodeKind::Text {
            if let Some(previous) = normalized_children.last_mut() {
                if previous.kind == NodeKind::Text
                    && previous.marks == child.marks
                    && text_has_no_unknown_extras(&previous.object)
                    && text_has_no_unknown_extras(&child.object)
                {
                    merge_text_nodes(&mut previous.object, &child.object);
                    continue;
                }
            }
        }
        if first_unknown_extra_depth.is_none() {
            first_unknown_extra_depth = child.first_unknown_extra_depth;
        }
        normalized_children.push(child);
    }

    let has_non_empty_text = normalized_children
        .iter()
        .any(|child| child.has_non_empty_text);
    object.insert(
        "children".to_owned(),
        Value::Array(
            normalized_children
                .into_iter()
                .map(|child| Value::Object(child.object))
                .collect(),
        ),
    );
    Ok(Some(NodeOutput {
        kind,
        marks: Vec::new(),
        object,
        has_non_empty_text,
        first_unknown_extra_depth,
    }))
}

fn merge_text_nodes(left: &mut Map<String, Value>, right: &Map<String, Value>) {
    // 호출자는 양쪽에 unknown extra가 없을 때만 전달하므로 node별 미래 metadata를 잃지 않는다.
    let right_text = right
        .get("text")
        .and_then(Value::as_str)
        .expect("normalized text node must retain text");
    let Value::String(left_text) = left
        .get_mut("text")
        .expect("normalized text node must retain text")
    else {
        unreachable!("normalized text node must retain a string");
    };
    left_text.push_str(right_text);
}

fn text_has_no_unknown_extras(object: &Map<String, Value>) -> bool {
    !contains_unknown_extra(object, NodeKind::Text)
}

fn parse_node_kind(
    object: &Map<String, Value>,
    depth: usize,
) -> Result<NodeKind, RichTextValidationError> {
    let Some(value) = object.get("kind") else {
        return Err(RichTextValidationError::at_node(
            if depth == 1 {
                RichTextValidationErrorCategory::InvalidRoot
            } else {
                RichTextValidationErrorCategory::InvalidNodeMember
            },
            if depth == 1 {
                RichTextErrorLocation::Root
            } else {
                RichTextErrorLocation::Block
            },
            depth,
            "node kind is required",
        ));
    };
    let Some(kind) = value.as_str() else {
        return Err(RichTextValidationError::at_node(
            RichTextValidationErrorCategory::InvalidNodeMember,
            if depth == 1 {
                RichTextErrorLocation::Root
            } else {
                RichTextErrorLocation::Block
            },
            depth,
            "node kind must be a string",
        ));
    };
    match kind {
        "root" => Ok(NodeKind::Root),
        "paragraph" => Ok(NodeKind::Paragraph),
        "heading" => Ok(NodeKind::Heading),
        "blockquote" => Ok(NodeKind::Blockquote),
        "bulletList" => Ok(NodeKind::BulletList),
        "orderedList" => Ok(NodeKind::OrderedList),
        "listItem" => Ok(NodeKind::ListItem),
        "taskList" => Ok(NodeKind::TaskList),
        "taskItem" => Ok(NodeKind::TaskItem),
        "text" => Ok(NodeKind::Text),
        "hardBreak" => Ok(NodeKind::HardBreak),
        _ => Err(RichTextValidationError::at_node(
            RichTextValidationErrorCategory::UnknownNodeType,
            if depth == 1 {
                RichTextErrorLocation::Root
            } else {
                RichTextErrorLocation::Block
            },
            depth,
            "node discriminator is not in the rich-text allowlist",
        )),
    }
}

fn ensure_child_kind(
    kind: NodeKind,
    expected: ChildRule,
    depth: usize,
) -> Result<(), RichTextValidationError> {
    let valid = match expected {
        ChildRule::Root => kind == NodeKind::Root,
        ChildRule::Block => matches!(
            kind,
            NodeKind::Paragraph
                | NodeKind::Heading
                | NodeKind::Blockquote
                | NodeKind::BulletList
                | NodeKind::OrderedList
                | NodeKind::TaskList
        ),
        ChildRule::Inline => matches!(kind, NodeKind::Text | NodeKind::HardBreak),
        ChildRule::ListItem => kind == NodeKind::ListItem,
        ChildRule::TaskItem => kind == NodeKind::TaskItem,
    };
    if valid {
        return Ok(());
    }
    Err(RichTextValidationError::at_node(
        if matches!(expected, ChildRule::Root) {
            RichTextValidationErrorCategory::InvalidRoot
        } else {
            RichTextValidationErrorCategory::InvalidChildNode
        },
        kind.location(),
        depth,
        "node is not allowed under its parent",
    ))
}

fn ensure_allowed_members(
    object: &Map<String, Value>,
    kind: NodeKind,
    depth: usize,
) -> Result<(), RichTextValidationError> {
    let allowed = known_members(kind);
    for key in object.keys() {
        if is_reserved_json_object_key(key) {
            return Err(RichTextValidationError::at_node(
                RichTextValidationErrorCategory::ReservedExtraKey,
                RichTextErrorLocation::ExtraProperty,
                depth,
                "node contains a reserved interoperability key",
            ));
        }
        if (SEMANTIC_MEMBER_KEYS.contains(&key.as_str()) && !allowed.contains(&key.as_str()))
            || FORBIDDEN_ATTRIBUTE_KEYS.contains(&key.as_str())
        {
            return Err(RichTextValidationError::at_node(
                RichTextValidationErrorCategory::InvalidAttribute,
                RichTextErrorLocation::ExtraProperty,
                depth,
                "node contains a forbidden or misplaced attribute",
            ));
        }
    }
    Ok(())
}

fn known_members(kind: NodeKind) -> &'static [&'static str] {
    match kind {
        NodeKind::Text => &["kind", "text", "marks"],
        NodeKind::Heading => &["kind", "children", "level"],
        NodeKind::TaskItem => &["kind", "children", "checked"],
        NodeKind::HardBreak => &["kind"],
        _ => &["kind", "children"],
    }
}

fn contains_unknown_extra(object: &Map<String, Value>, kind: NodeKind) -> bool {
    let known = known_members(kind);
    object.keys().any(|key| !known.contains(&key.as_str()))
}

/// 검증된 rich-text AST에서 미래 storage metadata의 존재만 private하게 보고한다.
///
/// mutation 계층은 raw AST를 받지 않고 이 판정만 사용해, node 삭제·병합 시 metadata 위치를
/// 임의로 대응시키지 않는 fail-closed 정책을 적용한다. 잘못된 node도 안전하게 unknown으로 본다.
pub(crate) fn rich_text_contains_unknown_storage_data(content: &Map<String, Value>) -> bool {
    let mut pending = vec![content];
    while let Some(object) = pending.pop() {
        let Ok(kind) = parse_node_kind(object, 1) else {
            return true;
        };
        if contains_unknown_extra(object, kind) {
            return true;
        }
        if let Some(children) = object.get("children").and_then(Value::as_array) {
            for child in children {
                let Some(child) = child.as_object() else {
                    return true;
                };
                pending.push(child);
            }
        }
    }

    false
}

fn required_children(
    object: &Map<String, Value>,
    kind: NodeKind,
    depth: usize,
) -> Result<&[Value], RichTextValidationError> {
    object
        .get("children")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| {
            RichTextValidationError::at_node(
                RichTextValidationErrorCategory::InvalidNodeMember,
                kind.location(),
                depth,
                "container node children must be an array",
            )
        })
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    kind: NodeKind,
    depth: usize,
) -> Result<&'a str, RichTextValidationError> {
    object.get(key).and_then(Value::as_str).ok_or_else(|| {
        RichTextValidationError::at_node(
            RichTextValidationErrorCategory::InvalidNodeMember,
            kind.location(),
            depth,
            "required node string member is missing or has the wrong type",
        )
    })
}

fn required_bool(
    object: &Map<String, Value>,
    key: &str,
    kind: NodeKind,
    depth: usize,
) -> Result<bool, RichTextValidationError> {
    object.get(key).and_then(Value::as_bool).ok_or_else(|| {
        RichTextValidationError::at_node(
            RichTextValidationErrorCategory::InvalidNodeMember,
            kind.location(),
            depth,
            "required node boolean member is missing or has the wrong type",
        )
    })
}

fn validate_heading_level(
    object: &Map<String, Value>,
    depth: usize,
) -> Result<(), RichTextValidationError> {
    let valid = object
        .get("level")
        .and_then(Value::as_u64)
        .is_some_and(|level| (1..=6).contains(&level));
    if valid {
        return Ok(());
    }
    Err(RichTextValidationError::at_node(
        RichTextValidationErrorCategory::InvalidAttribute,
        RichTextErrorLocation::Block,
        depth,
        "heading level must be an integer from 1 through 6",
    ))
}

fn validate_marks(
    value: Option<&Value>,
    depth: usize,
) -> Result<Vec<Mark>, RichTextValidationError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let marks = value.as_array().ok_or_else(|| {
        RichTextValidationError::at_node(
            RichTextValidationErrorCategory::InvalidNodeMember,
            RichTextErrorLocation::Mark,
            depth,
            "text marks must be an array",
        )
    })?;
    if marks.is_empty() {
        return Err(RichTextValidationError::at_node(
            RichTextValidationErrorCategory::InvalidAttribute,
            RichTextErrorLocation::Mark,
            depth,
            "an empty marks member is unnecessary and noncanonical",
        ));
    }

    let parsed = parse_marks(marks, depth)?;
    let mut seen = [false; 4];
    for mark in parsed.iter().copied() {
        let index = mark as usize;
        if seen[index] {
            return Err(RichTextValidationError::at_node(
                RichTextValidationErrorCategory::DuplicateMark,
                RichTextErrorLocation::Mark,
                depth,
                "text marks contain a duplicate",
            ));
        }
        seen[index] = true;
    }
    let mut previous = None;
    for mark in parsed.iter().copied() {
        if previous.is_some_and(|previous| previous > mark) {
            return Err(RichTextValidationError::at_node(
                RichTextValidationErrorCategory::NonCanonicalMarkOrder,
                RichTextErrorLocation::Mark,
                depth,
                "text marks are not in canonical order",
            ));
        }
        previous = Some(mark);
    }
    Ok(parsed)
}

fn normalize_marks(
    value: Option<&Value>,
    depth: usize,
) -> Result<Vec<Mark>, RichTextValidationError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let marks = value.as_array().ok_or_else(|| {
        RichTextValidationError::at_node(
            RichTextValidationErrorCategory::InvalidNodeMember,
            RichTextErrorLocation::Mark,
            depth,
            "text marks must be an array",
        )
    })?;
    let mut parsed = parse_marks(marks, depth)?;
    parsed.sort_unstable();
    parsed.dedup();
    Ok(parsed)
}

fn parse_marks(marks: &[Value], depth: usize) -> Result<Vec<Mark>, RichTextValidationError> {
    marks
        .iter()
        .map(|mark| {
            let Some(mark) = mark.as_str() else {
                return Err(RichTextValidationError::at_node(
                    RichTextValidationErrorCategory::InvalidNodeMember,
                    RichTextErrorLocation::Mark,
                    depth,
                    "each text mark must be a string discriminator",
                ));
            };
            match mark {
                "bold" => Ok(Mark::Bold),
                "italic" => Ok(Mark::Italic),
                "underline" => Ok(Mark::Underline),
                "strikethrough" => Ok(Mark::Strikethrough),
                _ => Err(RichTextValidationError::at_node(
                    RichTextValidationErrorCategory::UnknownMarkType,
                    RichTextErrorLocation::Mark,
                    depth,
                    "mark discriminator is not in the rich-text allowlist",
                )),
            }
        })
        .collect()
}

fn ensure_node_depth(depth: usize) -> Result<(), RichTextValidationError> {
    if depth <= MAX_JSON_NESTING_DEPTH {
        return Ok(());
    }
    Err(RichTextValidationError::new(
        RichTextValidationErrorCategory::NestingDepthExceeded,
        RichTextErrorLocation::Root,
        None,
        "rich-text node nesting exceeds the shared JSON limit",
    ))
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use serde_json::json;

    use super::*;

    fn content(value: Value) -> Map<String, Value> {
        value
            .as_object()
            .expect("test rich-text content must be an object")
            .clone()
    }

    fn text(value: &str) -> Value {
        json!({"kind":"text","text":value})
    }

    fn paragraph(children: Vec<Value>) -> Value {
        json!({"kind":"paragraph","children":children})
    }

    fn root(children: Vec<Value>) -> Map<String, Value> {
        content(json!({"kind":"root","children":children}))
    }

    fn category(value: Value) -> RichTextValidationErrorCategory {
        validate_persistent_rich_text(RICH_TEXT_SCHEMA_VERSION, &content(value))
            .unwrap_err()
            .category()
    }

    #[test]
    fn paragraph_text_and_all_canonical_marks_are_valid() {
        for mark in ["bold", "italic", "underline", "strikethrough"] {
            validate_persistent_rich_text(
                1,
                &root(vec![paragraph(vec![json!({
                    "kind":"text",
                    "text":"본문",
                    "marks":[mark]
                })])]),
            )
            .unwrap();
        }
        validate_persistent_rich_text(
            1,
            &root(vec![paragraph(vec![json!({
                "kind":"text",
                "text":"all",
                "marks":["bold","italic","underline","strikethrough"]
            })])]),
        )
        .unwrap();
    }

    #[test]
    fn heading_blockquote_lists_tasks_and_hard_break_are_valid() {
        let fixture = root(vec![
            json!({"kind":"heading","level":1,"children":[text("제목")]}),
            json!({"kind":"heading","level":6,"children":[text("작은 제목")]}),
            json!({"kind":"blockquote","children":[paragraph(vec![text("인용")])]}),
            json!({"kind":"bulletList","children":[
                {"kind":"listItem","children":[paragraph(vec![text("항목")])]}
            ]}),
            json!({"kind":"orderedList","children":[
                {"kind":"listItem","children":[paragraph(vec![text("순서")])]}
            ]}),
            json!({"kind":"taskList","children":[
                {"kind":"taskItem","checked":true,"children":[paragraph(vec![text("완료")])]},
                {"kind":"taskItem","checked":false,"children":[paragraph(vec![text("미완료")])]}
            ]}),
            paragraph(vec![json!({"kind":"hardBreak"})]),
        ]);
        validate_persistent_rich_text(1, &fixture).unwrap();
    }

    #[test]
    fn nested_blockquote_and_lists_are_bounded_and_valid() {
        let nested = json!({"kind":"blockquote","children":[
            {"kind":"bulletList","children":[
                {"kind":"listItem","children":[
                    {"kind":"orderedList","children":[
                        {"kind":"listItem","children":[paragraph(vec![text("깊은 본문")])]}
                    ]}
                ]}
            ]}
        ]});
        validate_persistent_rich_text(1, &root(vec![nested])).unwrap();
    }

    #[test]
    fn whitespace_and_unicode_are_preserved_exactly() {
        let original = root(vec![paragraph(vec![
            text(" \t\n"),
            text("한글 e\u{301} 😀"),
        ])]);
        let NormalizedRichText::RichText(normalized) = normalize_rich_text(1, &original).unwrap()
        else {
            panic!("whitespace and Unicode text are semantic content");
        };
        let children = normalized.content()["children"][0]["children"]
            .as_array()
            .unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0]["text"], json!(" \t\n한글 e\u{301} 😀"));
    }

    #[test]
    fn unknown_extras_and_arbitrary_precision_numbers_survive_normalization() {
        let huge: Value = serde_json::from_str("1234567890123456789012345678901234567890").unwrap();
        let mut fixture = root(vec![paragraph(vec![text("본문")])]);
        fixture.insert(
            "future".to_owned(),
            json!({"nested":[{"huge":huge.clone()},3,1]}),
        );
        let NormalizedRichText::RichText(normalized) = normalize_rich_text(1, &fixture).unwrap()
        else {
            panic!("fixture is not empty");
        };
        assert_eq!(normalized.content()["future"]["nested"][0]["huge"], huge);
        validate_persistent_rich_text(1, normalized.content()).unwrap();
    }

    #[test]
    fn unsupported_version_and_invalid_root_fail_closed() {
        let fixture = root(vec![paragraph(vec![text("본문")])]);
        assert_eq!(
            validate_persistent_rich_text(2, &fixture)
                .unwrap_err()
                .category(),
            RichTextValidationErrorCategory::UnsupportedSchemaVersion
        );
        assert_eq!(
            category(json!({"kind":"paragraph","children":[text("본문")]})),
            RichTextValidationErrorCategory::InvalidRoot
        );
        assert_eq!(
            category(json!({"children":[]})),
            RichTextValidationErrorCategory::InvalidRoot
        );
    }

    #[test]
    fn unknown_and_forbidden_nodes_and_marks_fail_closed() {
        for kind in [
            "futureNode",
            "html",
            "table",
            "codeBlock",
            "code",
            "image",
            "attachment",
            "iframe",
            "embed",
            "script",
        ] {
            assert_eq!(
                category(json!({"kind":"root","children":[{"kind":kind,"children":[]}]})),
                RichTextValidationErrorCategory::UnknownNodeType
            );
        }
        for mark in ["link", "code", "color", "fontFamily", "futureMark"] {
            assert_eq!(
                category(json!({"kind":"root","children":[paragraph(vec![json!({
                    "kind":"text","text":"본문","marks":[mark]
                })])]})),
                RichTextValidationErrorCategory::UnknownMarkType
            );
        }
    }

    #[test]
    fn invalid_parent_child_and_member_types_are_rejected() {
        for fixture in [
            json!({"kind":"root","children":[text("inline at root")]}),
            json!({"kind":"root","children":[{"kind":"listItem","children":[]}]}),
            json!({"kind":"root","children":[{"kind":"paragraph","children":[paragraph(vec![])]}]}),
            json!({"kind":"root","children":[{"kind":"bulletList","children":[paragraph(vec![])]}]}),
            json!({"kind":"root","children":[{"kind":"taskList","children":[{"kind":"listItem","children":[]}]}]}),
        ] {
            assert_eq!(
                category(fixture),
                RichTextValidationErrorCategory::InvalidChildNode
            );
        }
        for fixture in [
            json!({"kind":"root","children":"wrong"}),
            json!({"kind":"root","children":[{"kind":"paragraph","children":[7]}]}),
            json!({"kind":"root","children":[paragraph(vec![json!({"kind":"text","text":7})])]}),
            json!({"kind":"root","children":[{"kind":"taskList","children":[{"kind":"taskItem","checked":"yes","children":[]}]}]}),
        ] {
            let error = validate_persistent_rich_text(1, &content(fixture)).unwrap_err();
            assert!(matches!(
                error.category(),
                RichTextValidationErrorCategory::InvalidNodeMember
                    | RichTextValidationErrorCategory::InvalidChildNode
            ));
        }
    }

    #[test]
    fn heading_range_and_forbidden_attributes_are_rejected() {
        for level in [json!(0), json!(7), json!(1.5), json!("1"), Value::Null] {
            assert_eq!(
                category(json!({"kind":"root","children":[{
                    "kind":"heading","level":level,"children":[text("제목")]
                }]})),
                RichTextValidationErrorCategory::InvalidAttribute
            );
        }
        for (key, value) in [
            ("style", json!("color:red")),
            ("fontFamily", json!("private-font")),
            ("fontSize", json!(72)),
            ("color", json!("#ffffff")),
            ("alignment", json!("center")),
            ("href", json!("https://example.invalid/secret")),
            ("html", json!("<script>bad()</script>")),
        ] {
            let mut node = json!({"kind":"text","text":"본문"});
            node[key] = value;
            assert_eq!(
                category(json!({"kind":"root","children":[paragraph(vec![node])]})),
                RichTextValidationErrorCategory::InvalidAttribute
            );
        }
    }

    #[test]
    fn empty_duplicate_unordered_and_adjacent_text_are_noncanonical() {
        assert_eq!(
            category(json!({"kind":"root","children":[paragraph(vec![text("")])]})),
            RichTextValidationErrorCategory::EmptyTextNode
        );
        assert_eq!(
            category(json!({"kind":"root","children":[paragraph(vec![json!({
                "kind":"text","text":"x","marks":["bold","bold"]
            })])]})),
            RichTextValidationErrorCategory::DuplicateMark
        );
        assert_eq!(
            category(json!({"kind":"root","children":[paragraph(vec![json!({
                "kind":"text","text":"x","marks":["bold","italic","bold"]
            })])]})),
            RichTextValidationErrorCategory::DuplicateMark
        );
        assert_eq!(
            category(json!({"kind":"root","children":[paragraph(vec![json!({
                "kind":"text","text":"x","marks":["underline","italic"]
            })])]})),
            RichTextValidationErrorCategory::NonCanonicalMarkOrder
        );
        assert_eq!(
            category(json!({"kind":"root","children":[paragraph(vec![text("a"),text("b")])]})),
            RichTextValidationErrorCategory::AdjacentEquivalentText
        );
    }

    #[test]
    fn semantic_empty_documents_must_use_unset() {
        for fixture in [
            json!({"kind":"root","children":[]}),
            json!({"kind":"root","children":[paragraph(vec![])]}),
            json!({"kind":"root","children":[{"kind":"heading","level":1,"children":[]}]}),
            json!({"kind":"root","children":[{"kind":"blockquote","children":[paragraph(vec![])]}]}),
            json!({"kind":"root","children":[{"kind":"bulletList","children":[{"kind":"listItem","children":[]}]}]}),
            json!({"kind":"root","children":[{"kind":"orderedList","children":[{"kind":"listItem","children":[paragraph(vec![])]}]}]}),
            json!({"kind":"root","children":[{"kind":"taskList","children":[{"kind":"taskItem","checked":true,"children":[]}]}]}),
            json!({"kind":"root","children":[paragraph(vec![json!({"kind":"hardBreak"})])]}),
            json!({"kind":"root","children":[paragraph(vec![json!({"kind":"hardBreak"}),json!({"kind":"hardBreak"})])]}),
            json!({"kind":"root","children":[paragraph(vec![]),paragraph(vec![json!({"kind":"hardBreak"})])]}),
            json!({"kind":"root","children":[{"kind":"blockquote","children":[{"kind":"bulletList","children":[{"kind":"listItem","children":[paragraph(vec![json!({"kind":"hardBreak"})])]}]}]}]}),
            json!({"kind":"root","children":[{"kind":"taskList","children":[{"kind":"taskItem","checked":true,"children":[paragraph(vec![json!({"kind":"hardBreak"})])]}]}]}),
        ] {
            let content = content(fixture);
            assert_eq!(
                validate_persistent_rich_text(1, &content)
                    .unwrap_err()
                    .category(),
                RichTextValidationErrorCategory::SemanticEmpty
            );
            assert_eq!(
                normalize_rich_text(1, &content).unwrap(),
                NormalizedRichText::Unset
            );
        }
        validate_persistent_rich_text(
            1,
            &root(vec![paragraph(vec![]), paragraph(vec![text("본문")])]),
        )
        .unwrap();
    }

    #[test]
    fn hard_break_is_preserved_only_when_the_document_has_non_empty_text() {
        for original in [
            root(vec![paragraph(vec![
                text(" "),
                json!({"kind":"hardBreak"}),
            ])]),
            root(vec![paragraph(vec![
                json!({"kind":"hardBreak"}),
                text("앞"),
            ])]),
            root(vec![paragraph(vec![
                text("뒤"),
                json!({"kind":"hardBreak"}),
            ])]),
            root(vec![paragraph(vec![
                text("앞"),
                json!({"kind":"hardBreak"}),
                text("뒤"),
            ])]),
            root(vec![paragraph(vec![
                json!({"kind":"hardBreak"}),
                text("한글 😀"),
                json!({"kind":"hardBreak"}),
            ])]),
        ] {
            validate_persistent_rich_text(1, &original).unwrap();
            let NormalizedRichText::RichText(normalized) =
                normalize_rich_text(1, &original).unwrap()
            else {
                panic!("non-empty text must keep the rich-text document");
            };
            assert_eq!(normalized.content(), &original);
        }
    }

    #[test]
    fn semantic_empty_normalization_fails_before_discarding_unknown_data() {
        let huge_literal = "1234567890123456789012345678901234567890";
        let huge: Value = serde_json::from_str(huge_literal).unwrap();
        let secret = "credential=lossless-normalization-secret";
        let fixtures = [
            (
                json!({"kind":"root","children":[],"futureRoot":{"huge":huge,"items":[3,1,2],"secret":secret}}),
                1,
            ),
            (
                json!({"kind":"root","children":[{"kind":"paragraph","children":[],"futureParagraph":{"nested":[1,{"keep":true}]}}]}),
                2,
            ),
            (
                json!({"kind":"root","children":[{"kind":"heading","level":2,"children":[],"futureHeading":true}]}),
                2,
            ),
            (
                json!({"kind":"root","children":[{"kind":"blockquote","children":[],"futureQuote":{"keep":true}}]}),
                2,
            ),
            (
                json!({"kind":"root","children":[{"kind":"bulletList","children":[{"kind":"listItem","children":[]}],"futureList":[1,{"keep":true}]}]}),
                2,
            ),
            (
                json!({"kind":"root","children":[{"kind":"blockquote","children":[{"kind":"bulletList","children":[{"kind":"listItem","children":[paragraph(vec![])],"futureItem":{"keep":"item"}}]}]}]}),
                4,
            ),
            (
                json!({"kind":"root","children":[{"kind":"taskList","children":[{"kind":"taskItem","checked":true,"children":[],"futureTask":[{"keep":true}]}]}]}),
                3,
            ),
            (
                json!({"kind":"root","children":[paragraph(vec![json!({"kind":"text","text":"","futureText":{"keep":true}})])]}),
                3,
            ),
            (
                json!({"kind":"root","children":[paragraph(vec![json!({"kind":"hardBreak","futureBreak":{"keep":true}})])]}),
                3,
            ),
        ];

        for (fixture, expected_depth) in fixtures {
            let original = content(fixture);
            let before = original.clone();
            let error = normalize_rich_text(1, &original)
                .expect_err("unknown data must not be discarded to produce unset");
            assert_eq!(
                error.category(),
                RichTextValidationErrorCategory::LossyNormalizationRequired
            );
            assert_eq!(error.location(), RichTextErrorLocation::ExtraProperty);
            assert_eq!(error.node_depth(), Some(expected_depth));
            assert_eq!(original, before);
            for forbidden in [secret, huge_literal, "futureRoot", "futureParagraph"] {
                assert!(!error.to_string().contains(forbidden));
                assert!(!format!("{error:?}").contains(forbidden));
            }
        }

        assert_eq!(
            normalize_rich_text(1, &root(vec![])).unwrap(),
            NormalizedRichText::Unset
        );
    }

    #[test]
    fn adjacent_text_merge_requires_both_unknown_extra_maps_to_be_empty() {
        let huge_left: Value =
            serde_json::from_str("1234567890123456789012345678901234567890").unwrap();
        let huge_right: Value =
            serde_json::from_str("1234567890123456789012345678901234567891").unwrap();
        let identical_extra = json!({"scope":"same"});
        let identical_nested_extra = json!({"nested":[{"keep":true},3,1],"label":"metadata"});
        let preserved = [
            (
                "left only",
                root(vec![paragraph(vec![
                    json!({"kind":"text","text":"a","marks":["bold"],"future":{"side":"left"}}),
                    json!({"kind":"text","text":"b","marks":["bold"]}),
                ])]),
            ),
            (
                "right only",
                root(vec![paragraph(vec![
                    json!({"kind":"text","text":"a","marks":["bold"]}),
                    json!({"kind":"text","text":"b","marks":["bold"],"future":{"side":"right"}}),
                ])]),
            ),
            (
                "different keys",
                root(vec![paragraph(vec![
                    json!({"kind":"text","text":"a","marks":["bold"],"futureA":1}),
                    json!({"kind":"text","text":"b","marks":["bold"],"futureB":2}),
                ])]),
            ),
            (
                "same key different value",
                root(vec![paragraph(vec![
                    json!({"kind":"text","text":"a","marks":["bold"],"future":{"value":1}}),
                    json!({"kind":"text","text":"b","marks":["bold"],"future":{"value":2}}),
                ])]),
            ),
            (
                "identical non-empty extra",
                root(vec![paragraph(vec![
                    json!({"kind":"text","text":"a","marks":["bold"],"future":identical_extra.clone()}),
                    json!({"kind":"text","text":"b","marks":["bold"],"future":identical_extra}),
                ])]),
            ),
            (
                "identical nested object and array",
                root(vec![paragraph(vec![
                    json!({"kind":"text","text":"a","marks":["bold"],"future":identical_nested_extra.clone()}),
                    json!({"kind":"text","text":"b","marks":["bold"],"future":identical_nested_extra}),
                ])]),
            ),
            (
                "identical arbitrary-precision number",
                root(vec![paragraph(vec![
                    json!({"kind":"text","text":"a","marks":["bold"],"future":{"huge":huge_left.clone()}}),
                    json!({"kind":"text","text":"b","marks":["bold"],"future":{"huge":huge_left.clone()}}),
                ])]),
            ),
            (
                "different arbitrary-precision number",
                root(vec![paragraph(vec![
                    json!({"kind":"text","text":"a","marks":["bold"],"future":{"huge":huge_left}}),
                    json!({"kind":"text","text":"b","marks":["bold"],"future":{"huge":huge_right}}),
                ])]),
            ),
            (
                "different marks without extras",
                root(vec![paragraph(vec![
                    json!({"kind":"text","text":"a","marks":["bold"]}),
                    json!({"kind":"text","text":"b","marks":["italic"]}),
                ])]),
            ),
        ];

        for (case, original) in preserved {
            validate_persistent_rich_text(1, &original).unwrap();
            let original_bytes = serde_json::to_vec(&original).unwrap();
            let NormalizedRichText::RichText(first) = normalize_rich_text(1, &original).unwrap()
            else {
                panic!("text fixtures are non-empty");
            };
            assert_eq!(
                serde_json::to_vec(first.content()).unwrap(),
                original_bytes,
                "{case}"
            );
            let children = first.content()["children"][0]["children"]
                .as_array()
                .unwrap();
            assert_eq!(children.len(), 2, "{case}");
            assert_eq!(children[0]["text"], json!("a"), "{case}");
            assert_eq!(children[1]["text"], json!("b"), "{case}");
            validate_persistent_rich_text(1, first.content()).unwrap();
            let NormalizedRichText::RichText(second) =
                normalize_rich_text(1, first.content()).unwrap()
            else {
                panic!("canonical text fixtures stay non-empty");
            };
            assert_eq!(second.content(), first.content(), "{case}");
        }

        let merge_input = root(vec![paragraph(vec![
            json!({"kind":"text","text":"a","marks":["bold","underline"]}),
            json!({"kind":"text","text":"b","marks":["bold","underline"]}),
        ])]);
        let before = merge_input.clone();
        assert_eq!(
            validate_persistent_rich_text(1, &merge_input)
                .unwrap_err()
                .category(),
            RichTextValidationErrorCategory::AdjacentEquivalentText
        );
        let NormalizedRichText::RichText(merged) = normalize_rich_text(1, &merge_input).unwrap()
        else {
            panic!("merged text is non-empty");
        };
        assert_eq!(
            merged.content()["children"][0]["children"],
            json!([{"kind":"text","text":"ab","marks":["bold","underline"]}])
        );
        assert_eq!(merge_input, before);
        validate_persistent_rich_text(1, merged.content()).unwrap();
        let NormalizedRichText::RichText(merged_again) =
            normalize_rich_text(1, merged.content()).unwrap()
        else {
            panic!("merged text stays non-empty");
        };
        assert_eq!(merged_again.content(), merged.content());
    }

    #[test]
    fn normalization_sorts_deduplicates_removes_merges_recurses_and_is_idempotent() {
        let original = root(vec![json!({
            "kind":"blockquote",
            "children":[paragraph(vec![
                json!({"kind":"text","text":"","marks":["bold"]}),
                json!({"kind":"text","text":"a","marks":["underline","bold","bold"]}),
                json!({"kind":"text","text":"b","marks":["bold","underline"]}),
                text("c")
            ])]
        })]);
        let before = original.clone();
        let NormalizedRichText::RichText(first) = normalize_rich_text(1, &original).unwrap() else {
            panic!("normalization should retain text");
        };
        assert_eq!(original, before);
        let inline = &first.content()["children"][0]["children"][0]["children"];
        assert_eq!(
            inline,
            &json!([
                {"kind":"text","text":"ab","marks":["bold","underline"]},
                {"kind":"text","text":"c"}
            ])
        );
        let NormalizedRichText::RichText(second) = normalize_rich_text(1, first.content()).unwrap()
        else {
            panic!("canonical content stays non-empty");
        };
        assert_eq!(second.content(), first.content());
        validate_persistent_rich_text(1, second.content()).unwrap();
    }

    #[test]
    fn normalization_maps_empty_to_unset_and_rejects_unknown_discriminators() {
        let empty = root(vec![paragraph(vec![text("")])]);
        assert_eq!(
            normalize_rich_text(1, &empty).unwrap(),
            NormalizedRichText::Unset
        );

        for fixture in [
            json!({"kind":"root","children":[{"kind":"futureNode","children":[]}]}),
            json!({"kind":"root","children":[paragraph(vec![json!({
                "kind":"text","text":"x","marks":["link"]
            })])]}),
            json!({"kind":"root","children":[paragraph(vec![json!({
                "kind":"text","text":"","marks":["link"]
            })])]}),
        ] {
            assert!(normalize_rich_text(1, &content(fixture)).is_err());
        }
    }

    #[test]
    fn shared_json_depth_and_reserved_namespace_are_enforced() {
        fn with_nested_extra(array_depth: usize) -> Map<String, Value> {
            let mut nested = Value::Null;
            for _ in 0..array_depth {
                nested = Value::Array(vec![nested]);
            }
            let mut value = root(vec![paragraph(vec![text("본문")])]);
            value.insert("future".to_owned(), nested);
            value
        }

        validate_persistent_rich_text(1, &with_nested_extra(MAX_JSON_NESTING_DEPTH - 1)).unwrap();
        assert_eq!(
            validate_persistent_rich_text(1, &with_nested_extra(MAX_JSON_NESTING_DEPTH))
                .unwrap_err()
                .category(),
            RichTextValidationErrorCategory::NestingDepthExceeded
        );

        let mut reserved = root(vec![paragraph(vec![text("본문")])]);
        reserved.insert(
            "$serde_json::private::FutureTransport".to_owned(),
            json!({"credential":"must not leak"}),
        );
        assert_eq!(
            validate_persistent_rich_text(1, &reserved)
                .unwrap_err()
                .category(),
            RichTextValidationErrorCategory::ReservedExtraKey
        );
    }

    #[test]
    fn errors_and_sources_redact_payload_discriminator_url_and_path() {
        let secret = "credential=rich-secret";
        let path = "C:\\Users\\audit\\rich-text.json";
        let url = "https://example.invalid/private?token=secret";
        let fixture = content(json!({
            "kind":"root",
            "children":[{"kind":secret,"children":[]}],
            "future":{"path":path,"url":url}
        }));
        let error = validate_persistent_rich_text(1, &fixture).unwrap_err();
        for forbidden in [secret, path, url, "token=secret"] {
            assert!(!error.to_string().contains(forbidden));
            assert!(!format!("{error:?}").contains(forbidden));
        }
        assert!(error.source().is_none());
    }
}
