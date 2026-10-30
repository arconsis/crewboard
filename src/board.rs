//! The board (GitHub Project v2): schema, item reads by id, field writes.
//!
//! Rules built in here (spec/board#fields, spec/board#item-ids-are-the-source-of-truth):
//!
//! - Every field value is written by [`Board::write`], which asks the field-writer table
//!   ([`check_field_write`]) first: "one writer per field" holds by construction. Two fields
//!   are the exception: `Status` and `State` are written by [`Board::write_status`] and
//!   [`Board::write_state`], which run no check of their own. Their gate is the transition
//!   check of the caller (`rules::status`, `rules::lifecycle`). A field is a [`Field`], never a
//!   string.
//! - Every text write checks the 1024-byte limit first
//!   ([`crate::rules::limits::check_text_field`]).
//! - Single-select options are written only by [`Board::write_options`]: it merges with the
//!   existing ids (the complete-list rule), writes, reads back, and checks with
//!   `verify_ids_kept` that no earlier id was lost.
//! - Known items are read by id (`nodes(ids:)`). The item listing is used only to
//!   discover items and to report items that are missing from it.

use crate::base::ids;
use crate::config::split_repo;
use crate::error::{CbError, CbResult};
use crate::github::{Client, GhError};
use crate::items::{ITEM_FRAGMENT, ItemSnapshot, parse_item_node};
use crate::rules::fields::{Field, FieldType, Item, WriteCtx, check_field_write};
use crate::rules::lifecycle::State;
use crate::rules::limits::check_text_field;
use crate::rules::options::{
    DesiredOption, OptionEntry, check_option_limit, names_missing_after, rebase_on_fresh,
    verify_ids_kept, verify_ids_kept_except,
};
use crate::rules::status::Status;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionInfo {
    pub id: String,
    pub name: String,
    pub color: String,
    pub description: String,
}

/// A field by its name on the board. Crew code names a field with a [`Field`]; a test can name a
/// scratch field of its own. There is no impl for `str`, so a misspelt field name does not
/// compile.
pub trait FieldName {
    fn board_name(&self) -> &str;
}

impl FieldName for Field {
    fn board_name(&self) -> &str {
        self.name()
    }
}

/// The item that a field write goes to: a ticket (`Work`) or an agent item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemRef<'a> {
    pub item: Item,
    pub id: &'a str,
}

impl<'a> ItemRef<'a> {
    pub fn work(id: &'a str) -> Self {
        ItemRef {
            item: Item::Work,
            id,
        }
    }

    pub fn agent(id: &'a str) -> Self {
        ItemRef {
            item: Item::Agent,
            id,
        }
    }
}

/// A value for [`Board::write`]. Its type must be the type of the field ([`Field::kind`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldValue<'a> {
    Text(&'a str),
    /// A single-select option by name. The option must exist.
    Option(&'a str),
    Number(i64),
    /// No value (for example the `Agent` of a reassigned ticket).
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInfo {
    pub id: String,
    pub name: String,
    pub data_type: String,
    pub options: Vec<OptionInfo>,
}

impl FieldInfo {
    pub fn entries(&self) -> Vec<OptionEntry> {
        self.options
            .iter()
            .map(|o| OptionEntry {
                id: o.id.clone(),
                name: o.name.clone(),
            })
            .collect()
    }

    pub fn option_id(&self, name: &str) -> Option<&str> {
        self.options
            .iter()
            .find(|o| o.name == name)
            .map(|o| o.id.as_str())
    }
}

/// The value of one single-select field on one listed item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedValue {
    pub item_id: String,
    pub option_id: Option<String>,
    pub name: Option<String>,
}

/// The draft issue of a draft item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub id: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerKind {
    Org,
    User,
}

#[derive(Debug, Clone)]
pub struct Board {
    pub project_id: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub public: bool,
    pub owner: String,
    pub owner_kind: OwnerKind,
    pub fields: BTreeMap<String, FieldInfo>,
}

const PROJECT_FIELDS: &str = "id number title url public fields(first:50){nodes{__typename \
... on ProjectV2FieldCommon{id name dataType} \
... on ProjectV2SingleSelectField{options{id name color description}}}}";

/// Who owns a project: an organisation or a user, with the GraphQL node id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerInfo {
    pub kind: OwnerKind,
    pub node_id: String,
}

/// The REST base path of the projects of an owner: `orgs/<login>` or `users/<login>`. Both
/// the field list and the view creation take the LOGIN. GitHub's docs name the user form
/// `POST /users/{user_id}/projectsV2/{n}/views`, but the numeric id answers 404 and the login
/// works (verified live on 2026-10-16 against a user-owned project).
pub fn rest_base(kind: &OwnerKind, login: &str) -> String {
    match kind {
        OwnerKind::Org => format!("orgs/{login}"),
        OwnerKind::User => format!("users/{login}"),
    }
}

/// The GraphQL root field of the owner: `organization` or `user`.
pub fn graphql_root(kind: &OwnerKind) -> &'static str {
    match kind {
        OwnerKind::Org => "organization",
        OwnerKind::User => "user",
    }
}

/// Look up the owner of a project: organisation or user.
pub fn owner_info(client: &Client, login: &str) -> CbResult<OwnerInfo> {
    let r = client.get(&format!("users/{login}"))?;
    let kind = match r.body.get("type").and_then(Value::as_str) {
        Some("Organization") => OwnerKind::Org,
        Some("User") => OwnerKind::User,
        other => {
            return Err(GhError::Shape(format!("owner type {other:?} of {login}")).into());
        }
    };
    let node_id = r
        .body
        .get("node_id")
        .and_then(Value::as_str)
        .ok_or_else(|| GhError::Shape("owner has no node_id".into()))?;
    Ok(OwnerInfo {
        kind,
        node_id: node_id.to_string(),
    })
}

fn parse_fields(v: &Value) -> CbResult<BTreeMap<String, FieldInfo>> {
    let nodes = v
        .pointer("/fields/nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| GhError::Shape("project has no fields.nodes".into()))?;
    let mut out = BTreeMap::new();
    for n in nodes {
        let (Some(id), Some(name)) = (
            n.get("id").and_then(Value::as_str),
            n.get("name").and_then(Value::as_str),
        ) else {
            continue; // a field type we do not read
        };
        let options = n
            .get("options")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|o| {
                        Some(OptionInfo {
                            id: o.get("id")?.as_str()?.to_string(),
                            name: o.get("name")?.as_str()?.to_string(),
                            color: o.get("color")?.as_str()?.to_string(),
                            description: o
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.insert(
            name.to_string(),
            FieldInfo {
                id: id.to_string(),
                name: name.to_string(),
                data_type: n
                    .get("dataType")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                options,
            },
        );
    }
    Ok(out)
}

impl Board {
    /// The Status field of the board has the option of `status` (a board that `init` made
    /// before the option existed lacks Dropped).
    pub fn has_status_option(&self, status: Status) -> bool {
        self.has_field(Field::Status)
            .is_some_and(|f| f.option_id(status.name()).is_some())
    }

    /// Load the project by owner login and number.
    pub fn load(client: &Client, owner: &str, number: u64) -> CbResult<Board> {
        let kind = owner_info(client, owner)?.kind;
        let root = graphql_root(&kind);
        let q = format!(
            "query($l:String!,$n:Int!){{{root}(login:$l){{projectV2(number:$n){{{PROJECT_FIELDS}}}}}}}"
        );
        let d = client.graphql(&q, json!({"l": owner, "n": number}))?;
        let p = d
            .pointer(&format!("/{root}/projectV2"))
            .filter(|p| p.is_object())
            .ok_or_else(|| {
                CbError::config(format!(
                    "project {number} of {owner} not found or not visible"
                ))
            })?;
        Board::from_json(p, owner, kind)
    }

    pub fn from_json(p: &Value, owner: &str, kind: OwnerKind) -> CbResult<Board> {
        let s = |k: &str| {
            p.get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| GhError::Shape(format!("project `{k}` missing")))
        };
        Ok(Board {
            project_id: s("id")?,
            number: p
                .get("number")
                .and_then(Value::as_u64)
                .ok_or_else(|| GhError::Shape("project `number` missing".into()))?,
            title: s("title")?,
            url: s("url")?,
            public: p.get("public").and_then(Value::as_bool).unwrap_or(false),
            owner: owner.to_string(),
            owner_kind: kind,
            fields: parse_fields(p)?,
        })
    }

    /// Re-read the fields (after a schema write).
    pub fn reload(&mut self, client: &Client) -> CbResult<()> {
        let fresh = Board::load(client, &self.owner, self.number)?;
        self.fields = fresh.fields;
        Ok(())
    }

    /// The field, or a config error that names `init` when the board lacks it.
    pub fn field(&self, field: impl FieldName) -> CbResult<&FieldInfo> {
        self.field_by_name(field.board_name())
    }

    /// [`Board::field`] by the name on the board (inside the option writer, where the name is
    /// already resolved).
    fn field_by_name(&self, name: &str) -> CbResult<&FieldInfo> {
        self.fields.get(name).ok_or_else(|| {
            CbError::config(format!(
                "the board has no field '{name}'; run `crewboard init --project {}` first",
                self.number
            ))
        })
    }

    /// The field, or `None` when the board lacks it.
    pub fn has_field(&self, field: impl FieldName) -> Option<&FieldInfo> {
        self.fields.get(field.board_name())
    }

    /// Replace the cached options of a field (after a fresh read).
    pub fn cache_options(&mut self, field: impl FieldName, options: Vec<OptionInfo>) {
        if let Some(f) = self.fields.get_mut(field.board_name()) {
            f.options = options;
        }
    }

    /// Read items by id. The answer has one entry per id, `None` for an unknown id.
    pub fn read_items(
        &self,
        client: &Client,
        ids: &[String],
    ) -> CbResult<Vec<Option<ItemSnapshot>>> {
        let q = format!(
            "query($ids:[ID!]!){{nodes(ids:$ids){{... on ProjectV2Item{{{ITEM_FRAGMENT}}}}}}}"
        );
        let mut out = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(100) {
            let d = client.graphql(&q, json!({"ids": chunk}))?;
            let nodes = d
                .get("nodes")
                .and_then(Value::as_array)
                .ok_or_else(|| GhError::Shape("nodes(ids:) did not return a list".into()))?;
            if nodes.len() != chunk.len() {
                return Err(GhError::Shape(format!(
                    "nodes(ids:) returned {} entries for {} ids",
                    nodes.len(),
                    chunk.len()
                ))
                .into());
            }
            for n in nodes {
                // An id of another object type returns `{}`: treat it as unknown.
                out.push(if n.as_object().is_some_and(|o| o.is_empty()) {
                    None
                } else {
                    parse_item_node(n)?
                });
            }
        }
        Ok(out)
    }

    /// Read one item by id; it must exist and belong to this project.
    pub fn read_item(&self, client: &Client, id: &str) -> CbResult<ItemSnapshot> {
        let it = self
            .read_items(client, &[id.to_string()])?
            .into_iter()
            .next()
            .flatten()
            .ok_or_else(|| CbError::config(format!("item {id} not found on the board")))?;
        if it.project_number.is_some_and(|n| n != self.number) {
            return Err(CbError::config(format!(
                "item {id} belongs to another project"
            )));
        }
        Ok(it)
    }

    /// Page through the project items with `query` (cursor in `$after`, 100 per page) and hand
    /// the `nodes` of each page to `each`. `extra` holds the other variables of the query.
    fn each_item_page(
        &self,
        client: &Client,
        query: &str,
        extra: Value,
        mut each: impl FnMut(&[Value]),
    ) -> CbResult<()> {
        let mut after = Value::Null;
        for _ in 0..100 {
            let mut vars = json!({"p": self.project_id, "after": after});
            if let (Some(v), Some(x)) = (vars.as_object_mut(), extra.as_object()) {
                v.extend(x.iter().map(|(k, val)| (k.clone(), val.clone())));
            }
            let d = client.graphql(query, vars)?;
            let items = d
                .pointer("/node/items")
                .ok_or_else(|| GhError::Shape("project has no items".into()))?;
            each(
                items
                    .get("nodes")
                    .and_then(Value::as_array)
                    .ok_or_else(|| GhError::Shape("items has no nodes".into()))?,
            );
            if items
                .pointer("/pageInfo/hasNextPage")
                .and_then(Value::as_bool)
                != Some(true)
            {
                return Ok(());
            }
            after = items
                .pointer("/pageInfo/endCursor")
                .cloned()
                .unwrap_or(Value::Null);
        }
        Err(GhError::Shape("item listing has more than 10,000 items".into()).into())
    }

    /// All item ids that the (eventually consistent) listing returns. Pages through all.
    pub fn list_item_ids(&self, client: &Client) -> CbResult<Vec<String>> {
        let q = "query($p:ID!,$after:String){node(id:$p){... on ProjectV2{items(first:100,after:$after){pageInfo{hasNextPage endCursor} nodes{id}}}}}";
        let mut ids = Vec::new();
        self.each_item_page(client, q, json!({}), |nodes| {
            ids.extend(
                nodes
                    .iter()
                    .filter_map(|n| n.get("id").and_then(Value::as_str))
                    .map(str::to_string),
            );
        })?;
        Ok(ids)
    }

    /// The value of one single-select field on every listed item. The option is `None` for an
    /// item without a value. One GraphQL read per 100
    /// items. The listing lags for items added in the last minutes, so a caller
    /// that must know every user of an option adds the items it knows by id.
    pub fn list_select_values(
        &self,
        client: &Client,
        field: impl FieldName,
    ) -> CbResult<Vec<ListedValue>> {
        let q = "query($p:ID!,$f:String!,$after:String){node(id:$p){... on ProjectV2{items(first:100,after:$after){pageInfo{hasNextPage endCursor} nodes{id v:fieldValueByName(name:$f){... on ProjectV2ItemFieldSingleSelectValue{name optionId}}}}}}}";
        let mut out = Vec::new();
        self.each_item_page(client, q, json!({"f": field.board_name()}), |nodes| {
            for n in nodes {
                let Some(id) = n.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let s = |k: &str| {
                    n.pointer(&format!("/v/{k}"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                };
                out.push(ListedValue {
                    item_id: id.to_string(),
                    option_id: s("optionId"),
                    name: s("name"),
                });
            }
        })?;
        Ok(out)
    }

    /// The draft issue behind an agent item: its id (`DI_...`, not the item id) and body.
    /// `None` if the item is not a draft issue.
    pub fn read_draft(&self, client: &Client, item_id: &str) -> CbResult<Option<Draft>> {
        let d = client.graphql(
            "query($i:ID!){node(id:$i){... on ProjectV2Item{content{__typename ... on DraftIssue{id body}}}}}",
            json!({"i": item_id}),
        )?;
        let c = d
            .pointer("/node/content")
            .ok_or_else(|| GhError::Shape(format!("item {item_id} has no content")))?;
        if c.get("__typename").and_then(Value::as_str) != Some("DraftIssue") {
            return Ok(None);
        }
        let id = c
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| GhError::Shape("draft issue has no id".into()))?;
        Ok(Some(Draft {
            id: id.to_string(),
            body: c
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        }))
    }

    /// Write the body of a draft issue (the agent item, spec/profiles#storage). Same size rule as a
    /// message body.
    pub fn set_draft_body(&self, client: &Client, draft_id: &str, body: &str) -> CbResult<()> {
        crate::rules::limits::check_body(body)?;
        let d = client.graphql(
            "mutation($d:ID!,$b:String!){updateProjectV2DraftIssue(input:{draftIssueId:$d,body:$b}){draftIssue{id}}}",
            json!({"d": draft_id, "b": body}),
        )?;
        d.pointer("/updateProjectV2DraftIssue/draftIssue/id")
            .and_then(Value::as_str)
            .map(|_| ())
            .ok_or_else(|| {
                GhError::Shape("updateProjectV2DraftIssue returned no draft issue".into()).into()
            })
    }

    /// Create a draft item (an agent item). Returns its id.
    pub fn add_draft(&self, client: &Client, title: &str, body: &str) -> CbResult<String> {
        let d = client.graphql(
            "mutation($p:ID!,$t:String!,$b:String!){addProjectV2DraftIssue(input:{projectId:$p,title:$t,body:$b}){projectItem{id}}}",
            json!({"p": self.project_id, "t": title, "b": body}),
        )?;
        d.pointer("/addProjectV2DraftIssue/projectItem/id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                GhError::Shape("addProjectV2DraftIssue returned no item id".into()).into()
            })
    }

    /// THE write of a field value (spec/board#fields, "One writer per field"). The field-writer
    /// table decides first ([`check_field_write`]): a refused write sends no request. `who` holds
    /// the facts of the write. `Status` and `State` are refused here (`use-transition`): their
    /// writers are [`Board::write_status`] and [`Board::write_state`].
    pub fn write(
        &self,
        client: &Client,
        at: ItemRef<'_>,
        field: Field,
        value: FieldValue<'_>,
        who: &WriteCtx,
    ) -> CbResult<()> {
        check_field_write(at.item, field, who)?;
        self.put(client, at.id, field, value)
    }

    /// The `Status` of a ticket. The Status flow (spec/tickets#status-flow-and-writers) is checked
    /// by the caller with the transition rules of [`crate::rules::status`].
    pub fn write_status(&self, client: &Client, item_id: &str, to: Status) -> CbResult<()> {
        self.put(
            client,
            item_id,
            Field::Status,
            FieldValue::Option(to.name()),
        )
    }

    /// The `State` of an agent item. The lifecycle (spec/board#lifecycle) is checked by the
    /// caller with the transition rules of [`crate::rules::lifecycle`].
    pub fn write_state(&self, client: &Client, item_id: &str, to: State) -> CbResult<()> {
        self.put(client, item_id, Field::State, FieldValue::Option(to.name()))
    }

    /// The request behind every field write. The value type must be the field type
    /// ([`value_fits`]): a wrong type is an error before any request, in every build.
    fn put(
        &self,
        client: &Client,
        item_id: &str,
        field: Field,
        value: FieldValue<'_>,
    ) -> CbResult<()> {
        value_fits(field, value)?;
        match value {
            FieldValue::Text(t) => self.set_text(client, item_id, field, t),
            FieldValue::Option(o) => self.set_select(client, item_id, field, o),
            FieldValue::Number(n) => self.set_number(client, item_id, field, n),
            FieldValue::Clear => self.clear_field(client, item_id, field),
        }
    }

    /// The text write. Refuses a value over 1024 bytes before any request.
    fn set_text(&self, client: &Client, item_id: &str, field: Field, value: &str) -> CbResult<()> {
        check_text_field(value)?;
        let f = self.field(field)?;
        client.graphql(
            "mutation($p:ID!,$i:ID!,$f:ID!,$t:String!){updateProjectV2ItemFieldValue(input:{projectId:$p,itemId:$i,fieldId:$f,value:{text:$t}}){projectV2Item{id}}}",
            json!({"p": self.project_id, "i": item_id, "f": f.id, "t": value}),
        )?;
        Ok(())
    }

    /// Add an issue to the project. Idempotent: an issue that is on the board returns its
    /// existing item id. Returns the item id.
    pub fn add_issue_item(&self, client: &Client, issue_node_id: &str) -> CbResult<String> {
        let d = client.graphql(
            "mutation($p:ID!,$c:ID!){addProjectV2ItemById(input:{projectId:$p,contentId:$c}){item{id}}}",
            json!({"p": self.project_id, "c": issue_node_id}),
        )?;
        d.pointer("/addProjectV2ItemById/item/id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| GhError::Shape("addProjectV2ItemById returned no item id".into()).into())
    }

    /// Clear a field value.
    fn clear_field(&self, client: &Client, item_id: &str, field: Field) -> CbResult<()> {
        let f = self.field(field)?;
        client.graphql(
            "mutation($p:ID!,$i:ID!,$f:ID!){clearProjectV2ItemFieldValue(input:{projectId:$p,itemId:$i,fieldId:$f}){projectV2Item{id}}}",
            json!({"p": self.project_id, "i": item_id, "f": f.id}),
        )?;
        Ok(())
    }

    /// Set a single-select value by option name. The option must exist.
    fn set_select(
        &self,
        client: &Client,
        item_id: &str,
        field: Field,
        option: &str,
    ) -> CbResult<()> {
        let f = self.field(field)?;
        let oid = f.option_id(option).ok_or_else(|| {
            CbError::refused(
                ids::OPTION_MISSING,
                format!("field {field} has no option '{option}'"),
            )
        })?;
        client.graphql(
            "mutation($p:ID!,$i:ID!,$f:ID!,$o:String!){updateProjectV2ItemFieldValue(input:{projectId:$p,itemId:$i,fieldId:$f,value:{singleSelectOptionId:$o}}){projectV2Item{id}}}",
            json!({"p": self.project_id, "i": item_id, "f": f.id, "o": oid}),
        )?;
        Ok(())
    }

    /// Set a number field. A number field has no options: nothing to keep or resend.
    fn set_number(&self, client: &Client, item_id: &str, field: Field, value: i64) -> CbResult<()> {
        let f = self.field(field)?;
        if f.data_type != "NUMBER" {
            return Err(CbError::refused(
                ids::FIELD_TYPE_MISMATCH,
                format!(
                    "field {field} exists as {}, it must be a number",
                    f.data_type
                ),
            ));
        }
        client.graphql(
            "mutation($p:ID!,$i:ID!,$f:ID!,$n:Float!){updateProjectV2ItemFieldValue(input:{projectId:$p,itemId:$i,fieldId:$f,value:{number:$n}}){projectV2Item{id}}}",
            json!({"p": self.project_id, "i": item_id, "f": f.id, "n": value as f64}),
        )?;
        Ok(())
    }

    /// Make sure a number field exists. A missing field is created; a field of another type is
    /// refused (`field-type-mismatch`). Returns true if the field was created. Used for the
    /// optional `Order` field.
    pub fn ensure_number_field(&mut self, client: &Client, field: Field) -> CbResult<bool> {
        let name = field.name();
        match self.fields.get(name) {
            None => {
                client.graphql(
                    "mutation($p:ID!,$n:String!){createProjectV2Field(input:{projectId:$p,dataType:NUMBER,name:$n}){projectV2Field{... on ProjectV2Field{id}}}}",
                    json!({"p": self.project_id, "n": name}),
                )?;
                self.reload(client)?;
                Ok(true)
            }
            Some(f) if f.data_type != "NUMBER" => Err(CbError::refused(
                ids::FIELD_TYPE_MISMATCH,
                format!(
                    "field {name} exists as {}, it must be a number",
                    f.data_type
                ),
            )),
            Some(_) => Ok(false),
        }
    }

    /// Read the options of one single-select field from GitHub (not from the cache).
    pub fn read_options(
        &self,
        client: &Client,
        field: impl FieldName,
    ) -> CbResult<Vec<OptionInfo>> {
        self.read_options_by_name(client, field.board_name())
    }

    /// [`Board::read_options`] by the name on the board (inside the option writer).
    fn read_options_by_name(&self, client: &Client, name: &str) -> CbResult<Vec<OptionInfo>> {
        let fresh = Board::load(client, &self.owner, self.number)?;
        Ok(fresh.field_by_name(name)?.options.clone())
    }

    /// THE writer of single-select options (spec/board#fields). `desired` must come from
    /// `merge_options` / `plan_options`, which keep every id. After the write it reads the
    /// field back and refuses (exit 2, rule `option-id-lost`) if an earlier id is gone.
    pub fn write_options(
        &mut self,
        client: &Client,
        field: impl FieldName,
        desired: &[DesiredOption],
    ) -> CbResult<Vec<OptionInfo>> {
        self.write_options_inner(client, field.board_name(), desired, &[], &[])
    }

    /// The same writer for `init`, which turns a pristine built-in name into ours: `renames`
    /// (`from`, `to`, the pairs given to `plan_options`) are the only name changes the
    /// read-back accepts.
    pub fn write_options_renaming(
        &mut self,
        client: &Client,
        field: impl FieldName,
        desired: &[DesiredOption],
        renames: &[(&str, &str)],
    ) -> CbResult<Vec<OptionInfo>> {
        self.write_options_inner(client, field.board_name(), desired, &[], renames)
    }

    /// The same writer for `prune`: the ids in `removing` are meant to be gone. Every other
    /// earlier id must be in `desired` before the write and must still be there after it.
    pub fn write_options_removing(
        &mut self,
        client: &Client,
        field: impl FieldName,
        desired: &[DesiredOption],
        removing: &[String],
    ) -> CbResult<Vec<OptionInfo>> {
        self.write_options_inner(client, field.board_name(), desired, removing, &[])
    }

    /// The one write behind the three public writers. A kept id keeps its name unless
    /// `renames` plans the change.
    fn write_options_inner(
        &mut self,
        client: &Client,
        field: &str,
        desired: &[DesiredOption],
        removing: &[String],
        renames: &[(&str, &str)],
    ) -> CbResult<Vec<OptionInfo>> {
        // The write replaces the whole list, so an option that a concurrent writer added after
        // the caller planned would be deleted. Read the list right before the write, carry what
        // is new, and check the read-back against that list. One retry if the write was still
        // overtaken; then a refusal (`option-lost-concurrently`). The window that is left is the
        // round trip between the read and the write (spec/board#fields).
        let mut desired = desired.to_vec();
        for attempt in 0..2 {
            let fresh = Board::load(client, &self.owner, self.number)?;
            let f = fresh.field_by_name(field)?.clone();
            let before = f.entries();
            desired = rebase_on_fresh(&desired, &before, removing);
            let expected: Vec<OptionEntry> = before
                .iter()
                .filter(|o| !removing.contains(&o.id))
                .cloned()
                .collect();
            // A caller that built `desired` without the current ids is refused before the write.
            verify_ids_kept(
                &expected,
                &desired
                    .iter()
                    .filter_map(|d| {
                        d.id.as_ref().map(|id| OptionEntry {
                            id: id.clone(),
                            name: d.name.clone(),
                        })
                    })
                    .collect::<Vec<_>>(),
            )?;
            // GitHub refuses more than 100 options with a raw GraphQL error: refuse first, by name.
            check_option_limit(field, desired.len())?;
            let inputs = options_payload(&f, &desired);
            client.graphql(
                "mutation($f:ID!,$o:[ProjectV2SingleSelectFieldOptionInput!]!){updateProjectV2Field(input:{fieldId:$f,singleSelectOptions:$o}){projectV2Field{... on ProjectV2SingleSelectField{id}}}}",
                json!({"f": f.id, "o": inputs}),
            )?;
            // Read back from GitHub and check that every earlier id survived.
            let after = self.read_options_by_name(client, field)?;
            let after_entries: Vec<OptionEntry> = after
                .iter()
                .map(|o| OptionEntry {
                    id: o.id.clone(),
                    name: o.name.clone(),
                })
                .collect();
            verify_ids_kept_except(&before, &after_entries, removing, renames)?;
            let lost = names_missing_after(&desired, &after_entries);
            if !lost.is_empty() {
                if attempt == 0 {
                    continue;
                }
                return Err(CbError::refused(
                    ids::OPTION_LOST_CONCURRENTLY,
                    format!(
                        "another writer changed the options of {field} at the same time and the options {} are not there after the write; run the command again",
                        lost.join(", ")
                    ),
                ));
            }
            if let Some(fi) = self.fields.get_mut(field) {
                fi.options = after.clone();
            }
            return Ok(after);
        }
        Err(CbError::config("option write: no attempt ran"))
    }
}

/// A value of the type of the field (`Clear` fits every field). The other case is a bug of the
/// caller: an internal error, and the request is not sent, so a text mutation never reaches a
/// single-select field.
fn value_fits(field: Field, value: FieldValue<'_>) -> CbResult<()> {
    let fits = matches!(
        (field.kind(), value),
        (FieldType::Text, FieldValue::Text(_))
            | (FieldType::SingleSelect, FieldValue::Option(_))
            | (FieldType::Number, FieldValue::Number(_))
            | (_, FieldValue::Clear)
    );
    if fits {
        Ok(())
    } else {
        Err(CbError::config(format!(
            "internal error: the field {field} ({:?}) gets a value of another type; nothing was sent",
            field.kind()
        )))
    }
}

/// The option list of an `updateProjectV2Field` call. GitHub replaces the whole list, and an
/// option sent without its id gets a new id, which empties that value on every item. So
/// every option that has an id is sent WITH its id, its color and its description.
pub fn options_payload(f: &FieldInfo, desired: &[DesiredOption]) -> Vec<Value> {
    desired
        .iter()
        .map(|d| {
            let known = d
                .id
                .as_ref()
                .and_then(|id| f.options.iter().find(|o| &o.id == id));
            let mut o = json!({
                "name": d.name,
                "color": known.map_or_else(|| default_color(&d.name).to_string(), |k| k.color.clone()),
                "description": known.map(|k| k.description.clone()).unwrap_or_default(),
            });
            if let Some(id) = &d.id {
                o["id"] = json!(id);
            }
            o
        })
        .collect()
}

/// Colors for the options we create. Others are gray.
pub fn default_color(name: &str) -> &'static str {
    match name {
        "Work" | "Ready" | "requested" => "BLUE",
        "Agent" | "Done" | "active" => "GREEN",
        "Backlog" | "left" => "GRAY",
        "In Progress" | "approved" | "paused" => "YELLOW",
        "Review" | "Dev" => "PURPLE",
        "Blocked" | "draining" => "RED",
        "Coordinator" => "ORANGE",
        _ => "GRAY",
    }
}

/// Split `owner/repo` and build REST paths used by several commands.
pub fn repo_path(repo: &str) -> CbResult<String> {
    let (o, r) = split_repo(repo)?;
    Ok(format!("repos/{o}/{r}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_paths_and_graphql_roots_differ_for_org_and_user() {
        assert_eq!(
            rest_base(&OwnerKind::Org, "example-org"),
            "orgs/example-org"
        );
        // the user form takes the login, not the numeric id (see rest_base)
        assert_eq!(
            rest_base(&OwnerKind::User, "owner-login"),
            "users/owner-login"
        );
        assert_eq!(graphql_root(&OwnerKind::Org), "organization");
        assert_eq!(graphql_root(&OwnerKind::User), "user");
    }

    fn project_json() -> Value {
        json!({
            "id": "PVT_x", "number": 15, "title": "t", "url": "https://x", "public": false,
            "fields": {"nodes": [
                {"__typename": "ProjectV2Field", "id": "F1", "name": "Gate", "dataType": "TEXT"},
                {"__typename": "ProjectV2SingleSelectField", "id": "F2", "name": "Kind", "dataType": "SINGLE_SELECT",
                 "options": [{"id": "o1", "name": "Work", "color": "BLUE", "description": ""},
                             {"id": "o2", "name": "Agent", "color": "GREEN", "description": "d"}]},
                {}
            ]}
        })
    }

    #[test]
    fn a_value_of_another_type_is_an_internal_error_in_every_build() {
        assert!(value_fits(Field::Gate, FieldValue::Text("x")).is_ok());
        assert!(value_fits(Field::Kind, FieldValue::Option("o1")).is_ok());
        assert!(value_fits(Field::Order, FieldValue::Number(1)).is_ok());
        for f in Field::ALL {
            assert!(value_fits(f, FieldValue::Clear).is_ok(), "{f}");
        }
        let e = value_fits(Field::Kind, FieldValue::Text("Work")).unwrap_err();
        assert!(
            e.to_string().contains(
                "internal error: the field Kind (SingleSelect) gets a value of another type"
            ),
            "{e}"
        );
        assert!(value_fits(Field::Gate, FieldValue::Option("o1")).is_err());
        assert!(value_fits(Field::Order, FieldValue::Text("1")).is_err());
    }

    #[test]
    fn parses_fields_and_options() {
        let b = Board::from_json(&project_json(), "example-org", OwnerKind::Org).unwrap();
        assert_eq!(b.number, 15);
        assert_eq!(b.fields.len(), 2);
        assert_eq!(b.field(Field::Kind).unwrap().option_id("Agent"), Some("o2"));
        assert_eq!(b.field(Field::Kind).unwrap().option_id("Nope"), None);
        assert_eq!(b.field(Field::Gate).unwrap().data_type, "TEXT");
    }

    #[test]
    fn a_missing_field_is_a_config_error_that_names_init() {
        let b = Board::from_json(&project_json(), "example-org", OwnerKind::Org).unwrap();
        let e = b.field(Field::Cursor).unwrap_err();
        assert_eq!(e.exit(), crate::exit::Exit::Config);
        assert!(e.to_string().contains("init"));
        assert!(Board::from_json(&json!({"id": "x"}), "o", OwnerKind::Org).is_err());
    }

    #[test]
    fn colors_are_known_for_our_names() {
        assert_eq!(default_color("Blocked"), "RED");
        assert_eq!(default_color("claude@host-a"), "GRAY");
    }

    #[test]
    fn the_options_payload_keeps_every_existing_id_color_and_description() {
        let b = Board::from_json(&project_json(), "example-org", OwnerKind::Org).unwrap();
        let f = b.field(Field::Kind).unwrap();
        // an add: the complete list, the two old options with their ids, the new one without
        let desired = crate::rules::options::merge_options(
            &f.entries(),
            &crate::rules::options::OptionChange::Add { name: "P9".into() },
        )
        .unwrap();
        let p = options_payload(f, &desired);
        assert_eq!(p.len(), 3);
        assert_eq!(
            (p[0]["id"].as_str(), p[0]["name"].as_str()),
            (Some("o1"), Some("Work"))
        );
        assert_eq!(
            (p[1]["id"].as_str(), p[1]["color"].as_str()),
            (Some("o2"), Some("GREEN"))
        );
        assert_eq!(p[1]["description"], "d");
        assert!(p[2].get("id").is_none(), "a new option has no id: {}", p[2]);
        // control: a list built without the ids sends no ids (this is what wipes values)
        let bad = vec![DesiredOption {
            id: None,
            name: "Work".into(),
        }];
        assert!(options_payload(f, &bad)[0].get("id").is_none());
    }

    #[test]
    fn repo_path_checks_the_shape() {
        assert_eq!(repo_path("a/b").unwrap(), "repos/a/b");
        assert!(repo_path("ab").is_err());
    }
}
