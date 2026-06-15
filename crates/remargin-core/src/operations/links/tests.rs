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

// 4. External URL kept with path null.
#[test]
fn external_url_kept_path_null() {
    let sys = vault(&[]);
    let body = "See [docs](https://stripe.com/docs) for details.";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "https://stripe.com/docs");
    assert!(links[0].path.is_none());
    assert_eq!(links[0].alias.as_deref(), Some("docs"));
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

// Autolink and bare URL detection.
#[test]
fn autolink_and_bare_url_detected() {
    let sys = vault(&[]);
    let body = "Autolink <https://a.example> and bare https://b.example here.";
    let links = run(body, &sys);
    assert_eq!(links.len(), 2);
    assert!(target_of(&links, "https://a.example").path.is_none());
    assert!(target_of(&links, "https://b.example").path.is_none());
}

// Reference links resolve against their definition.
#[test]
fn reference_link_resolves_against_definition() {
    let sys = vault(&[]);
    let body = "See [the docs][d].\n\n[d]: https://example.com/docs";
    let links = run(body, &sys);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].target, "https://example.com/docs");
    assert_eq!(links[0].alias.as_deref(), Some("the docs"));
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
