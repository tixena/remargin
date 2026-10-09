//! Tests for the structural markdown linter.

use std::path::Path;

use os_shim::mock::MemorySystem;

use crate::config::{Mode, ResolvedConfig};
use crate::linter::LintErrorView;
use crate::linter::{lint, lint_doc, lint_or_fail};
use crate::parser::AuthorType;

const DOC_WITH_UNKNOWN_RECIPIENT: &str = "\
---
title: Test
---

```remargin
---
id: abc
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
to: [eduardo_burgos]
checksum: sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
---
hello
```
";

const DOC_WITH_ACTIVE_RECIPIENT: &str = "\
---
title: Test
---

```remargin
---
id: abc
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
to: [eduardo-burgos]
checksum: sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
---
hello
```
";

const DOC_WITH_REVOKED_RECIPIENT: &str = "\
---
title: Test
---

```remargin
---
id: abc
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
to: [bob]
checksum: sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
---
hello
```
";

const LINT_REGISTRY_YAML: &str = "\
participants:
  alice:
    type: human
    status: active
    pubkeys: []
  bob:
    type: human
    status: revoked
    pubkeys: []
  eduardo-burgos:
    type: human
    status: active
    pubkeys: []
";

fn lint_messages(content: &str) -> Vec<String> {
    lint(content)
        .unwrap()
        .into_iter()
        .map(|err| err.message)
        .collect()
}

fn lint_pairs(content: &str) -> Vec<(usize, String)> {
    lint(content)
        .unwrap()
        .into_iter()
        .map(|err| (err.line, err.message))
        .collect()
}

#[test]
fn valid_document() {
    let doc = "\
---
title: My Document
author: eduardo
---

# Introduction

Some text here.

```remargin
---
id: abc
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:deadbeef
---
This is a review comment.
```

More text.

```python
def hello():
    pass
```
";
    let errors = lint(doc).unwrap();
    assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
}

#[test]
fn unclosed_fence() {
    let doc = "\
Some text.

```python
def hello():
    pass
";
    let errors = lint_pairs(doc);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, 3);
    assert!(errors[0].1.contains("unclosed fenced code block"));
}

#[test]
fn invalid_frontmatter() {
    let doc = "\
---
[bad yaml
---
";
    let errors = lint_messages(doc);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("invalid YAML in frontmatter"));
}

#[test]
fn invalid_remargin_yaml() {
    let doc = "\
```remargin
---
[bad yaml here
---
```
";
    let errors = lint_messages(doc);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("invalid YAML in remargin block header"));
}

#[test]
fn missing_required_field() {
    let doc = "\
```remargin
---
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:deadbeef
---
Missing the id field.
```
";
    let errors = lint_messages(doc);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("missing required field: id"));
}

#[test]
fn fence_depth_mismatch() {
    let doc = "\
````python
some code
```
";
    let errors = lint_pairs(doc);
    assert!(
        !errors.is_empty(),
        "expected at least 1 error, got: {errors:?}"
    );
    assert_eq!(errors[0].0, 1);
    assert!(errors[0].1.contains("unclosed fenced code block"));
    assert!(errors[0].1.contains("4 backticks"));
}

#[test]
fn nested_fences_valid() {
    let doc = "\
````markdown
Here is a code block inside:
```python
print('hello')
```
````
";
    let errors = lint(doc).unwrap();
    assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
}

#[test]
fn multiple_errors() {
    let doc = "\
---
[bad yaml
---

```remargin
---
author: eduardo
---
Missing id, type, ts, checksum.
```

````
unclosed four-backtick block
";
    let errors = lint(doc).unwrap();
    assert!(
        errors.len() >= 3,
        "expected at least 3 errors, got {}: {errors:?}",
        errors.len()
    );
}

#[test]
fn no_fences_clean() {
    let doc = "\
# Just a heading

Some paragraph text.

- List item 1
- List item 2
";
    let errors = lint(doc).unwrap();
    assert!(errors.is_empty());
}

#[test]
fn lint_or_fail_clean() {
    let doc = "# Simple document\n\nSome text.\n";
    lint_or_fail(doc).unwrap();
}

#[test]
fn lint_or_fail_with_errors() {
    let doc = "```python\nunclosed\n";
    let result = lint_or_fail(doc);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("Lint errors"));
    assert!(msg.contains("unclosed fenced code block"));
}

#[test]
fn no_frontmatter_is_fine() {
    let doc = "# No frontmatter here\n\nJust content.\n";
    let errors = lint(doc).unwrap();
    assert!(errors.is_empty());
}

#[test]
fn unclosed_frontmatter() {
    let doc = "\
---
title: Oops
no closing marker
";
    let errors = lint_messages(doc);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("unclosed YAML frontmatter"));
}

#[test]
fn remargin_no_yaml_header() {
    let doc = "\
```remargin
Just content, no --- delimiters at all.
```
";
    let errors = lint_messages(doc);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("missing YAML header"));
}

#[test]
fn remargin_unclosed_yaml_header() {
    let doc = "\
```remargin
---
id: abc
author: eduardo
```
";
    let errors = lint_messages(doc);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("YAML header not closed"));
}

#[test]
fn remargin_all_required_fields_present() {
    let doc = "\
```remargin
---
id: abc
author: eduardo
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:deadbeef
---
Content.
```
";
    let errors = lint(doc).unwrap();
    assert!(errors.is_empty());
}

#[test]
fn remargin_multiple_missing_fields() {
    let doc = "\
```remargin
---
id: abc
---
Content.
```
";
    let errors = lint_messages(doc);
    assert_eq!(errors.len(), 4);
    assert!(errors.iter().any(|e| e.contains("author")));
    assert!(errors.iter().any(|e| e.contains("type")));
    assert!(errors.iter().any(|e| e.contains("ts")));
    assert!(errors.iter().any(|e| e.contains("checksum")));
}

#[test]
fn error_line_numbers_correct() {
    let doc = "\
line 1
line 2
```python
line 4
";
    let errors = lint_pairs(doc);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, 3, "unclosed fence should be on line 3");
}

/// `alice` is an active participant of [`LINT_REGISTRY_YAML`], so the
/// strict-realm fixtures admit her read.
fn alice_config() -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("alice")),
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: None,
        source_path: None,
        trusted_roots: Vec::new(),
        unrestricted: false,
    }
}

fn build_lint_system(doc_content: &str, realm_yaml: &str) -> MemorySystem {
    MemorySystem::new()
        .with_file(Path::new("/vault/doc.md"), doc_content.as_bytes())
        .unwrap()
        .with_file(Path::new("/vault/.remargin.yaml"), realm_yaml.as_bytes())
        .unwrap()
        .with_file(
            Path::new("/vault/.remargin-registry.yaml"),
            LINT_REGISTRY_YAML.as_bytes(),
        )
        .unwrap()
}

#[test]
fn lint_doc_strict_unknown_recipient_finding() {
    let system = build_lint_system(DOC_WITH_UNKNOWN_RECIPIENT, "mode: strict\n");
    let report = lint_doc(&system, Path::new("/vault/doc.md"), &alice_config()).unwrap();
    assert!(
        !report.recipients.is_empty(),
        "expected recipient findings, got none"
    );
    assert!(!report.is_clean(), "report should not be clean");
    let finding = &report.recipients[0];
    assert!(
        finding.message.contains("eduardo_burgos"),
        "finding should name the bad recipient: {:?}",
        finding.message
    );
    assert!(
        finding
            .message
            .contains("not an active registry participant"),
        "finding should explain why: {:?}",
        finding.message
    );
}

#[test]
fn lint_doc_open_mode_no_recipient_findings() {
    let system = MemorySystem::new()
        .with_file(
            Path::new("/vault/doc.md"),
            DOC_WITH_UNKNOWN_RECIPIENT.as_bytes(),
        )
        .unwrap();
    let report = lint_doc(&system, Path::new("/vault/doc.md"), &alice_config()).unwrap();
    assert!(
        report.recipients.is_empty(),
        "open mode should produce no recipient findings"
    );
}

#[test]
fn lint_doc_all_recipients_active_no_findings() {
    let system = build_lint_system(DOC_WITH_ACTIVE_RECIPIENT, "mode: strict\n");
    let report = lint_doc(&system, Path::new("/vault/doc.md"), &alice_config()).unwrap();
    assert!(
        report.recipients.is_empty(),
        "active recipient should produce no findings"
    );
}

#[test]
fn lint_doc_registered_mode_revoked_recipient_finding() {
    let system = build_lint_system(DOC_WITH_REVOKED_RECIPIENT, "mode: registered\n");
    let report = lint_doc(&system, Path::new("/vault/doc.md"), &alice_config()).unwrap();
    assert!(
        !report.recipients.is_empty(),
        "revoked recipient should produce a finding"
    );
    assert!(!report.is_clean());
}

#[test]
fn lint_doc_missing_registry_skipped() {
    let system = MemorySystem::new()
        .with_file(
            Path::new("/vault/doc.md"),
            DOC_WITH_UNKNOWN_RECIPIENT.as_bytes(),
        )
        .unwrap()
        .with_file(Path::new("/vault/.remargin.yaml"), b"mode: registered\n")
        .unwrap();
    let report = lint_doc(&system, Path::new("/vault/doc.md"), &alice_config()).unwrap();
    assert!(
        report.recipients.is_empty(),
        "missing registry should produce no recipient findings"
    );
    assert_eq!(report.errors, [] as [LintErrorView; 0]);
}

#[test]
fn lint_content_pure_no_recipient_checking() {
    let errors = lint(DOC_WITH_UNKNOWN_RECIPIENT).unwrap();
    assert!(
        errors.is_empty(),
        "lint(content) must not check recipients: {errors:?}"
    );
}

#[test]
fn lint_report_to_json_includes_recipients_field() {
    let system = build_lint_system(DOC_WITH_UNKNOWN_RECIPIENT, "mode: registered\n");
    let report = lint_doc(&system, Path::new("/vault/doc.md"), &alice_config()).unwrap();
    let json = report.to_json();
    assert!(
        json.get("recipients").is_some(),
        "to_json must include a 'recipients' key"
    );
    let recipients_arr = json["recipients"].as_array().unwrap();
    assert!(
        !recipients_arr.is_empty(),
        "recipients array should be non-empty"
    );
    assert_eq!(
        json["ok"].as_bool(),
        Some(false),
        "ok should be false when recipients are bad"
    );
}

#[test]
fn lint_report_format_text_includes_recipients() {
    let system = build_lint_system(DOC_WITH_UNKNOWN_RECIPIENT, "mode: registered\n");
    let report = lint_doc(&system, Path::new("/vault/doc.md"), &alice_config()).unwrap();
    let text = report.format_text();
    assert!(
        text.contains("recipients:"),
        "format_text should include recipient findings: {text}"
    );
    assert!(
        text.contains("eduardo_burgos"),
        "format_text should name the bad recipient: {text}"
    );
}
