export function flagValue(args, flag, { required = false } = {}) {
  const index = args.indexOf(flag);
  if (index === -1) {
    if (required) throw new Error(`Missing required ${flag}`);
    return undefined;
  }
  const value = args[index + 1];
  if (value === undefined || value.startsWith('--')) throw new Error(`Missing value for ${flag}`);
  return value;
}
