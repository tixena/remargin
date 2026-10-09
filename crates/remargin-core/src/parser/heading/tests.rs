//! Tests for heading-path resolution: prefix match, nesting, duplicates and the skipped regions.

use super::*;
use crate::parser::parse;

fn doc(markdown: &str) -> ParsedDocument {
    parse(markdown).unwrap()
}

#[test]
fn empty_path_errors() {
    let md = "# A\n";
    let parsed = doc(md);
    resolve_heading_path(&parsed, "").unwrap_err();
    resolve_heading_path(&parsed, "   ").unwrap_err();
}

#[test]
fn first_match_wins_at_same_path() {
    let md = "\
## Section\n\
### Item\n\
\n\
## Other\n\
### Item\n\
";
    let parsed = doc(md);
    assert_eq!(resolve_heading_path(&parsed, "Item").unwrap(), 2);
    assert_eq!(resolve_heading_path(&parsed, "Other > Item").unwrap(), 5);
}

#[test]
fn full_text_also_matches_via_prefix() {
    let md = "### P3. `deny_ops` (operation-level)\n";
    let parsed = doc(md);
    assert_eq!(
        resolve_heading_path(&parsed, "P3. `deny_ops` (operation-level)").unwrap(),
        1
    );
}

#[test]
fn malformed_path_separators_error() {
    let md = "# A\n";
    let parsed = doc(md);
    resolve_heading_path(&parsed, "> A").unwrap_err();
    resolve_heading_path(&parsed, "A >").unwrap_err();
    resolve_heading_path(&parsed, "A > > B").unwrap_err();
}

#[test]
fn missing_heading_errors() {
    let md = "# Title\n";
    let parsed = doc(md);
    let err = resolve_heading_path(&parsed, "Z9.").unwrap_err();
    assert!(format!("{err:#}").contains("Z9."));
}

#[test]
fn path_can_skip_levels() {
    let md = "# A\n\n### C\n";
    let parsed = doc(md);
    assert_eq!(resolve_heading_path(&parsed, "A > C").unwrap(), 3);
}

#[test]
fn path_disambiguates_duplicate_headings() {
    let md = "\
## Activity epic tests\n\
### A10. MCP / CLI parity\n\
\n\
## Permissions epic tests\n\
### P11. MCP / CLI parity\n\
";
    let parsed = doc(md);
    let line = resolve_heading_path(&parsed, "Activity epic tests > A10.").unwrap();
    assert_eq!(line, 2);
}

#[test]
fn resolves_simple_prefix_match() {
    let md = "# Title\n\n### P3. deny_ops\n\nbody\n";
    let parsed = doc(md);
    assert_eq!(resolve_heading_path(&parsed, "P3.").unwrap(), 3);
}

#[test]
fn same_level_child_terminates_parent_section() {
    let md = "## A\n\n## B\n";
    let parsed = doc(md);
    let err = resolve_heading_path(&parsed, "A > B").unwrap_err();
    assert!(format!("{err:#}").contains("A > B"));
}

#[test]
fn skips_headings_inside_code_fences() {
    let md = "\
### Real\n\
\n\
```text\n\
### Fake\n\
```\n\
";
    let parsed = doc(md);
    assert_eq!(resolve_heading_path(&parsed, "Real").unwrap(), 1);
    let err = resolve_heading_path(&parsed, "Fake").unwrap_err();
    assert!(format!("{err:#}").contains("Fake"));
}

#[test]
fn skips_yaml_frontmatter() {
    let md = "\
---\n\
title: doc\n\
remargin_kind: question\n\
---\n\
\n\
### P3. real heading\n\
";
    let parsed = doc(md);
    assert_eq!(resolve_heading_path(&parsed, "P3.").unwrap(), 6);
    let err = resolve_heading_path(&parsed, "remargin_kind").unwrap_err();
    assert!(format!("{err:#}").contains("remargin_kind"));
}

#[test]
fn trailing_atx_hashes_are_stripped() {
    let md = "### Foo ###\n";
    let parsed = doc(md);
    assert_eq!(resolve_heading_path(&parsed, "Foo").unwrap(), 1);
}
