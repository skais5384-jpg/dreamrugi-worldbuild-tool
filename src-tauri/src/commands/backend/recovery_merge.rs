//! Read-only three-way comparison of supported draft projections. A plan grants no
//! write authority: callers must reload source tokens, acquire owners and use normal
//! artifact preparation. Unknown canonical storage stays with those source owners.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Change {
    pub(crate) id: String,
    pub(crate) path: Vec<String>,
    pub(crate) status: &'static str,
    pub(crate) original: Option<Value>,
    pub(crate) current: Option<Value>,
    pub(crate) preserved: Option<Value>,
    pub(crate) reason: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Plan {
    pub(crate) changes: Vec<Change>,
    current: Value,
    preserved: Value,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Apply {
    pub(crate) template_digest: String,
    pub(crate) document_digest: Option<String>,
    pub(crate) selected: Vec<String>,
}

// Only schema-owned identity arrays are keyed. Text nodes and primitive arrays are
// atomic values, never matched by their index or by a user-visible label.
fn normalize(value: &Value, key: &str) -> Value {
    match value {
        Value::Array(items)
            if matches!(
                key,
                "fields" | "members" | "options" | "sections" | "instances"
            ) =>
        {
            let identity =
                if key == "fields" && items.first().is_some_and(|i| i.get("field").is_some()) {
                    "field"
                } else {
                    "id"
                };
            if items
                .iter()
                .all(|item| item.get(identity).and_then(Value::as_str).is_some())
            {
                let mut map = Map::new();
                let mut order = vec![];
                for item in items {
                    let id = item[identity].as_str().unwrap();
                    // Duplicated IDs are not silently reduced to one candidate.
                    if map.contains_key(id) {
                        return value.clone();
                    }
                    map.insert(id.into(), normalize(item, ""));
                    order.push(Value::String(id.into()));
                }
                serde_json::json!({"$items":map,"$order":order})
            } else {
                value.clone()
            }
        }
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), normalize(value, key)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn denormalize(value: Value) -> Value {
    match value {
        Value::Object(mut object)
            if object.contains_key("$items") && object.contains_key("$order") =>
        {
            let Some(Value::Object(mut items)) = object.remove("$items") else {
                return Value::Object(object);
            };
            let Some(Value::Array(order)) = object.remove("$order") else {
                return Value::Object(object);
            };
            let mut out = vec![];
            for id in order.iter().filter_map(Value::as_str) {
                if let Some(item) = items.remove(id) {
                    out.push(denormalize(item));
                }
            }
            // Unselected additions from the current source must not disappear.
            out.extend(items.into_values().map(denormalize));
            Value::Array(out)
        }
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, denormalize(value)))
                .collect(),
        ),
        _ => value,
    }
}

fn at<'a>(value: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut current = value;
    for key in path {
        current = current.get(key)?;
    }
    Some(current)
}

fn semantic_projection(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .filter(|(key, _)| {
                    !matches!(
                        key.as_str(),
                        "composing"
                            | "restore"
                            | "archiveIndex"
                            | "archiveOrder"
                            | "archiveTitle"
                            | "source"
                            | "lineage"
                            | "protected"
                            | "labels"
                    )
                })
                .map(|(key, value)| (key, semantic_projection(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(semantic_projection).collect()),
        value => value,
    }
}

fn put(value: &mut Value, path: &[String], replacement: Option<Value>) -> Result<(), ()> {
    if path.is_empty() {
        *value = replacement.ok_or(())?;
        return Ok(());
    }
    let mut parent = value;
    for key in &path[..path.len() - 1] {
        parent = parent.get_mut(key).ok_or(())?;
    }
    let object = parent.as_object_mut().ok_or(())?;
    if let Some(next) = replacement {
        object.insert(path.last().unwrap().clone(), next);
    } else {
        object.remove(path.last().unwrap());
    }
    Ok(())
}

fn compare(
    base: Option<&Value>,
    current: Option<&Value>,
    preserved: Option<&Value>,
    path: &mut Vec<String>,
    out: &mut Vec<Change>,
) {
    if base == preserved || current == preserved {
        return;
    }
    // Intent values are one logical property: Keep/Set/Unset cannot be split into
    // independent checkbox fragments. Group cells are supplied as separate drafts.
    let group_intent = |v: Option<&Value>| {
        v.is_some_and(|v| {
            v.get("intent").and_then(Value::as_str) == Some("set")
                && v.get("value")
                    .and_then(|v| v.get("kind"))
                    .and_then(Value::as_str)
                    == Some("group")
        })
    };
    let atomic = preserved.is_some_and(|v| v.get("intent").is_some())
        && !(group_intent(base) && group_intent(current) && group_intent(preserved))
        || path.last().is_some_and(|k| k == "$order")
        || base.is_none()
        || current.is_none()
        || preserved.is_none();
    if !atomic
        && base.is_some_and(Value::is_object)
        && current.is_some_and(Value::is_object)
        && preserved.is_some_and(Value::is_object)
    {
        let keys: BTreeSet<_> = base
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .chain(preserved.unwrap().as_object().unwrap().keys())
            .cloned()
            .collect();
        for key in keys {
            if matches!(
                key.as_str(),
                "composing"
                    | "restore"
                    | "archiveIndex"
                    | "archiveOrder"
                    | "archiveTitle"
                    | "source"
                    | "lineage"
                    | "protected"
                    | "labels"
            ) {
                continue;
            }
            path.push(key.clone());
            compare(
                base.unwrap().get(&key),
                current.unwrap().get(&key),
                preserved.unwrap().get(&key),
                path,
                out,
            );
            path.pop();
        }
        return;
    }
    out.push(Change {
        id: serde_json::to_string(path).unwrap(),
        path: path.clone(),
        status: if current == base {
            "proposed"
        } else {
            "conflict"
        },
        original: base.cloned(),
        current: current.cloned(),
        preserved: preserved.cloned(),
        reason: None,
    });
}

impl Plan {
    pub(crate) fn new(
        base: Value,
        current_semantic: Value,
        preserved_semantic: Value,
        current_raw: Value,
        preserved_raw: Value,
    ) -> Self {
        let base = semantic_projection(normalize(&base, ""));
        let current_semantic = semantic_projection(normalize(&current_semantic, ""));
        let preserved_semantic = semantic_projection(normalize(&preserved_semantic, ""));
        let mut changes = vec![];
        compare(
            Some(&base),
            Some(&current_semantic),
            Some(&preserved_semantic),
            &mut vec![],
            &mut changes,
        );
        for change in &mut changes {
            change.original = change.original.take().map(denormalize);
            change.current = change.current.take().map(denormalize);
            change.preserved = change.preserved.take().map(denormalize);
        }
        Self {
            changes,
            current: normalize(&current_raw, ""),
            preserved: normalize(&preserved_raw, ""),
        }
    }
    pub(crate) fn apply(&self, selected: &[String]) -> Result<Value, ()> {
        let unique: BTreeSet<_> = selected.iter().collect();
        if unique.len() != selected.len()
            || selected.iter().any(|id| {
                !self
                    .changes
                    .iter()
                    .any(|c| &c.id == id && c.status != "blocked")
            })
        {
            return Err(());
        }
        let mut result = self.current.clone();
        // Definition additions precede their child/order changes; neither current
        // additions nor missing parent objects are reconstructed by guessing.
        for change in &self.changes {
            if !unique.contains(&change.id) {
                continue;
            }
            let mut replacement = at(&self.preserved, &change.path).cloned();
            if change.path.last().is_some_and(|key| key == "$order") {
                if let (Some(Value::Array(current)), Some(Value::Array(chosen))) =
                    (at(&result, &change.path), replacement.as_mut())
                {
                    let old: BTreeSet<_> = change
                        .original
                        .as_ref()
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect();
                    for id in current {
                        if id.as_str().is_some_and(|id| !old.contains(id)) && !chosen.contains(id) {
                            chosen.push(id.clone());
                        }
                    }
                }
            }
            put(&mut result, &change.path, replacement)?;
        }
        Ok(denormalize(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn different_fields_and_current_new_definitions_survive_selected_recovery() {
        let base = json!({"fields":[{"id":"a","label":"old"},{"id":"b","label":"old"}]});
        let current = json!({"fields":[{"id":"a","label":"old"},{"id":"b","label":"current"},{"id":"c","label":"new current"}]});
        let preserved = json!({"fields":[{"id":"b","label":"old"},{"id":"a","label":"input"},{"id":"d","label":"new input"}]});
        let plan = Plan::new(base, current.clone(), preserved.clone(), current, preserved);
        let choices = plan
            .changes
            .iter()
            .map(|c| c.id.clone())
            .collect::<Vec<_>>();
        let applied = plan.apply(&choices).unwrap();
        let fields = applied["fields"].as_array().unwrap();
        assert_eq!(
            fields
                .iter()
                .map(|f| f["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["b", "a", "d", "c"]
        );
        assert_eq!(fields[0]["label"], "current");
        assert_eq!(fields[1]["label"], "input");
        assert!(plan.apply(&["forged".into()]).is_err());
    }
    #[test]
    fn conflict_defaults_to_current_and_partial_selection_is_not_full_snapshot_replacement() {
        let base = json!({"name":"base","fields":[{"id":"a","label":"base"}]});
        let current = json!({"name":"current","fields":[{"id":"a","label":"base"}]});
        let input = json!({"name":"input","fields":[{"id":"a","label":"input"}]});
        let plan = Plan::new(base, current.clone(), input.clone(), current.clone(), input);
        assert_eq!(
            plan.changes
                .iter()
                .find(|c| c.path == vec!["name"])
                .unwrap()
                .status,
            "conflict"
        );
        assert_eq!(plan.apply(&[]).unwrap(), current);
        let label = plan
            .changes
            .iter()
            .find(|c| c.path.last().is_some_and(|v| v == "label"))
            .unwrap()
            .id
            .clone();
        let applied = plan.apply(&[label]).unwrap();
        assert_eq!(applied["name"], "current");
        assert_eq!(applied["fields"][0]["label"], "input");
    }
}
