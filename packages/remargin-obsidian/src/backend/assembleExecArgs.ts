/** Argv assembly for one CLI invocation. */

/**
 * Places the per-subcommand flags (identity, then `--json`) right after the subcommand name,
 * ahead of its own arguments. Identity flags are dropped when the caller skips them or the
 * subcommand does not accept them.
 */
export function assembleExecArgs(params: {
  args: string[];
  identityArgs: string[];
  useJson: boolean;
  identityAccepted: boolean;
  skipIdentity: boolean;
}): string[] {
  const { args, identityArgs, useJson, identityAccepted, skipIdentity } = params;
  const effectiveIdentity = skipIdentity || !identityAccepted ? [] : identityArgs;
  const perSubcommandFlags = [...effectiveIdentity, ...(useJson ? ["--json"] : [])];
  const subcommand = args[0];
  if (subcommand === undefined) {
    return perSubcommandFlags;
  }
  return [subcommand, ...perSubcommandFlags, ...args.slice(1)];
}
