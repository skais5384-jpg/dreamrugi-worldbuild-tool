//! 문서 본문과 독립적인 배치. 모든 변경은 사본에서 검증한 뒤 한 번 공개한다.
use super::{schema::ArtifactTypeWire, ArtifactType, DocumentId};
use crate::data::{json::LosslessJsonValue, utc_time::is_utc_milliseconds};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DocumentLayout {
    pub(crate) schema_version: u32,
    pub(super) artifact_type: ArtifactTypeWire,
    pub(crate) revision: u32,
    pub(crate) root_order: Vec<DocumentId>,
    pub(crate) nodes: BTreeMap<DocumentId, LayoutNode>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
    #[serde(skip)]
    pub(super) source: Option<LosslessJsonValue>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LayoutNode {
    pub(crate) parent_id: Option<DocumentId>,
    pub(crate) child_order: Vec<DocumentId>,
    pub(crate) state: LayoutState,
    pub(crate) trash: Option<TrashPosition>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum LayoutState {
    Active,
    Trashed,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrashPosition {
    pub(crate) trashed_at_utc: String,
    pub(crate) parent_id: Option<DocumentId>,
    pub(crate) index: u32,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LayoutError {
    Header,
    Revision,
    Membership,
    Parent,
    Cycle,
    HasChildren,
    State,
    Timestamp,
    RestoreDestination,
}
#[derive(Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum LayoutEdit {
    Adopt {},
    Move {
        document: DocumentId,
        parent: Option<DocumentId>,
        index: u32,
    },
    Trash {
        document: DocumentId,
    },
    Restore {
        document: DocumentId,
        destination: Option<RestoreDestination>,
    },
    /// 휴지통 문서의 물리 삭제와 함께 쓰는 내부 구조 변경이다. 활성 문서나
    /// 자식이 남은 문서는 이 단계에 도달할 수 없다.
    Purge {
        document: DocumentId,
    },
}
#[derive(Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RestoreDestination {
    pub(crate) parent: Option<DocumentId>,
    pub(crate) index: u32,
}

impl DocumentLayout {
    pub(crate) fn flat(ids: impl IntoIterator<Item = DocumentId>) -> Self {
        let ids: BTreeSet<_> = ids.into_iter().collect();
        Self {
            schema_version: 1,
            artifact_type: ArtifactTypeWire::DocumentLayout,
            revision: 1,
            root_order: ids.iter().copied().collect(),
            nodes: ids
                .into_iter()
                .map(|id| (id, LayoutNode::active(None)))
                .collect(),
            extra: BTreeMap::new(),
            source: None,
        }
    }
    pub(crate) fn validate(&self) -> Result<(), LayoutError> {
        if self.schema_version != 1
            || ArtifactType::from(self.artifact_type) != ArtifactType::DocumentLayout
        {
            return Err(LayoutError::Header);
        }
        if self.revision == 0 {
            return Err(LayoutError::Revision);
        }
        let mut seen = BTreeSet::new();
        for (parent, order) in std::iter::once((None, &self.root_order))
            .chain(self.nodes.iter().map(|(id, n)| (Some(*id), &n.child_order)))
        {
            for id in order {
                let n = self.nodes.get(id).ok_or(LayoutError::Membership)?;
                if !seen.insert(*id) {
                    return Err(LayoutError::Membership);
                }
                if n.state != LayoutState::Active || n.parent_id != parent {
                    return Err(LayoutError::Parent);
                }
                if let Some(parent) = parent {
                    if self.nodes[&parent].state != LayoutState::Active {
                        return Err(LayoutError::Parent);
                    }
                }
            }
        }
        for (id, n) in &self.nodes {
            match n.state {
                LayoutState::Active => {
                    if !seen.contains(id) || n.trash.is_some() {
                        return Err(LayoutError::Membership);
                    }
                }
                LayoutState::Trashed => {
                    if seen.contains(id) || n.parent_id.is_some() || !n.child_order.is_empty() {
                        return Err(LayoutError::State);
                    }
                    let trash = n.trash.as_ref().ok_or(LayoutError::State)?;
                    if !is_utc_milliseconds(&trash.trashed_at_utc) {
                        return Err(LayoutError::Timestamp);
                    }
                }
            }
        }
        // 양방향/중복 검사가 끝났으므로 root에서 한 번만 순회해 고립 cycle을 검출한다.
        let mut reachable = BTreeSet::new();
        let mut pending = self.root_order.clone();
        while let Some(id) = pending.pop() {
            if !reachable.insert(id) {
                return Err(LayoutError::Cycle);
            }
            pending.extend(self.nodes[&id].child_order.iter().copied());
        }
        if self
            .nodes
            .iter()
            .any(|(id, n)| n.state == LayoutState::Active && !reachable.contains(id))
        {
            return Err(LayoutError::Cycle);
        }
        Ok(())
    }
    pub(crate) fn reconcile(
        &self,
        ids: &BTreeSet<DocumentId>,
    ) -> Result<Vec<DocumentId>, LayoutError> {
        self.validate()?;
        if self.nodes.keys().any(|id| !ids.contains(id)) {
            return Err(LayoutError::Membership);
        }
        Ok(ids
            .iter()
            .filter(|id| !self.nodes.contains_key(id))
            .copied()
            .collect())
    }
    fn order(&mut self, parent: Option<DocumentId>) -> Result<&mut Vec<DocumentId>, LayoutError> {
        match parent {
            None => Ok(&mut self.root_order),
            Some(id) => {
                let n = self.nodes.get_mut(&id).ok_or(LayoutError::Parent)?;
                if n.state != LayoutState::Active {
                    return Err(LayoutError::Parent);
                }
                Ok(&mut n.child_order)
            }
        }
    }
    pub(crate) fn insert(
        &mut self,
        id: DocumentId,
        parent: Option<DocumentId>,
    ) -> Result<(), LayoutError> {
        if self.nodes.contains_key(&id) {
            return Err(LayoutError::Membership);
        }
        self.order(parent)?.push(id);
        self.nodes.insert(id, LayoutNode::active(parent));
        self.validate()
    }
    pub(crate) fn changed(
        &self,
        ids: &BTreeSet<DocumentId>,
        edit: &LayoutEdit,
        now: &str,
    ) -> Result<Self, LayoutError> {
        let missing = self.reconcile(ids)?;
        let mut next = self.clone();
        match edit {
            // 외부 문서의 배치는 명시적인 메뉴 동작에만 속한다. 다른 이동에 섞지 않는다.
            LayoutEdit::Adopt {} => {
                for id in missing {
                    next.root_order.push(id);
                    next.nodes.insert(id, LayoutNode::active(None));
                }
            }
            LayoutEdit::Move {
                document,
                parent,
                index,
            } => {
                let node = next.nodes.get(document).ok_or(LayoutError::Membership)?;
                if node.state != LayoutState::Active {
                    return Err(LayoutError::State);
                }
                let old = node.parent_id;
                next.order(old)?.retain(|id| id != document);
                let order = next.order(*parent)?;
                let at = (*index as usize).min(order.len());
                order.insert(at, *document);
                next.nodes
                    .get_mut(document)
                    .ok_or(LayoutError::Membership)?
                    .parent_id = *parent;
            }
            LayoutEdit::Trash { document } => {
                let node = next.nodes.get(document).ok_or(LayoutError::Membership)?;
                if node.state != LayoutState::Active {
                    return Err(LayoutError::State);
                }
                if !node.child_order.is_empty() {
                    return Err(LayoutError::HasChildren);
                }
                if !is_utc_milliseconds(now) {
                    return Err(LayoutError::Timestamp);
                }
                let parent = node.parent_id;
                let order = next.order(parent)?;
                let index = order
                    .iter()
                    .position(|id| id == document)
                    .ok_or(LayoutError::Membership)?;
                order.remove(index);
                let node = next
                    .nodes
                    .get_mut(document)
                    .ok_or(LayoutError::Membership)?;
                node.state = LayoutState::Trashed;
                node.parent_id = None;
                node.trash = Some(TrashPosition {
                    trashed_at_utc: now.into(),
                    parent_id: parent,
                    index: u32::try_from(index).map_err(|_| LayoutError::Membership)?,
                    extra: BTreeMap::new(),
                });
            }
            LayoutEdit::Restore {
                document,
                destination,
            } => {
                let node = next.nodes.get(document).ok_or(LayoutError::Membership)?;
                if node.state != LayoutState::Trashed {
                    return Err(LayoutError::State);
                }
                let old = node.trash.as_ref().ok_or(LayoutError::State)?;
                let (parent, index) = destination
                    .as_ref()
                    .map(|d| (d.parent, d.index))
                    .unwrap_or((old.parent_id, old.index));
                let order = next
                    .order(parent)
                    .map_err(|_| LayoutError::RestoreDestination)?;
                let at = (index as usize).min(order.len());
                order.insert(at, *document);
                let node = next
                    .nodes
                    .get_mut(document)
                    .ok_or(LayoutError::Membership)?;
                node.parent_id = parent;
                node.state = LayoutState::Active;
                node.trash = None;
            }
            LayoutEdit::Purge { document } => {
                let node = next.nodes.get(document).ok_or(LayoutError::Membership)?;
                if node.state != LayoutState::Trashed || !node.child_order.is_empty() {
                    return Err(LayoutError::State);
                }
                next.nodes.remove(document);
            }
        }
        next.validate()?;
        if next != *self {
            next.bump()?;
        }
        Ok(next)
    }
    pub(crate) fn bump(&mut self) -> Result<(), LayoutError> {
        self.revision = self.revision.checked_add(1).ok_or(LayoutError::Revision)?;
        Ok(())
    }
}
impl LayoutNode {
    fn active(parent_id: Option<DocumentId>) -> Self {
        Self {
            parent_id,
            child_order: vec![],
            state: LayoutState::Active,
            trash: None,
            extra: BTreeMap::new(),
        }
    }
}
impl std::fmt::Debug for DocumentLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DocumentLayout([private])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::artifact::{decode_layout, encode_layout};
    const NOW: &str = "2026-09-15T01:02:03.004Z";
    #[test]
    fn only_explicit_adopt_places_unplaced_documents() {
        let a = DocumentId::new();
        let b = DocumentId::new();
        let external = DocumentId::new();
        let ids = [a, b, external].into();
        let base = DocumentLayout::flat([a, b]);
        let moved = base
            .changed(
                &ids,
                &LayoutEdit::Move {
                    document: b,
                    parent: Some(a),
                    index: 0,
                },
                NOW,
            )
            .unwrap();
        let trashed = moved
            .changed(&ids, &LayoutEdit::Trash { document: b }, NOW)
            .unwrap();
        let restored = trashed
            .changed(
                &ids,
                &LayoutEdit::Restore {
                    document: b,
                    destination: None,
                },
                NOW,
            )
            .unwrap();
        for layout in [&moved, &trashed, &restored] {
            assert_eq!(layout.reconcile(&ids).unwrap(), vec![external]);
            assert!(!layout.nodes.contains_key(&external));
        }
        let adopted = restored.changed(&ids, &LayoutEdit::Adopt {}, NOW).unwrap();
        assert!(adopted.reconcile(&ids).unwrap().is_empty());
        assert_eq!(adopted.root_order.last(), Some(&external));
    }
    #[test]
    fn purge_only_removes_an_explicit_trashed_leaf() {
        let id = DocumentId::new();
        let ids = [id].into();
        let active = DocumentLayout::flat([id]);
        assert_eq!(
            active.changed(&ids, &LayoutEdit::Purge { document: id }, NOW),
            Err(LayoutError::State)
        );
        let trashed = active
            .changed(&ids, &LayoutEdit::Trash { document: id }, NOW)
            .unwrap();
        let purged = trashed
            .changed(&ids, &LayoutEdit::Purge { document: id }, NOW)
            .unwrap();
        assert!(!purged.nodes.contains_key(&id));
        assert!(purged.root_order.is_empty());
        assert_eq!(purged.revision, trashed.revision + 1);
    }
    #[test]
    fn layout_codec_relations_unknown_future_time_and_integer_boundaries() {
        let a = DocumentId::new();
        let b = DocumentId::new();
        let base = DocumentLayout::flat([a, b]);
        let bytes = encode_layout(&base).unwrap();
        let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for case in [
            "duplicate",
            "missing",
            "parent",
            "cycle",
            "state",
            "revision",
            "future",
            "timestamp",
        ] {
            let mut v = original.clone();
            let sa = a.to_string();
            let sb = b.to_string();
            match case {
                "duplicate" => v["rootOrder"] = serde_json::json!([a, a, b]),
                "missing" => v["rootOrder"] = serde_json::json!([a]),
                "parent" => v["nodes"][&sa]["parentId"] = serde_json::json!(b),
                "cycle" => {
                    v["rootOrder"] = serde_json::json!([]);
                    v["nodes"][&sa]["parentId"] = serde_json::json!(b);
                    v["nodes"][&sb]["parentId"] = serde_json::json!(a);
                    v["nodes"][&sa]["childOrder"] = serde_json::json!([b]);
                    v["nodes"][&sb]["childOrder"] = serde_json::json!([a]);
                }
                "state" => v["nodes"][&sa]["state"] = "future".into(),
                "revision" => v["revision"] = serde_json::json!(4294967296u64),
                "future" => v["schemaVersion"] = 2.into(),
                "timestamp" => {
                    v["rootOrder"] = serde_json::json!([b]);
                    v["nodes"][&sa]["state"] = "trashed".into();
                    v["nodes"][&sa]["trash"] = serde_json::json!({"parentId":null,"index":0,"trashedAtUtc":"2026-09-15T01:02:03Z"});
                }
                _ => unreachable!(),
            }
            assert!(
                decode_layout(&serde_json::to_vec(&v).unwrap()).is_err(),
                "{case}"
            );
        }
        let raw = String::from_utf8(bytes).unwrap().replacen(
            '{',
            "{\"extension\":{\"large\":9007199254740993,\"fraction\":1.2300},",
            1,
        );
        let decoded = decode_layout(raw.as_bytes()).unwrap();
        let changed = decoded
            .changed(&[a, b].into(), &LayoutEdit::Trash { document: a }, NOW)
            .unwrap();
        let encoded = String::from_utf8(encode_layout(&changed).unwrap()).unwrap();
        assert!(encoded.contains("9007199254740993"));
        assert!(encoded.contains("1.2300"));
        assert!(decode_layout(encoded.as_bytes()).is_ok());
        let mut maximum = base;
        maximum.revision = u32::MAX;
        assert!(maximum
            .changed(&[a, b].into(), &LayoutEdit::Adopt {}, NOW)
            .is_ok());
        assert_eq!(
            maximum
                .changed(&[a, b].into(), &LayoutEdit::Trash { document: a }, NOW)
                .unwrap_err(),
            LayoutError::Revision
        );
    }
    #[test]
    fn layout_unplaced_missing_and_deep_tree_are_distinct() {
        let ids: Vec<_> = (0..2000).map(|_| DocumentId::new()).collect();
        let mut l = DocumentLayout::flat(ids.iter().copied());
        l.root_order = vec![ids[0]];
        for i in 1..ids.len() {
            l.nodes.get_mut(&ids[i]).unwrap().parent_id = Some(ids[i - 1]);
            l.nodes.get_mut(&ids[i - 1]).unwrap().child_order = vec![ids[i]];
        }
        l.validate().unwrap();
        let extra = DocumentId::new();
        let mut all: BTreeSet<_> = ids.iter().copied().collect();
        all.insert(extra);
        assert_eq!(l.reconcile(&all).unwrap(), vec![extra]);
        all.remove(&ids[0]);
        assert_eq!(l.reconcile(&all), Err(LayoutError::Membership));
    }
}
