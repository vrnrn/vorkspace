export const DEFAULT_PROVIDER_ACCOUNT_ID = "default";
/** Legacy sessions predate persisted account ids and belong to the default profile. */
export function sameProviderAccountId(left, right) {
  return (
    (left ?? DEFAULT_PROVIDER_ACCOUNT_ID) ===
    (right ?? DEFAULT_PROVIDER_ACCOUNT_ID)
  );
}
