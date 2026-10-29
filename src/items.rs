//! Project items as read by id (`nodes(ids:)`, spec/board#item-ids-are-the-source-of-truth).
//!
//! Read side of the board. `board.rs` writes the fields; `ticket/` and `ops/status.rs` use
//! the items.

use crate::base::GhError;
use serde_json::Value;
use std::collections::BTreeMap;

/// GraphQL selection for one item (used by `nodes(ids:)` and by listings).
pub const ITEM_FRAGMENT: &str = "id updatedAt type isArchived project{number} content{... on Issue{state}} \
fieldValues(first:30){nodes{__typename \
... on ProjectV2ItemFieldTextValue{text field{... on ProjectV2FieldCommon{name}}} \
... on ProjectV2ItemFieldSingleSelectValue{name field{... on ProjectV2FieldCommon{name}}} \
... on ProjectV2ItemFieldNumberValue{number field{... on ProjectV2FieldCommon{name}}}}}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemSnapshot {
    pub id: String,
    pub updated_at: String,
    pub item_type: String,
    pub project_number: Option<u64>,
    /// `OPEN` or `CLOSED` for an item that is an issue; `None` for a draft item.
    pub issue_state: Option<String>,
    /// Field name -> text or option name. Fields without a value are absent.
    pub fields: BTreeMap<String, String>,
}

impl ItemSnapshot {
    /// The issue behind the item is closed. `false` for an open issue and for a draft item.
    pub fn issue_closed(&self) -> bool {
        self.issue_state.as_deref() == Some("CLOSED")
    }

    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

/// A number field value as text: a whole number has no decimal point (`3`, not `3.0`).
fn number_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Parse one element of a `nodes(ids:)` answer. `null` (unknown id) is `Ok(None)`.
pub fn parse_item_node(v: &Value) -> Result<Option<ItemSnapshot>, GhError> {
    if v.is_null() {
        return Ok(None);
    }
    let shape = |m: &str| GhError::Shape(format!("item: {m}"));
    let s = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| shape(&format!("`{k}` missing")))
    };
    let mut fields = BTreeMap::new();
    let nodes = v
        .pointer("/fieldValues/nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| shape("`fieldValues.nodes` missing"))?;
    for n in nodes {
        // Value types we do not read (dates, numbers, ...) come back as `{}`.
        let Some(name) = n.pointer("/field/name").and_then(Value::as_str) else {
            continue;
        };
        let val = n
            .get("text")
            .and_then(Value::as_str)
            .or_else(|| n.get("name").and_then(Value::as_str))
            .map(str::to_string)
            .or_else(|| n.get("number").and_then(Value::as_f64).map(number_text));
        if let Some(val) = val {
            fields.insert(name.to_string(), val);
        }
    }
    Ok(Some(ItemSnapshot {
        id: s("id")?,
        updated_at: s("updatedAt")?,
        item_type: s("type")?,
        project_number: v.pointer("/project/number").and_then(Value::as_u64),
        issue_state: v
            .pointer("/content/state")
            .and_then(Value::as_str)
            .map(str::to_string),
        fields,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node() -> Value {
        json!({
            "id": "PVTI_x", "updatedAt": "2026-10-15T14:00:00Z", "type": "DRAFT_ISSUE",
            "isArchived": false, "project": {"number": 15},
            "fieldValues": {"nodes": [
                {"__typename": "ProjectV2ItemFieldTextValue", "text": "ssh-ed25519 AAAA x@y", "field": {"name": "Key"}},
                {"__typename": "ProjectV2ItemFieldSingleSelectValue", "name": "requested", "field": {"name": "State"}},
                {}
            ]}
        })
    }

    #[test]
    fn parses_text_and_select_values_and_skips_unknown_types() {
        let it = parse_item_node(&node()).unwrap().unwrap();
        assert_eq!(it.field("Key"), Some("ssh-ed25519 AAAA x@y"));
        assert_eq!(it.field("State"), Some("requested"));
        assert_eq!(it.field("Role"), None);
        assert_eq!(it.project_number, Some(15));
        assert_eq!(it.fields.len(), 2);
    }

    #[test]
    fn the_issue_state_comes_from_the_content_and_a_draft_has_none() {
        let mut n = node();
        assert!(!parse_item_node(&n).unwrap().unwrap().issue_closed());
        assert_eq!(parse_item_node(&n).unwrap().unwrap().issue_state, None);
        n["content"] = json!({"state": "CLOSED"});
        assert!(parse_item_node(&n).unwrap().unwrap().issue_closed());
        n["content"] = json!({"state": "OPEN"});
        assert!(!parse_item_node(&n).unwrap().unwrap().issue_closed());
        n["content"] = json!({});
        assert_eq!(parse_item_node(&n).unwrap().unwrap().issue_state, None);
    }

    #[test]
    fn a_number_value_is_read_as_text_without_a_decimal_point() {
        let mut n = node();
        n["fieldValues"]["nodes"].as_array_mut().unwrap().extend([
            json!({"__typename": "ProjectV2ItemFieldNumberValue", "number": 3.0, "field": {"name": "Order"}}),
            json!({"__typename": "ProjectV2ItemFieldNumberValue", "number": 2.5, "field": {"name": "Weight"}}),
        ]);
        let it = parse_item_node(&n).unwrap().unwrap();
        assert_eq!(it.field("Order"), Some("3"));
        assert_eq!(it.field("Weight"), Some("2.5"));
        // control: a number node without a value adds nothing
        let mut n = node();
        n["fieldValues"]["nodes"].as_array_mut().unwrap().push(
            json!({"__typename": "ProjectV2ItemFieldNumberValue", "field": {"name": "Order"}}),
        );
        assert_eq!(parse_item_node(&n).unwrap().unwrap().field("Order"), None);
    }

    #[test]
    fn null_node_is_an_unknown_id_and_broken_nodes_are_errors() {
        assert_eq!(parse_item_node(&Value::Null).unwrap(), None);
        let mut n = node();
        n.as_object_mut().unwrap().remove("updatedAt");
        assert!(parse_item_node(&n).is_err());
        let mut n = node();
        n.as_object_mut().unwrap().remove("fieldValues");
        assert!(parse_item_node(&n).is_err());
    }
}
