//! Runs every identity-aware subcommand with `--config` plus a conflicting identity flag.

use core::str;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

use assert_cmd::Command;

/// One invocation per identity-aware subcommand, with the minimum arguments that get past clap's
/// required-argument check. Nothing executes: clap exits with code 2 first.
const SUBCOMMANDS: &[(&str, &[&str])] = &[
    ("ack", &["foo"]),
    ("activity", &[]),
    ("batch", &["a.md", "--ops", "[]"]),
    ("comment", &["a.md", "hi"]),
    ("comments", &["a.md"]),
    ("cp", &["a.md", "b.md"]),
    ("delete", &["a.md", "foo"]),
    ("edit", &["a.md", "foo", "content"]),
    ("get", &["a.md"]),
    ("get-image", &["a.png"]),
    ("identity", &[]),
    ("lint", &["a.md"]),
    ("ls", &[]),
    ("mcp", &[]),
    ("metadata", &["a.md"]),
    ("mv", &["a.md", "b.md"]),
    ("plan", &["comment", "a.md", "hi"]),
    ("prompt", &["resolve", "a.md"]),
    ("purge", &["a.md"]),
    ("query", &[]),
    ("react", &["a.md", "foo", "thumbsup"]),
    ("replace", &["foo", "bar"]),
    ("rm", &["a.md"]),
    ("sandbox", &["list"]),
    ("search", &["needle"]),
    ("sign", &["a.md", "--ids", "foo"]),
    ("verify", &["a.md"]),
    ("write", &["a.md", "body"]),
];

fn run(args: &[&str]) -> (Option<i32>, String) {
    let output = Command::cargo_bin("remargin")
        .unwrap()
        .args(args)
        .output()
        .unwrap();
    let stderr = String::from(str::from_utf8(&output.stderr).unwrap());
    (output.status.code(), stderr)
}

/// Identity flags go immediately after the parent subcommand name
/// so they attach to the correct clap scope. `plan` flattens
/// `IdentityArgs` on its parent (not per sub-action), so
/// `remargin plan --config X comment a.md hi` is the valid shape;
/// `remargin plan comment a.md hi --config X` would be interpreted
/// as arguments to the `comment` sub-action, which does not accept
/// `--config` here.
fn args_for(cmd: &str, tail: &[&str], extra: &[&str]) -> Vec<String> {
    let mut out = vec![String::from(cmd)];
    out.extend(extra.iter().map(|s| String::from(*s)));
    out.extend(tail.iter().map(|s| String::from(*s)));
    out
}

fn run_strs(args: &[String]) -> (Option<i32>, String) {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run(&refs)
}

fn assert_conflict_strs(args: &[String], flag_label: &str) {
    let (code, stderr) = run_strs(args);
    assert_eq!(
        code,
        Some(2_i32),
        "expected clap exit code 2 for {args:?}, got {code:?}; stderr={stderr}"
    );
    assert!(
        stderr.contains(flag_label) || stderr.contains("conflicts with"),
        "expected clap conflict mentioning {flag_label:?}, got: {stderr}"
    );
}

#[test]
fn config_conflicts_with_identity_on_every_subcommand() {
    for &(cmd, tail) in SUBCOMMANDS {
        let args = args_for(cmd, tail, &["--config", "/x.yaml", "--identity", "alice"]);
        assert_conflict_strs(&args, "--identity");
    }
}

#[test]
fn config_conflicts_with_type_on_every_subcommand() {
    for &(cmd, tail) in SUBCOMMANDS {
        let args = args_for(cmd, tail, &["--config", "/x.yaml", "--type", "human"]);
        assert_conflict_strs(&args, "--type");
    }
}

#[test]
fn config_conflicts_with_key_on_every_subcommand() {
    for &(cmd, tail) in SUBCOMMANDS {
        let args = args_for(cmd, tail, &["--config", "/x.yaml", "--key", "id"]);
        assert_conflict_strs(&args, "--key");
    }
}

/// Kebab-cases a `PascalCase` variant identifier the way clap's
/// `Subcommand` derive names subcommands by default (`cli.rs` has no
/// `rename_all` override).
fn to_kebab_case(ident: &str) -> String {
    let mut out = String::new();
    for (i, ch) in ident.char_indices() {
        if ch.is_uppercase() && i != 0 {
            out.push('-');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

/// Parses `src/cli.rs` and returns the subcommand names of every
/// `Commands` variant that flattens `identity_args: IdentityArgs`.
/// This is the same ground truth `dispatch.rs`'s `subcommand_identity`
/// switches on, read directly from the enum instead of duplicating its
/// match arms.
fn identity_flattening_subcommands() -> HashSet<String> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = Path::new(manifest_dir).join("src/cli.rs");
    let src = fs::read_to_string(&path).unwrap();
    let file = syn::parse_file(&src).unwrap();
    let commands_enum = file
        .items
        .iter()
        .find_map(|item| {
            if let syn::Item::Enum(e) = item {
                (e.ident == "Commands").then_some(e)
            } else {
                None
            }
        })
        .unwrap();
    let struct_has_identity_args = |name: &syn::Ident| -> bool {
        file.items.iter().any(|item| {
            let syn::Item::Struct(st) = item else {
                return false;
            };
            st.ident == *name
                && matches!(&st.fields, syn::Fields::Named(named)
                    if named.named.iter().any(|f| f.ident.as_ref().is_some_and(|i| i == "identity_args")))
        })
    };
    commands_enum
        .variants
        .iter()
        .filter(|v| match &v.fields {
            syn::Fields::Named(named) => named
                .named
                .iter()
                .any(|f| f.ident.as_ref().is_some_and(|i| i == "identity_args")),
            syn::Fields::Unnamed(unnamed) => unnamed.unnamed.iter().any(|f| {
                let syn::Type::Path(p) = &f.ty else {
                    return false;
                };
                p.path
                    .segments
                    .last()
                    .is_some_and(|seg| struct_has_identity_args(&seg.ident))
            }),
            syn::Fields::Unit => false,
        })
        .map(|v| to_kebab_case(&v.ident.to_string()))
        .collect()
}

/// Fails when `SUBCOMMANDS` and the `IdentityArgs`-flattening variants of `Commands` diverge.
#[test]
fn subcommands_table_matches_identity_flattening_commands() {
    let expected = identity_flattening_subcommands();
    let actual: HashSet<String> = SUBCOMMANDS
        .iter()
        .map(|&(name, _)| name.to_owned())
        .collect();
    assert_eq!(
        actual, expected,
        "SUBCOMMANDS in cli_config_conflicts/tests.rs has drifted from the \
         IdentityArgs-flattening variants of Commands in cli.rs; add or remove \
         rows to match"
    );
}
