/** Statuses a provider uses for a call that did not work. */
export function isFailedStatus(status) {
  const value = status?.toLowerCase() ?? "";
  return (
    value === "failed" ||
    value === "error" ||
    value === "cancelled" ||
    value === "canceled"
  );
}
export function hasReadableText(text) {
  return /[^\s\p{P}]/u.test(text);
}
