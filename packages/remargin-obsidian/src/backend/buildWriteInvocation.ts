import type { WriteOpts } from "./types";

/**
 * Build the argv + stdin for a `remargin write`.
 *
 * The body goes on stdin, never argv: a file whose first line is a `---`
 * frontmatter fence (or any `-`/`--` token) would otherwise be parsed as
 * a flag and rejected by clap. stdin also sidesteps the OS arg-length
 * limit on large files. The CLI reads the body from stdin whenever the
 * positional CONTENT is omitted.
 */
export function buildWriteInvocation(
  path: string,
  content: string,
  opts?: WriteOpts
): { args: string[]; stdin: string } {
  const args: string[] = ["write", path];
  if (opts?.create) args.push("--create");
  if (opts?.raw) args.push("--raw");
  return { args, stdin: content };
}
