//! Tests for the structural markdown linter.

use crate::linter::{lint, lint_or_fail};

/// Lint content and collect error messages.
fn lint_messages(content: &str) -> Vec<String> {
    lint(content)
        .unwrap()
        .into_iter()
        .map(|err| err.message)
        .collect()
}

/// Lint content and return (line, message) pairs.
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
    assert_eq!(errors[0].0, 3); // line 3
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
    // Opened with 4 backticks, "closed" with 3 -- the 3-backtick line
    // does not close the 4-backtick block.
    let doc = "\
````python
some code
```
";
    let errors = lint_pairs(doc);
    // Two errors: the 4-backtick block is unclosed, and the 3-backtick
    // line is also detected as an unclosed opener.
    assert!(
        !errors.is_empty(),
        "expected at least 1 error, got: {errors:?}"
    );
    assert_eq!(errors[0].0, 1); // line 1
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
    // Should have: invalid frontmatter + unclosed 4-backtick fence + missing remargin fields
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
    // Missing: author, type, ts, checksum
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
