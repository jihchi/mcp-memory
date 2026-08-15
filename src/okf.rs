use serde_json::Value as JsonValue;
use serde_yaml_ng::{Mapping, Value};
use std::fs;
use std::path::{Component, Path, PathBuf};
use time::{OffsetDateTime, macros::format_description};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, Default)]
pub struct OkfOptions {
    pub key: String,
    pub content: JsonValue,
    pub tags: Vec<String>,
    pub namespace: String,
    pub concept_type: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub resource: Option<String>,
    pub status: Option<String>,
    pub stale_after: Option<String>,
    pub sources: Option<JsonValue>,
    pub usage_window: Option<JsonValue>,
    pub verified: Option<JsonValue>,
    pub generated_by: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub runtime: Option<String>,
    pub parameters: Option<JsonValue>,
    pub executor: Option<JsonValue>,
    pub attester: Option<JsonValue>,
    pub computation: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ParsedOkf {
    pub key: String,
    pub namespace: String,
    pub tags: Vec<String>,
    pub frontmatter: Mapping,
    pub body: String,
    pub raw_payload: String,
}

fn yaml_key(value: &str) -> Value {
    Value::String(value.to_owned())
}
fn insert(map: &mut Mapping, key: &str, value: Value) {
    map.insert(yaml_key(key), value);
}
fn json_to_yaml(value: JsonValue) -> Result<Value> {
    serde_yaml_ng::to_value(value).map_err(|e| e.to_string())
}

pub fn utc_now_iso() -> String {
    OffsetDateTime::now_utc()
        .format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
        ))
        .expect("static UTC timestamp format")
}

pub fn serialize_okf(options: &OkfOptions) -> Result<String> {
    let now = utc_now_iso();
    let created_at = options.created_at.as_deref().unwrap_or(&now);
    let updated_at = options.updated_at.as_deref().unwrap_or(&now);
    let title = options.title.clone().unwrap_or_else(|| {
        options
            .key
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .map(title_case)
            .unwrap_or_else(|| "Memory Record".into())
    });

    let mut fm = Mapping::new();
    insert(
        &mut fm,
        "type",
        yaml_key(options.concept_type.as_deref().unwrap_or("Agent Memory")),
    );
    insert(&mut fm, "title", yaml_key(&title));
    insert(&mut fm, "key", yaml_key(&options.key));
    insert(&mut fm, "namespace", yaml_key(&options.namespace));
    if let Some(v) = &options.description {
        insert(&mut fm, "description", yaml_key(v));
    }
    if let Some(v) = &options.resource {
        insert(&mut fm, "resource", yaml_key(v));
    }
    insert(
        &mut fm,
        "tags",
        Value::Sequence(options.tags.iter().map(|v| yaml_key(v)).collect()),
    );
    let status = options
        .status
        .as_deref()
        .filter(|v| matches!(*v, "draft" | "stable" | "deprecated"))
        .unwrap_or("stable");
    insert(&mut fm, "status", yaml_key(status));
    if let Some(v) = &options.stale_after {
        insert(&mut fm, "stale_after", yaml_key(v));
    }

    let mut generated = Mapping::new();
    insert(
        &mut generated,
        "by",
        yaml_key(
            options
                .generated_by
                .as_deref()
                .unwrap_or("mcp-memory/0.2.0"),
        ),
    );
    insert(&mut generated, "at", yaml_key(updated_at));
    insert(&mut fm, "generated", Value::Mapping(generated));

    for (key, value) in [
        ("verified", &options.verified),
        ("sources", &options.sources),
        ("usage_window", &options.usage_window),
    ] {
        if let Some(value) = value {
            insert(&mut fm, key, json_to_yaml(value.clone())?);
        }
    }
    if let Some(v) = &options.runtime {
        insert(&mut fm, "runtime", yaml_key(v));
    }
    for (key, value) in [
        ("parameters", &options.parameters),
        ("executor", &options.executor),
        ("attester", &options.attester),
    ] {
        if let Some(value) = value {
            insert(&mut fm, key, json_to_yaml(value.clone())?);
        }
    }
    if let Some(v) = &options.computation {
        insert(&mut fm, "computation", yaml_key(v));
    }
    insert(&mut fm, "created_at", yaml_key(created_at));
    insert(&mut fm, "updated_at", yaml_key(updated_at));

    let body = match &options.content {
        JsonValue::String(value) => value.trim().to_owned(),
        JsonValue::Object(object) => {
            insert(&mut fm, "content_type", yaml_key("json"));
            let mut lines = vec!["# Memory Content\n".to_owned()];
            for (key, value) in object {
                if value.is_array() || value.is_object() {
                    lines.push(format!("- **{key}**: `{}`", python_json(value, None, 0)));
                } else if let Some(value) = value.as_str() {
                    lines.push(format!("- **{key}**: {value}"));
                } else {
                    lines.push(format!("- **{key}**: {}", python_scalar(value)));
                }
            }
            lines.push("\n```json".into());
            lines.push(python_json(&JsonValue::Object(object.clone()), Some(2), 0));
            lines.push("```".into());
            lines.join("\n")
        }
        value => value.to_string().trim().to_owned(),
    };

    let yaml = serde_yaml_ng::to_string(&fm).map_err(|e| e.to_string())?;
    let yaml = quote_yaml_dates(yaml.strip_prefix("---\n").unwrap_or(&yaml));
    Ok(format!("---\n{yaml}---\n\n{body}\n"))
}

fn python_scalar(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "None".into(),
        JsonValue::Bool(true) => "True".into(),
        JsonValue::Bool(false) => "False".into(),
        other => other.to_string(),
    }
}

fn python_json(value: &JsonValue, indent: Option<usize>, level: usize) -> String {
    match value {
        JsonValue::Null => "null".into(),
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Number(value) => value.to_string(),
        JsonValue::String(value) => json_string_ascii(value),
        JsonValue::Array(values) => {
            if values.is_empty() {
                return "[]".into();
            }
            match indent {
                None => format!(
                    "[{}]",
                    values
                        .iter()
                        .map(|v| python_json(v, None, level + 1))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Some(width) => {
                    let pad = " ".repeat(width * (level + 1));
                    let close = " ".repeat(width * level);
                    format!(
                        "[\n{}\n{close}]",
                        values
                            .iter()
                            .map(|v| format!("{pad}{}", python_json(v, indent, level + 1)))
                            .collect::<Vec<_>>()
                            .join(",\n")
                    )
                }
            }
        }
        JsonValue::Object(values) => {
            if values.is_empty() {
                return "{}".into();
            }
            match indent {
                None => format!(
                    "{{{}}}",
                    values
                        .iter()
                        .map(|(k, v)| format!(
                            "{}: {}",
                            json_string_ascii(k),
                            python_json(v, None, level + 1)
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Some(width) => {
                    let pad = " ".repeat(width * (level + 1));
                    let close = " ".repeat(width * level);
                    format!(
                        "{{\n{}\n{close}}}",
                        values
                            .iter()
                            .map(|(k, v)| format!(
                                "{pad}{}: {}",
                                json_string_ascii(k),
                                python_json(v, indent, level + 1)
                            ))
                            .collect::<Vec<_>>()
                            .join(",\n")
                    )
                }
            }
        }
    }
}

fn json_string_ascii(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '\"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            c if c < '\u{20}' || !c.is_ascii() => {
                let code = c as u32;
                if code <= 0xffff {
                    output.push_str(&format!("\\u{code:04x}"));
                } else {
                    let code = code - 0x10000;
                    output.push_str(&format!(
                        "\\u{:04x}\\u{:04x}",
                        0xd800 + (code >> 10),
                        0xdc00 + (code & 0x3ff)
                    ));
                }
            }
            c => output.push(c),
        }
    }
    output.push('\"');
    output
}

fn quote_yaml_dates(yaml: &str) -> String {
    yaml.lines()
        .map(|line| {
            let (prefix, scalar) = if let Some((left, right)) = line.rsplit_once(": ") {
                (format!("{left}: "), right)
            } else if let Some(right) = line.strip_prefix("- ") {
                ("- ".to_owned(), right)
            } else {
                return line.to_owned();
            };
            if looks_like_iso_date(scalar) {
                format!("{prefix}'{}'", scalar.replace('\'', "''"))
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn looks_like_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    let date = bytes.len() >= 10
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit);
    date && (bytes.len() == 10 || bytes.get(10) == Some(&b'T'))
}

fn title_case(value: &str) -> String {
    let mut output = String::new();
    let mut previous_cased = false;
    for character in value.replace('_', " ").chars() {
        if character.is_alphabetic() {
            if previous_cased {
                output.extend(character.to_lowercase());
            } else {
                output.extend(character.to_uppercase());
            }
            previous_cased = true;
        } else {
            output.push(character);
            previous_cased = false;
        }
    }
    output
}

pub fn parse_okf(raw: &str) -> Result<ParsedOkf> {
    if raw.is_empty() {
        return Err("Empty OKF payload.".into());
    }
    let lines: Vec<&str> = raw.lines().collect();
    if lines.first().map(|v| v.trim()) != Some("---") {
        return Err(
            "Invalid OKF document: missing starting YAML frontmatter delimiter '---'".into(),
        );
    }
    let end = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.trim() == "---")
        .map(|(i, _)| i)
        .ok_or_else(|| {
            "Invalid OKF document: missing closing YAML frontmatter delimiter '---'".to_owned()
        })?;
    let value: Value = serde_yaml_ng::from_str(&lines[1..end].join("\n"))
        .map_err(|e| format!("Failed to parse YAML frontmatter: {e}"))?;
    let fm = value.as_mapping().cloned().unwrap_or_default();
    let string = |key: &str| {
        fm.get(yaml_key(key))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let tags = fm
        .get(yaml_key("tags"))
        .and_then(Value::as_sequence)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Ok(ParsedOkf {
        key: string("key"),
        namespace: fm
            .get(yaml_key("namespace"))
            .and_then(Value::as_str)
            .unwrap_or("default")
            .to_owned(),
        tags,
        frontmatter: fm,
        body: lines[end + 1..].join("\n").trim().to_owned(),
        raw_payload: raw.to_owned(),
    })
}

pub fn validate_okf_conformance(raw: &str) -> (bool, Vec<String>) {
    let parsed = match parse_okf(raw) {
        Ok(v) => v,
        Err(e) => return (false, vec![e]),
    };
    let mut errors = Vec::new();
    let get = |key: &str| parsed.frontmatter.get(yaml_key(key));
    if get("type")
        .and_then(Value::as_str)
        .is_none_or(|v| v.trim().is_empty())
    {
        errors.push("Required field 'type' is missing or empty (§11)".into());
    }
    if let Some(status) = get("status").and_then(Value::as_str)
        && !matches!(status, "draft" | "stable" | "deprecated")
    {
        errors.push(format!(
            "Invalid status '{status}'. Expected 'draft', 'stable', or 'deprecated' (§5.4)"
        ));
    }
    if let Some(actor) = get("generated")
        .and_then(Value::as_mapping)
        .and_then(|m| m.get(yaml_key("by")))
        .and_then(Value::as_str)
        && !actor.is_empty()
        && !actor.starts_with("human:")
        && !actor.starts_with("process:")
        && !actor.contains('/')
    {
        errors.push(format!("Actor '{actor}' in generated.by does not follow actor convention (<producer>/<version>, human:<id>, process:<id>) (§7)"));
    }
    (errors.is_empty(), errors)
}

fn safe_relative(value: &str, add_md: bool) -> Result<PathBuf> {
    let trimmed = value.trim_matches('/');
    if trimmed.is_empty() {
        return Err("memory key must not be empty".into());
    }
    let path = Path::new(trimmed);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(
            "memory key and namespace must not contain '.', '..', or absolute path components"
                .into(),
        );
    }
    let mut result = path.to_path_buf();
    if add_md && !trimmed.ends_with(".md") {
        let mut name = result.file_name().unwrap_or_default().to_os_string();
        name.push(".md");
        result.set_file_name(name);
    }
    if result
        .file_name()
        .and_then(|v| v.to_str())
        .is_some_and(|v| matches!(v, "index.md" | "log.md"))
    {
        return Err("'index.md' and 'log.md' are reserved OKF filenames".into());
    }
    Ok(result)
}

pub fn memory_file_path(key: &str, namespace: &str, base: &Path) -> Result<PathBuf> {
    let key = safe_relative(key, true)?;
    if namespace.is_empty() || namespace == "default" {
        return Ok(base.join(key));
    }
    Ok(base.join(safe_relative(namespace, false)?).join(key))
}

pub fn sync_memory_to_disk(
    key: &str,
    namespace: &str,
    payload: &str,
    base: &Path,
) -> Result<PathBuf> {
    let path = memory_file_path(key, namespace, base)?;
    let parent = path
        .parent()
        .ok_or_else(|| "invalid memory path".to_owned())?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    atomic_write(&path, payload)?;
    update_indexes(parent, base)?;
    append_log(base, "Update", key, namespace)?;
    Ok(path)
}

pub fn remove_memory_from_disk(key: &str, namespace: &str, base: &Path) -> Result<bool> {
    let path = memory_file_path(key, namespace, base)?;
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).map_err(|e| e.to_string())?;
    let mut current = path.parent().unwrap_or(base).to_path_buf();
    loop {
        update_directory_index(&current, current == base)?;
        if current != base && directory_content_entries(&current)?.is_empty() {
            let index = current.join("index.md");
            if index.exists() {
                let _ = fs::remove_file(index);
            }
            let _ = fs::remove_dir(&current);
        }
        if current == base {
            break;
        }
        current = current
            .parent()
            .filter(|p| p.starts_with(base))
            .unwrap_or(base)
            .to_path_buf();
    }
    append_log(base, "Deletion", key, namespace)?;
    Ok(true)
}

fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|v| v.to_str()).unwrap_or("")
    ));
    fs::write(&tmp, content).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn update_indexes(parent: &Path, base: &Path) -> Result<()> {
    let mut current = parent.to_path_buf();
    loop {
        update_directory_index(&current, current == base)?;
        if current == base {
            break;
        }
        current = current
            .parent()
            .filter(|p| p.starts_with(base))
            .ok_or_else(|| "memory path escaped bundle root".to_owned())?
            .to_path_buf();
    }
    Ok(())
}

fn directory_content_entries(path: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|v| v.to_str())
                .is_some_and(|v| !v.starts_with('.') && v != "index.md" && v != "log.md")
        })
        .collect::<Vec<_>>();
    entries.sort();
    Ok(entries)
}

pub fn update_directory_index(path: &Path, root: bool) -> Result<()> {
    if !path.is_dir() {
        return Ok(());
    }
    let entries = directory_content_entries(path)?;
    let index = path.join("index.md");
    if entries.is_empty() {
        if index.exists() {
            fs::remove_file(index).map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    let mut dirs = Vec::new();
    let mut concepts = Vec::new();
    for entry in entries {
        let name = entry.file_name().unwrap().to_string_lossy().to_string();
        if entry.is_dir() {
            dirs.push(name);
        } else if name.ends_with(".md") {
            concepts.push(name);
        }
    }
    let name = title_case(&path.file_name().unwrap_or_default().to_string_lossy());
    let mut lines = Vec::new();
    if root {
        lines.push("---\nokf_version: \"0.2\"\n---".to_owned());
    }
    lines.push(format!("# {name}\n"));
    if !dirs.is_empty() {
        lines.push("## Directories\n".into());
        lines.extend(dirs.iter().map(|v| format!("* [{v}/]({v}/)")));
        lines.push(String::new());
    }
    if !concepts.is_empty() {
        lines.push("## Concepts\n".into());
        lines.extend(
            concepts
                .iter()
                .map(|v| format!("* [{}]({v})", title_case(v.trim_end_matches(".md")))),
        );
        lines.push(String::new());
    }
    atomic_write(&index, &(lines.join("\n").trim().to_owned() + "\n"))
}

fn append_log(base: &Path, action: &str, key: &str, namespace: &str) -> Result<()> {
    fs::create_dir_all(base).map_err(|e| e.to_string())?;
    let today = OffsetDateTime::now_utc()
        .format(format_description!("[year]-[month]-[day]"))
        .expect("static UTC date format");
    let title = title_case(key.rsplit('/').next().unwrap_or(key));
    let key_path = if key.ends_with(".md") {
        key.trim_start_matches('/').to_owned()
    } else {
        format!("{}.md", key.trim_start_matches('/'))
    };
    let rel = if namespace.is_empty() || namespace == "default" {
        key_path
    } else {
        format!("{namespace}/{key_path}")
    };
    let bullet = format!("* **{action}**: {title} ([{key}](/{}))", rel);
    let log = base.join("log.md");
    let existing = fs::read_to_string(&log).unwrap_or_default();
    let heading = format!("## {today}");
    let updated = if existing.is_empty() {
        format!("# Directory Update Log\n\n{heading}\n{bullet}\n")
    } else if existing.lines().any(|v| v.trim() == heading) {
        let mut lines = Vec::new();
        let mut inserted = false;
        for line in existing.lines() {
            lines.push(line.to_owned());
            if !inserted && line.trim() == heading {
                lines.push(bullet.clone());
                inserted = true;
            }
        }
        lines.join("\n") + "\n"
    } else if let Some(position) = existing.find("\n\n") {
        format!(
            "{}\n\n{heading}\n{bullet}{}",
            &existing[..position],
            &existing[position..]
        )
    } else {
        format!("# Directory Update Log\n\n{heading}\n{bullet}\n\n{existing}")
    };
    atomic_write(&log, &updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trip() {
        let payload = serialize_okf(&OkfOptions {
            key: "user/preferences/theme".into(),
            content: json!("dark mode"),
            tags: vec!["ui".into()],
            namespace: "default".into(),
            ..Default::default()
        })
        .unwrap();
        let parsed = parse_okf(&payload).unwrap();
        assert_eq!(parsed.key, "user/preferences/theme");
        assert_eq!(parsed.tags, ["ui"]);
        assert_eq!(parsed.body, "dark mode");
        assert!(validate_okf_conformance(&payload).0);
    }

    #[test]
    fn structured_and_extended_content() {
        let payload = serialize_okf(&OkfOptions {
            key: "metrics/revenue".into(),
            content: json!({"active": true, "count": 2, "config": {"currency": "USD"}}),
            namespace: "default".into(),
            concept_type: Some("Metric".into()),
            description: Some("Recognized annual revenue.".into()),
            stale_after: Some("2026-12-31".into()),
            generated_by: Some("agent/test-1".into()),
            sources: Some(json!([{"id": "policy", "resource": "https://example.test"}])),
            verified: Some(json!({"by": "human:reviewer", "at": "2026-08-12T19:00:00Z"})),
            created_at: Some("2026-08-12T19:00:00Z".into()),
            updated_at: Some("2026-08-12T19:00:00Z".into()),
            ..Default::default()
        })
        .unwrap();
        assert!(payload.contains("stale_after: '2026-12-31'"));
        assert!(payload.contains("at: '2026-08-12T19:00:00Z'"));
        assert!(payload.contains("- **active**: True"));
        assert!(payload.contains("- **config**: `{\"currency\": \"USD\"}`"));
        assert!(payload.contains("```json"));
        assert!(validate_okf_conformance(&payload).0);
    }

    #[test]
    fn conformance_errors() {
        let (valid, errors) = validate_okf_conformance("---\ntitle: Missing Type\n---\nBody");
        assert!(!valid);
        assert!(errors[0].contains("Required field 'type'"));
        let (valid, errors) = validate_okf_conformance(
            "---\ntype: Agent Memory\ngenerated: {by: invalidactor}\n---\nBody",
        );
        assert!(!valid);
        assert!(errors[0].contains("actor convention"));
    }

    #[test]
    fn bundle_indexes_logs_and_removes() {
        let temp = tempfile::tempdir().unwrap();
        let payload = serialize_okf(&OkfOptions {
            key: "user/preferences/theme".into(),
            content: json!("dark"),
            namespace: "default".into(),
            ..Default::default()
        })
        .unwrap();
        let path = sync_memory_to_disk("user/preferences/theme", "default", &payload, temp.path())
            .unwrap();
        assert!(path.exists());
        assert!(temp.path().join("user/preferences/index.md").exists());
        assert!(
            fs::read_to_string(temp.path().join("index.md"))
                .unwrap()
                .contains("okf_version: \"0.2\"")
        );
        assert!(
            fs::read_to_string(temp.path().join("log.md"))
                .unwrap()
                .contains("**Update**")
        );
        assert!(remove_memory_from_disk("user/preferences/theme", "default", temp.path()).unwrap());
        assert!(!path.exists());
        assert!(
            fs::read_to_string(temp.path().join("log.md"))
                .unwrap()
                .contains("**Deletion**")
        );
    }

    #[test]
    fn blocks_unsafe_paths() {
        let base = Path::new("memory");
        assert!(memory_file_path("../escape", "default", base).is_err());
        assert!(memory_file_path("index", "default", base).is_err());
        assert!(memory_file_path("safe", "../escape", base).is_err());
    }
}
