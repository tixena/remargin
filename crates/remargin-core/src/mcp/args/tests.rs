extern crate alloc;

use alloc::collections::BTreeSet;

use serde_json::{Value, json};

use super::*;

fn schemas() -> Vec<(&'static str, Value)> {
    vec![
        ("ack", AckArgs::json_schema()),
        ("activity", ActivityArgs::json_schema()),
        ("batch", BatchArgs::json_schema()),
        ("comment", CommentArgs::json_schema()),
        ("comments", CommentsArgs::json_schema()),
        ("cp", CpArgs::json_schema()),
        ("delete", DeleteArgs::json_schema()),
        ("doctor", DoctorArgs::json_schema()),
        ("edit", EditArgs::json_schema()),
        ("get", GetArgs::json_schema()),
        ("get_image", GetImageArgs::json_schema()),
        ("identity_create", IdentityCreateArgs::json_schema()),
        ("lint", LintArgs::json_schema()),
        ("ls", LsArgs::json_schema()),
        ("metadata", MetadataArgs::json_schema()),
        ("mv", MvArgs::json_schema()),
        ("permissions_check", PermissionsCheckArgs::json_schema()),
        ("permissions_show", PermissionsShowArgs::json_schema()),
        ("plan", PlanArgs::json_schema()),
        ("prompt_delete", PromptDeleteArgs::json_schema()),
        ("prompt_list", PromptListArgs::json_schema()),
        ("prompt_resolve", PromptResolveArgs::json_schema()),
        ("prompt_set", PromptSetArgs::json_schema()),
        ("purge", PurgeArgs::json_schema()),
        ("query", QueryArgs::json_schema()),
        ("react", ReactArgs::json_schema()),
        ("replace", ReplaceArgs::json_schema()),
        ("reply", ReplyArgs::json_schema()),
        ("report_spill", ReportSpillArgs::json_schema()),
        ("rm", RmArgs::json_schema()),
        ("sandbox_add", SandboxAddArgs::json_schema()),
        ("sandbox_list", SandboxListArgs::json_schema()),
        ("sandbox_remove", SandboxRemoveArgs::json_schema()),
        ("search", SearchArgs::json_schema()),
        ("sign", SignArgs::json_schema()),
        ("verify", VerifyArgs::json_schema()),
        ("whoami", WhoamiArgs::json_schema()),
        ("write", WriteArgs::json_schema()),
    ]
}

fn resolve<'schema>(schema: &'schema Value, root: &'schema Value) -> &'schema Value {
    if let Some(target) = schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix("#/$defs/"))
        .and_then(|name| root.get("$defs")?.get(name))
    {
        return resolve(target, root);
    }
    if let Some(Value::Array(branches)) = schema.get("anyOf")
        && let [inner, null] = branches.as_slice()
        && null.get("type").and_then(Value::as_str) == Some("null")
    {
        return resolve(inner, root);
    }
    schema
}

fn sorted_strings(value: Option<&Value>) -> Vec<String> {
    let mut strings: Vec<String> = value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    strings.sort();
    strings
}

fn compare(
    path: &str,
    today: &Value,
    today_root: &Value,
    args: &Value,
    args_root: &Value,
    faults: &mut Vec<String>,
) {
    let today_schema = resolve(today, today_root);
    let args_schema = resolve(args, args_root);
    // doc_markdown forces backticks into rustdoc that today's text sometimes lacks.
    let args_description = args
        .get("description")
        .or_else(|| args_schema.get("description"))
        .and_then(Value::as_str)
        .map(|text| text.replace('`', ""));
    let today_description = today_schema
        .get("description")
        .and_then(Value::as_str)
        .map(|text| text.replace('`', ""));
    if today_description != args_description {
        faults.push(format!(
            "{path}: `description` is {today_description:?} today, {args_description:?} in the struct"
        ));
    }

    for key in ["type", "minimum"] {
        if today_schema.get(key) != args_schema.get(key) {
            faults.push(format!(
                "{path}: `{key}` is {:?} today, {:?} in the struct",
                today_schema.get(key),
                args_schema.get(key)
            ));
        }
    }
    if sorted_strings(today_schema.get("enum")) != sorted_strings(args_schema.get("enum")) {
        faults.push(format!(
            "{path}: `enum` differs: {:?} today, {:?} in the struct",
            today_schema.get("enum"),
            args_schema.get("enum")
        ));
    }

    let today_default = today_schema.get("default");
    let args_default = args_schema.get("default");
    let must_default = args.get("anyOf").is_none()
        && (matches!(today_default, Some(Value::Bool(false))) || today_default == Some(&json!([])));
    if (args_default.is_some() || must_default) && today_default != args_default {
        faults.push(format!(
            "{path}: `default` is {today_default:?} today, {args_default:?} in the struct"
        ));
    }

    match (today_schema.get("items"), args_schema.get("items")) {
        (Some(today_items), Some(args_items)) => {
            compare(
                &format!("{path}[]"),
                today_items,
                today_root,
                args_items,
                args_root,
                faults,
            );
        }
        (None, None) => {}
        (today_items, args_items) => {
            faults.push(format!(
                "{path}: `items` is {today_items:?} today, {args_items:?} in the struct"
            ));
        }
    }

    let today_properties = today_schema.get("properties").and_then(Value::as_object);
    let args_properties = args_schema.get("properties").and_then(Value::as_object);
    let today_names: BTreeSet<&String> = today_properties
        .map(|map| map.keys().collect())
        .unwrap_or_default();
    let args_names: BTreeSet<&String> = args_properties
        .map(|map| map.keys().collect())
        .unwrap_or_default();
    if today_names != args_names {
        faults.push(format!(
            "{path}: properties are {today_names:?} today, {args_names:?} in the struct"
        ));
    }
    if sorted_strings(today_schema.get("required")) != sorted_strings(args_schema.get("required")) {
        faults.push(format!(
            "{path}: `required` is {:?} today, {:?} in the struct",
            today_schema.get("required"),
            args_schema.get("required")
        ));
    }
    if let (Some(today_map), Some(args_map)) = (today_properties, args_properties) {
        for (name, today_property) in today_map {
            if let Some(args_property) = args_map.get(name) {
                compare(
                    &format!("{path}.{name}"),
                    today_property,
                    today_root,
                    args_property,
                    args_root,
                    faults,
                );
            }
        }
    }
}

fn parity_faults(today: &[(String, Value)], args: &[(&str, Value)]) -> Vec<String> {
    let mut faults = Vec::new();
    for (name, today_schema) in today {
        match args.iter().find(|(args_name, _)| args_name == name) {
            Some((_, args_schema)) => compare(
                name,
                today_schema,
                today_schema,
                args_schema,
                args_schema,
                &mut faults,
            ),
            None => faults.push(format!("{name}: no argument struct")),
        }
    }
    for (name, _) in args {
        if !today.iter().any(|(today_name, _)| today_name == name) {
            faults.push(format!("{name}: no tool"));
        }
    }
    faults
}

fn today_schemas() -> Vec<(String, Value)> {
    let tools: Vec<Value> = serde_json::from_str(include_str!("today_schemas.json")).unwrap();
    tools
        .into_iter()
        .map(|mut tool| {
            let name = tool["name"].as_str().unwrap().to_owned();
            (name, tool["inputSchema"].take())
        })
        .collect()
}

#[test]
fn every_tool_has_an_argument_struct_held_to_its_schema() {
    let today = today_schemas();
    assert_eq!(today.len(), 38);
    let faults = parity_faults(&today, &schemas());
    assert!(faults.is_empty(), "{}", faults.join("\n"));
}

#[test]
fn parity_fails_when_a_property_is_on_one_side_only() {
    let mut today = today_schemas();
    for (name, schema) in &mut today {
        if name == "ls"
            && let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut)
        {
            properties.insert("extra".to_owned(), json!({"type": "string"}));
        }
    }
    let faults = parity_faults(&today, &schemas());
    assert_eq!(faults.len(), 1, "{faults:?}");
    assert!(faults[0].starts_with("ls: properties are"), "{faults:?}");
}

#[test]
fn an_unknown_field_is_refused_by_name() {
    let call = json!({"file": "a.md", "content": "x", "identity": "y"});
    let fault = serde_json::from_value::<CommentArgs>(call)
        .unwrap_err()
        .to_string();
    assert!(fault.contains("unknown field `identity`"), "{fault}");
}

#[test]
fn omitted_defaults_read_as_empty_and_false() {
    let call = json!({"file": "a.md", "content": "x"});
    let args = serde_json::from_value::<CommentArgs>(call).unwrap();
    assert_eq!(args.to, Vec::<String>::new());
    assert!(!args.sandbox);
}

#[test]
fn whoami_refuses_any_argument_and_reads_an_empty_object() {
    let fault = serde_json::from_value::<WhoamiArgs>(json!({"a": 1_u8}))
        .unwrap_err()
        .to_string();
    assert!(fault.contains("unknown field `a`"), "{fault}");
    serde_json::from_value::<WhoamiArgs>(json!({})).unwrap();
}

#[test]
fn permissions_show_refuses_any_argument_and_reads_an_empty_object() {
    let fault = serde_json::from_value::<PermissionsShowArgs>(json!({"a": 1_u8}))
        .unwrap_err()
        .to_string();
    assert!(fault.contains("unknown field `a`"), "{fault}");
    serde_json::from_value::<PermissionsShowArgs>(json!({})).unwrap();
}
