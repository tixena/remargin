/**
 * Parses a flat list of vault-relative paths into a hierarchical tree
 * suitable for rendering a collapsible file tree view.
 *
 * Single-child directory chains are collapsed into one node so that, for
 * example, `src/components/Button.tsx` with no siblings under `src/` renders
 * as a single `src/components` directory node rather than two nested levels.
 */

export interface FileTreeNode {
  /** Display name for this node (directory segment or filename). */
  name: string;
  /** Full vault-relative path. For collapsed dirs this is the deepest segment path. */
  fullPath: string;
  /** `true` for directory nodes, `false` for leaf files. */
  isDir: boolean;
  /** Sorted children. Empty for leaf nodes. */
  children: FileTreeNode[];
}

/**
 * Build a tree from a flat path list.
 *
 * Algorithm:
 * 1. Split every path into segments and insert into a trie-like map.
 * 2. Convert the map to `FileTreeNode[]`.
 * 3. Collapse single-child directory chains.
 * 4. Sort: directories first (alphabetical), then files (alphabetical).
 */
export function buildFileTree(paths: string[]): FileTreeNode[] {
  if (paths.length === 0) return [];

  // ---- Step 1: insert paths into a nested map ----
  interface RawNode {
    children: Map<string, RawNode>;
    isFile: boolean;
    fullPath: string;
  }

  const root: Map<string, RawNode> = new Map();

  for (const p of paths) {
    const segments = p.split("/").filter(Boolean);
    let current = root;
    let accumulated = "";

    for (let i = 0; i < segments.length; i++) {
      const seg = segments[i];
      accumulated = accumulated ? `${accumulated}/${seg}` : seg;
      const isLast = i === segments.length - 1;

      let node = current.get(seg);
      if (!node) {
        node = { children: new Map(), isFile: false, fullPath: accumulated };
        current.set(seg, node);
      }
      if (isLast) {
        node.isFile = true;
        node.fullPath = accumulated;
      }
      current = node.children;
    }
  }

  // ---- Step 2: convert map to FileTreeNode[] ----
  function toNodes(map: Map<string, RawNode>, parentPath: string): FileTreeNode[] {
    const nodes: FileTreeNode[] = [];
    for (const [name, raw] of map) {
      const fullPath = parentPath ? `${parentPath}/${name}` : name;
      if (raw.isFile && raw.children.size === 0) {
        // Pure leaf
        nodes.push({ name, fullPath: raw.fullPath, isDir: false, children: [] });
      } else if (raw.isFile && raw.children.size > 0) {
        // A path that is both a file and has children sharing its prefix.
        // Add both a directory node (for children) and a file node.
        nodes.push({ name, fullPath: raw.fullPath, isDir: false, children: [] });
        const children = toNodes(raw.children, fullPath);
        // Wrap children in a synthetic directory only if there are some
        if (children.length > 0) {
          nodes.push({ name, fullPath, isDir: true, children });
        }
      } else {
        // Directory only
        const children = toNodes(raw.children, fullPath);
        nodes.push({ name, fullPath, isDir: true, children });
      }
    }
    return nodes;
  }

  const tree = toNodes(root, "");

  // ---- Step 3: collapse single-child directory chains ----
  function collapse(nodes: FileTreeNode[]): FileTreeNode[] {
    return nodes.map((node) => {
      if (!node.isDir) return node;

      // Recurse first so children are already collapsed.
      let collapsed = { ...node, children: collapse(node.children) };

      // Collapse: if a dir has exactly one child and that child is also a dir,
      // merge them into one node.
      while (
        collapsed.children.length === 1 &&
        collapsed.children[0].isDir
      ) {
        const child = collapsed.children[0];
        collapsed = {
          name: `${collapsed.name}/${child.name}`,
          fullPath: child.fullPath,
          isDir: true,
          children: child.children,
        };
      }

      return collapsed;
    });
  }

  const collapsedTree = collapse(tree);

  // ---- Step 4: sort ----
  function sortNodes(nodes: FileTreeNode[]): FileTreeNode[] {
    const dirs = nodes.filter((n) => n.isDir).sort((a, b) => a.name.localeCompare(b.name));
    const leaves = nodes.filter((n) => !n.isDir).sort((a, b) => a.name.localeCompare(b.name));
    return [
      ...dirs.map((d) => ({ ...d, children: sortNodes(d.children) })),
      ...leaves,
    ];
  }

  return sortNodes(collapsedTree);
}
