# Rewriting whole files (without dropping comments)

A common day-to-day need: re-create most or all of a managed `.md` body — restructure sections, fold a discussion into the doc, rewrite a spec. The naive move (hand `write` a fresh whole-file body) **does not work** on a commented file, and this is why plus what to do instead.

## Why a whole-file `write` fails here

`write` parses **your payload** as the new document and runs the comment-preservation gate against the on-disk comment set. So a whole-file payload must itself contain every `` ```remargin `` block. You cannot satisfy that:

- The skill forbids hand-authoring comment blocks, and
- They carry `checksum` (and in strict mode `signature`) fields that must match byte-for-byte — reproducing them by hand breaks the verify gate.

So: **do not whole-file `write` a commented document.** Either rewrite it in pieces (below), or — if the comments are meant to go — rewrite freely and then `purge`/`delete` (destructive, user-initiated only).

## The model

Comment blocks are **pinned** at their current lines. A partial-line `write` (`start_line`/`end_line`) splices your replacement into the existing file's bytes for that range and leaves everything outside the range untouched. So you rewrite the **prose around** the pinned blocks — you don't move them.

## Procedure

1. **Map the file once.** `get path=… line_numbers=true`. Record the line range of every `` ```remargin `` … `` ``` `` block (pinned, never touch) and the prose gaps between/around them (editable).
2. **Rewrite the prose gaps with partial `write`** (`start_line`/`end_line`), **bottom-up — last gap first.** Editing a lower gap never shifts the line numbers of the gaps above it, so the ranges from your single initial `get` stay valid the whole way down. **Never include a comment block's lines in a range.**
3. **Use `replace` for plain substitutions** (rename a term, restate a settled decision). It is body-only and structurally cannot touch comments, so it's order-independent and safe.

## Gotchas

- **`batch` does not cover `write`.** The skill's "never order inserts bottom-up" rule is about *comment* inserts (where `batch` exists). For body writes there is no batch, so **bottom-up is the correct technique here**, not the anti-pattern.
- **Comment blocks can't be relocated this way.** They stay anchored where the discussion happened — after a reorder they may sit next to different prose. That's expected. To remove or relocate them, that's an explicit `delete`/`purge` (destructive, user-initiated).
- **Strict mode is fine.** The verify gate runs per write; since you never touch comment blocks, their signatures stay valid.

## Worked example

A doc with a comment block at lines 20–28, prose in 1–19 and 29–60. Rewrite both prose regions:

```
remargin write --lines 29-60 doc.md <<'EOF'
…new content for the lower section…
EOF
remargin write --lines 1-19 doc.md <<'EOF'
…new content for the upper section…
EOF
```

Lower region first: the 1–19 ranges are still accurate after the 29–60 write because nothing above line 29 moved.
