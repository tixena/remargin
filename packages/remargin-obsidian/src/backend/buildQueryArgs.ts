/** Argv builder for `remargin query`. */

import type { QueryOpts } from "./types";

/**
 * Build the argv for a `remargin query`.
 *
 * The pending flavors (`--pending`, `--pending-for`, `--pending-for-me`,
 * `--pending-broadcast`) union together in the CLI and AND-compose with
 * `--author` and `--content-regex`.
 */
export function buildQueryArgs(path: string, opts?: QueryOpts): string[] {
  const args: string[] = ["query", path];
  if (opts?.pending) args.push("--pending");
  if (opts?.pendingFor) args.push("--pending-for", opts.pendingFor);
  if (opts?.pendingForMe) args.push("--pending-for-me");
  if (opts?.pendingBroadcast) args.push("--pending-broadcast");
  if (opts?.author) args.push("--author", opts.author);
  if (opts?.since) args.push("--since", opts.since);
  if (opts?.expanded) args.push("--expanded");
  if (opts?.commentId) args.push("--comment-id", opts.commentId);
  if (opts?.contentRegex) args.push("--content-regex", opts.contentRegex);
  if (opts?.ignoreCase) args.push("--ignore-case");
  return args;
}
