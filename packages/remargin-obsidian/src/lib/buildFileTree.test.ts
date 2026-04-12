import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { buildFileTree, type FileTreeNode } from "./buildFileTree.ts";

/** Collect all fullPaths from a tree (depth-first). */
function collectPaths(nodes: FileTreeNode[]): string[] {
  const result: string[] = [];
  for (const node of nodes) {
    result.push(node.fullPath);
    result.push(...collectPaths(node.children));
  }
  return result;
}

/** Collect only leaf (file) fullPaths. */
function collectLeafPaths(nodes: FileTreeNode[]): string[] {
  const result: string[] = [];
  for (const node of nodes) {
    if (!node.isDir) {
      result.push(node.fullPath);
    }
    result.push(...collectLeafPaths(node.children));
  }
  return result;
}

describe("buildFileTree", () => {
  it("returns an empty array for empty input", () => {
    assert.deepStrictEqual(buildFileTree([]), []);
  });

  it("returns flat files at root level", () => {
    const tree = buildFileTree(["a.md", "b.md", "c.md"]);
    assert.strictEqual(tree.length, 3);
    for (const node of tree) {
      assert.strictEqual(node.isDir, false);
      assert.strictEqual(node.children.length, 0);
    }
    assert.deepStrictEqual(
      tree.map((n) => n.name),
      ["a.md", "b.md", "c.md"]
    );
  });

  it("groups files by directory", () => {
    const tree = buildFileTree(["src/a.ts", "src/b.ts", "docs/readme.md"]);
    // Two top-level dirs
    assert.strictEqual(tree.length, 2);
    assert.strictEqual(tree[0].isDir, true);
    assert.strictEqual(tree[1].isDir, true);
    // Sorted alphabetically: docs before src
    assert.strictEqual(tree[0].name, "docs");
    assert.strictEqual(tree[1].name, "src");
    assert.strictEqual(tree[0].children.length, 1);
    assert.strictEqual(tree[1].children.length, 2);
  });

  it("collapses single-child directory chains", () => {
    const tree = buildFileTree(["a/b/c/file.md"]);
    // a, b, c each have one child -> collapsed into "a/b/c"
    assert.strictEqual(tree.length, 1);
    assert.strictEqual(tree[0].isDir, true);
    assert.strictEqual(tree[0].name, "a/b/c");
    assert.strictEqual(tree[0].children.length, 1);
    assert.strictEqual(tree[0].children[0].name, "file.md");
    assert.strictEqual(tree[0].children[0].isDir, false);
  });

  it("does not collapse directories with multiple children", () => {
    const tree = buildFileTree(["a/b/x.ts", "a/b/y.ts"]);
    // a has one child (b), so a/b collapses. b has two children, so it stops.
    assert.strictEqual(tree.length, 1);
    assert.strictEqual(tree[0].name, "a/b");
    assert.strictEqual(tree[0].children.length, 2);
  });

  it("preserves all original file paths in the tree", () => {
    const paths = [
      "src/components/Button.tsx",
      "src/components/Input.tsx",
      "src/lib/utils.ts",
      "README.md",
    ];
    const tree = buildFileTree(paths);
    const leafPaths = collectLeafPaths(tree);
    assert.deepStrictEqual(leafPaths.sort(), [...paths].sort());
  });

  it("sorts directories before files", () => {
    const tree = buildFileTree(["z.md", "a/file.ts"]);
    assert.strictEqual(tree[0].isDir, true);
    assert.strictEqual(tree[0].name, "a");
    assert.strictEqual(tree[1].isDir, false);
    assert.strictEqual(tree[1].name, "z.md");
  });

  it("handles deeply nested single-child collapse", () => {
    const tree = buildFileTree(["x/y/z/w/v/leaf.md"]);
    assert.strictEqual(tree.length, 1);
    assert.strictEqual(tree[0].name, "x/y/z/w/v");
    assert.strictEqual(tree[0].isDir, true);
    assert.strictEqual(tree[0].children.length, 1);
    assert.strictEqual(tree[0].children[0].name, "leaf.md");
  });

  it("handles mixed depth files correctly", () => {
    const tree = buildFileTree(["root.md", "a/deep/file.ts", "a/shallow.ts"]);
    // "a" has 2 children (deep/ and shallow.ts), so it is not collapsed.
    const dirA = tree.find((n) => n.isDir && n.name === "a");
    assert.ok(dirA, "should have a directory node for 'a'");
    assert.strictEqual(dirA.children.length, 2);
    // "deep" is a single-child dir under "a" — it collapses but "a" doesn't.
    const deepDir = dirA.children.find((n) => n.isDir);
    assert.ok(deepDir);
    assert.strictEqual(deepDir.name, "deep");
  });

  it("fullPath on collapsed directory points to deepest segment", () => {
    const tree = buildFileTree(["a/b/c/file.md"]);
    assert.strictEqual(tree[0].fullPath, "a/b/c");
  });
});
