/** A fallback for adapters that report exhausted accounts as plain errors. */
export function usageLimitFromError(message) {
  return /(?:usage|spending|monthly|weekly|daily) limit (?:reached|exceeded)|(?:quota|credits?) (?:exceeded|exhausted|depleted)|insufficient[_ ](?:quota|credits)|hit your (?:usage )?limit|credit balance is too low/i.test(
    message,
  )
    ? {}
    : undefined;
}
