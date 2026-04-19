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
