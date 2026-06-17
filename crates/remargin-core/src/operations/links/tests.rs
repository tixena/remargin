//! Unit tests for outbound link extraction.

use std::path::Path;

use os_shim::mock::MockSystem;

use super::{Link, extract_links};

/// Build a mock vault with a `/vault` dir and the given same-folder
/// target files (name -> contents).
fn vault(targets: &[(&str, &str)]) -> MockSystem {
    let mut sys = MockSystem::new().with_dir(Path::new("/vault")).unwrap();
    for (name, contents) in targets {
        sys = sys
            .with_file(Path::new("/vault").join(name), contents.as_bytes())
            .unwrap();
    }
    sys
}

fn run(body: &str, sys: &MockSystem) -> Vec<Link> {
    extract_links(body, Path::new("/vault"), sys)
}

fn target_of<'link>(links: &'link [Link], target: &str) -> &'link Link {
    links.iter().find(|l| l.target == target).unwrap()
}

// 1. Three distinct resolvable wikilinks → three entries.
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

// 2. Same target ×3 → one entry, count 3, references length 3.
#[test]
fn same_target_thrice_one_entry_count_three() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    let body = "[[Alpha]]\n[[Alpha]]\n[[Alpha]]";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    let link = &links[0];
    assert_eq!(link.count, 3);
    assert_eq!(link.references.len(), 3);
    assert_eq!(link.references[0].line, 1);
    assert_eq!(link.references[1].line, 2);
    assert_eq!(link.references[2].line, 3);
}

// 3. Broken internal link → omitted entirely.
#[test]
fn broken_internal_link_omitted() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    let body = "[[Alpha]] and [[DoesNotExist]]";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "Alpha");
}

// 4. External URL dropped entirely (local links only).
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

// 5. Link inside a code fence → not detected.
#[test]
fn link_in_code_fence_not_detected() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    let body = "```\n[[Alpha]]\n```\n[[Alpha]]";
    let links = run(body, &sys);
    // Only the out-of-fence occurrence counts.
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].count, 1);
    assert_eq!(links[0].references[0].line, 4);
}

// 5b. Link inside an inline code span → not detected.
#[test]
fn link_in_inline_code_not_detected() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    let body = "Use `[[Alpha]]` literally, but [[Alpha]] resolves.";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].count, 1);
}

// 6. Wikilink with alias → alias set.
#[test]
fn wikilink_alias_set() {
    let sys = vault(&[("Budget Model.md", "# Budget Model")]);
    let body = "[[Budget Model|the model]] explains it.";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "Budget Model");
    assert_eq!(links[0].alias.as_deref(), Some("the model"));
}

// 7. Frontmatter up / related → detected.
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

// 8. Embed of a resolvable image → path set.
#[test]
fn embed_image_resolvable_path_set() {
    let sys = vault(&[("diagram.png", "PNGDATA")]);
    let body = "![[diagram.png]]";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "diagram.png");
    assert_eq!(links[0].path.as_deref(), Some("diagram.png"));
    // Non-markdown target → no title.
    assert!(links[0].title.is_none());
}

// 9. Sliced read → references slice-relative.
//
// The caller (get_with_links) slices and feeds the slice text in; here we
// emulate by passing only the slice's lines so reference lines start at 1
// for the slice's first line.
#[test]
fn sliced_references_are_slice_relative() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    // Slice text: two lines, link on the second.
    let slice = "intro line\nsee [[Alpha]]";
    let links = run(slice, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].references[0].line, 2);
}

// 10. One-hop title → title equals the target doc's own title.
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

// 10b. Title falls back to first heading when no frontmatter title.
#[test]
fn title_falls_back_to_heading() {
    let sys = vault(&[("Notes.md", "# Real Heading\n\nbody")]);
    let body = "[[Notes]]";
    let links = run(body, &sys);
    assert_eq!(links[0].title.as_deref(), Some("Real Heading"));
}

// 11. Dedup / references correctness across mixed syntaxes for one target.
#[test]
fn dedup_across_mixed_syntaxes() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    let body = "[[Alpha]] then [Alpha](Alpha.md) then [[Alpha|nick]]";
    let links = run(body, &sys);
    // `Alpha` (wikilink) and `Alpha.md` (md-link) are distinct targets by
    // text: wikilinks carry no extension, md-links carry the file name.
    let alpha = target_of(&links, "Alpha");
    assert_eq!(alpha.count, 2);
    assert_eq!(alpha.references.len(), 2);
}

// 12. Heading / block / anchor handling.
#[test]
fn heading_and_block_suffixes_resolve_to_note() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    let body = "[[Alpha#Section]] and [[Alpha^block1]]";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "Alpha");
    assert_eq!(links[0].count, 2);
}

// 12b. Pure self-anchors are not outbound links.
#[test]
fn self_anchor_not_a_link() {
    let sys = vault(&[("Alpha.md", "# Alpha")]);
    let body = "[jump](#section) and [[#heading]] are local";
    let links = run(body, &sys);
    assert!(links.is_empty());
}

// Autolinks and bare URLs are external → dropped.
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

// A reference link resolving to an external URL is dropped.
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

// A reference link with no matching definition is dropped.
#[test]
fn reference_link_without_definition_dropped() {
    let sys = vault(&[]);
    let body = "See [the docs][missing].";
    let links = run(body, &sys);
    assert!(links.is_empty());
}

// Markdown image with resolvable internal source → path set.
#[test]
fn md_image_internal_resolvable() {
    let sys = vault(&[("pic.png", "PNG")]);
    let body = "![a picture](pic.png)";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "pic.png");
    assert_eq!(links[0].path.as_deref(), Some("pic.png"));
}

// One resolvable internal link amid five external links → only the
// internal one survives, and it carries a real path.
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

// An external target repeated three times creates no entry at all (no
// entry, no references).
#[test]
fn repeated_external_target_creates_no_entry() {
    let sys = vault(&[]);
    let body = "[x](https://r.example)\n[x](https://r.example)\n[x](https://r.example)";
    let links = run(body, &sys);
    assert!(links.is_empty(), "repeated external is absent: {links:?}");
}

// `alias` / `title` keys are omitted (not null) when absent; present when
// the link supplies them.
#[test]
fn null_optional_fields_omitted_from_json() {
    // Internal link, no alias, target with no title → alias + title absent.
    let sys = vault(&[("Plain.md", "no heading, no frontmatter")]);
    let links = run("[[Plain]]", &sys);
    let json = serde_json::to_value(&links[0]).unwrap();
    let map = json.as_object().unwrap();
    assert!(!map.contains_key("alias"), "absent alias omitted: {json}");
    assert!(!map.contains_key("title"), "absent title omitted: {json}");
    assert!(map.contains_key("path"), "path is always present: {json}");

    // Aliased link to a titled target → both keys present.
    let sys2 = vault(&[("Beta.md", "---\ntitle: Beta Doc\n---\n# H")]);
    let links2 = run("[[Beta|nick]]", &sys2);
    let json2 = serde_json::to_value(&links2[0]).unwrap();
    let map2 = json2.as_object().unwrap();
    assert_eq!(map2["alias"], "nick");
    assert_eq!(map2["title"], "Beta Doc");
}

// `skip_serializing_if` only affects output: a Link with `None` fields
// round-trips back to `None`.
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
