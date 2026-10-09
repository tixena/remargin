//! Unit tests for outbound link extraction.

use std::path::Path;

use os_shim::mock::MemorySystem;

use super::{Link, extract_links};

/// Build a mock vault with a `/vault` dir and the given same-folder
/// target files (name -> contents).
fn vault(targets: &[(&str, &str)]) -> MemorySystem {
    let mut sys = MemorySystem::new().with_dir(Path::new("/vault")).unwrap();
    for (name, contents) in targets {
        sys = sys
            .with_file(Path::new("/vault").join(name), contents.as_bytes())
            .unwrap();
    }
    sys
}

fn run(body: &str, sys: &MemorySystem) -> Vec<Link> {
    extract_links(body, Path::new("/vault"), sys)
}

fn target_of<'link>(links: &'link [Link], target: &str) -> &'link Link {
    links.iter().find(|l| l.target == target).unwrap()
}

#[test]
fn three_distinct_wikilinks_three_entries() {
    let sys = vault(&[
        ("Alpha.md", "# Alpha"),
        ("Beta.md", "# Beta"),
        ("Gamma.md", "# Gamma"),
    ]);
    let body = "See [[Alpha]], then [[Beta]] and [[Gamma]].";
    let links = run(body, &sys);
    assert_eq!(links.len(), 3);
    assert_eq!(target_of(&links, "Alpha").path.as_deref(), Some("Alpha.md"));
    assert_eq!(target_of(&links, "Beta").count, 1);
}

#[test]
fn same_target_thrice_one_entry_count_three() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    let body = "[[Alpha]]\n[[Alpha]]\n[[Alpha]]";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    let link = &links[0];
    assert_eq!(link.count, 3);
    assert_eq!(link.lines.len(), 3);
    assert_eq!(link.lines[0], 1);
    assert_eq!(link.lines[1], 2);
    assert_eq!(link.lines[2], 3);
}

#[test]
fn broken_internal_link_omitted() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    let body = "[[Alpha]] and [[DoesNotExist]]";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "Alpha");
}

#[test]
fn external_url_dropped() {
    let sys = vault(&[]);
    let body = "See [docs](https://stripe.com/docs) for details.";
    let links = run(body, &sys);
    assert!(
        links.is_empty(),
        "external URLs are not returned: {links:?}"
    );
}

#[test]
fn link_in_code_fence_not_detected() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    let body = "```\n[[Alpha]]\n```\n[[Alpha]]";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].count, 1);
    assert_eq!(links[0].lines[0], 4);
}

#[test]
fn link_in_inline_code_not_detected() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    let body = "Use `[[Alpha]]` literally, but [[Alpha]] resolves.";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].count, 1);
}

#[test]
fn wikilink_alias_set() {
    let sys = vault(&[("Budget Model.md", "# Budget Model")]);
    let body = "[[Budget Model|the model]] explains it.";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "Budget Model");
    assert_eq!(links[0].alias.as_deref(), Some("the model"));
}

#[test]
fn frontmatter_up_related_detected() {
    let sys = vault(&[
        ("Parent.md", "# Parent"),
        ("Sibling A.md", "# Sibling A"),
        ("Sibling B.md", "# Sibling B"),
    ]);
    let body = "---\nup: Parent\nrelated: [Sibling A, Sibling B]\n---\n# Doc";
    let links = run(body, &sys);
    assert_eq!(links.len(), 3);
    assert_eq!(
        target_of(&links, "Parent").path.as_deref(),
        Some("Parent.md")
    );
    assert_eq!(
        target_of(&links, "Sibling A").path.as_deref(),
        Some("Sibling A.md")
    );
}

#[test]
fn embed_image_resolvable_path_set() {
    let sys = vault(&[("diagram.png", "PNGDATA")]);
    let body = "![[diagram.png]]";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "diagram.png");
    assert_eq!(links[0].path.as_deref(), Some("diagram.png"));
    assert!(links[0].title.is_none());
}

/// Passing only the slice's lines emulates a sliced read: reference lines start at 1.
#[test]
fn sliced_references_are_slice_relative() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    let slice = "intro line\nsee [[Alpha]]";
    let links = run(slice, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].lines[0], 2);
}

#[test]
fn one_hop_title_from_target() {
    let sys = vault(&[(
        "budget-model.md",
        "---\ntitle: Q3 revenue model\n---\n# Heading",
    )]);
    let body = "[[budget-model]]";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].title.as_deref(), Some("Q3 revenue model"));
}

#[test]
fn title_falls_back_to_heading() {
    let sys = vault(&[("Notes.md", "# Real Heading\n\nbody")]);
    let body = "[[Notes]]";
    let links = run(body, &sys);
    assert_eq!(links[0].title.as_deref(), Some("Real Heading"));
}

#[test]
fn dedup_across_mixed_syntaxes() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    let body = "[[Alpha]] then [Alpha](Alpha.md) then [[Alpha|nick]]";
    let links = run(body, &sys);
    let alpha = target_of(&links, "Alpha");
    assert_eq!(alpha.count, 2);
    assert_eq!(alpha.lines.len(), 2);
}

#[test]
fn heading_and_block_suffixes_resolve_to_note() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    let body = "[[Alpha#Section]] and [[Alpha^block1]]";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "Alpha");
    assert_eq!(links[0].count, 2);
}

#[test]
fn self_anchor_not_a_link() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    let body = "[jump](#section) and [[#heading]] are local";
    let links = run(body, &sys);
    assert_eq!(links, [] as [Link; 0]);
}

#[test]
fn autolink_and_bare_url_dropped() {
    let sys = vault(&[]);
    let body = "Autolink <https://a.example> and bare https://b.example here.";
    let links = run(body, &sys);
    assert!(
        links.is_empty(),
        "external autolinks are dropped: {links:?}"
    );
}

#[test]
fn external_reference_link_dropped() {
    let sys = vault(&[]);
    let body = "See [the docs][d].\n\n[d]: https://example.com/docs";
    let links = run(body, &sys);
    assert!(
        links.is_empty(),
        "external reference link is dropped: {links:?}"
    );
}

#[test]
fn reference_link_without_definition_dropped() {
    let sys = vault(&[]);
    let body = "See [the docs][missing].";
    let links = run(body, &sys);
    assert_eq!(links, [] as [Link; 0]);
}

#[test]
fn md_image_internal_resolvable() {
    let sys = vault(&[("pic.png", "PNG")]);
    let body = "![a picture](pic.png)";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "pic.png");
    assert_eq!(links[0].path.as_deref(), Some("pic.png"));
}

#[test]
fn mixed_local_and_external_returns_only_local() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    let body = "[[Alpha]] \
        [a](https://a.example) [b](https://b.example) \
        <https://c.example> https://d.example mailto:x@y.example";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1, "only the local link survives: {links:?}");
    assert_eq!(links[0].target, "Alpha");
    assert_eq!(links[0].path.as_deref(), Some("Alpha.md"));
}

#[test]
fn repeated_external_target_creates_no_entry() {
    let sys = vault(&[]);
    let body = "[x](https://r.example)\n[x](https://r.example)\n[x](https://r.example)";
    let links = run(body, &sys);
    assert!(links.is_empty(), "repeated external is absent: {links:?}");
}

#[test]
fn null_optional_fields_omitted_from_json() {
    let sys = vault(&[("Plain.md", "no heading, no frontmatter")]);
    let links = run("[[Plain]]", &sys);
    let json = serde_json::to_value(&links[0]).unwrap();
    let map = json.as_object().unwrap();
    assert!(!map.contains_key("alias"), "absent alias omitted: {json}");
    assert!(!map.contains_key("title"), "absent title omitted: {json}");
    assert!(map.contains_key("path"), "path is always present: {json}");

    let sys2 = vault(&[("Beta.md", "---\ntitle: Beta Doc\n---\n# H")]);
    let links2 = run("[[Beta|nick]]", &sys2);
    let json2 = serde_json::to_value(&links2[0]).unwrap();
    let map2 = json2.as_object().unwrap();
    assert_eq!(map2["alias"], "nick");
    assert_eq!(map2["title"], "Beta Doc");
}

#[test]
fn link_with_none_fields_round_trips() {
    let sys = vault(&[("Plain.md", "no heading")]);
    let original = run("[[Plain]]", &sys).remove(0);
    assert!(original.alias.is_none());
    assert!(original.title.is_none());

    let json = serde_json::to_string(&original).unwrap();
    let back: Link = serde_json::from_str(&json).unwrap();
    assert_eq!(back, original);
    assert!(back.alias.is_none());
    assert!(back.title.is_none());
}

#[test]
fn compact_rows_drop_count_and_path() {
    let sys = vault(&[
        ("Beta.md", "---\ntitle: Beta Doc\n---\n# H"),
        ("Plain.md", "no heading"),
    ]);
    let verbose_links = run("[[Beta|nick]] and [[Plain]] and [[Plain]] again", &sys);
    let rows = super::to_compact_rows(verbose_links);

    assert_eq!(super::LINK_COLS, ["alias", "lines", "target", "title"]);

    let (beta_alias, beta_lines, _, beta_title) = rows.iter().find(|r| r.2 == "Beta").unwrap();
    assert_eq!(beta_alias.as_deref(), Some("nick"));
    assert_eq!(beta_title.as_deref(), Some("Beta Doc"));
    assert_eq!(beta_lines, &vec![1]);

    let (plain_alias, plain_lines, _, plain_title) = rows.iter().find(|r| r.2 == "Plain").unwrap();
    assert_eq!(*plain_alias, None);
    assert_eq!(*plain_title, None);
    assert_eq!(plain_lines, &vec![1, 1]);
}

/// `path` is derivable from `target`: verbatim with a file extension, else `target + ".md"`.
#[test]
fn compact_row_path_derivable_from_target() {
    let sys = vault(&[("Note.md", "# Note"), ("img.png", "fakebytes")]);
    let links = run("[[Note]] and ![[img.png]]", &sys);
    for link in &links {
        let derived = if Path::new(&link.target).extension().is_some() {
            link.target.clone()
        } else {
            format!("{}.md", link.target)
        };
        assert_eq!(link.path.as_deref(), Some(derived.as_str()));
    }
    let rows = super::to_compact_rows(links);
    assert!(rows.iter().any(|r| r.2 == "Note"));
    assert!(rows.iter().any(|r| r.2 == "img.png"));
}

/// The row alias renders its `Option` tuple columns as nullable in TypeScript and Zod.
#[test]
fn compact_links_schema_renders_nullable_columns() {
    let row_ts = super::compact_link_row_schema::Schema::ts_definition();
    assert!(
        row_ts.contains("string | null"),
        "TS nullable columns: {row_ts}"
    );
    let row_zod = super::compact_link_row_schema::Schema::zod_schema();
    assert!(
        row_zod.contains("z.nullable(z.string())"),
        "Zod nullable columns: {row_zod}"
    );

    let links_ts = super::compact_links_schema::Schema::ts_definition();
    assert!(
        links_ts.contains("Array<CompactLinkRow>"),
        "compact-links payload references the row type: {links_ts}"
    );
    let links_zod = super::compact_links_schema::Schema::zod_schema();
    assert!(
        links_zod.contains("z.array(CompactLinkRow$Schema)"),
        "compact-links Zod references the row schema: {links_zod}"
    );
}
