import { asRecord, stringField } from "./codexProtocol.js";
/** Only confirmations can be represented faithfully by the Allow/Deny UI. */
export function codexMcpConfirmation(params) {
  const rec = asRecord(params);
  const schema = asRecord(rec?.requestedSchema);
  const properties = asRecord(schema?.properties);
  if (
    !["form", "openai/form", "openaiForm"].includes(String(rec?.mode)) ||
    schema?.type !== "object" ||
    !properties ||
    Object.keys(schema).some(
      (key) =>
        ![
          "type",
          "properties",
          "required",
          "title",
          "description",
          "$schema",
          "additionalProperties",
        ].includes(key),
    )
  )
    return null;
  const entries = Object.entries(properties);
  const required = schema.required ?? [];
  if (
    !Array.isArray(required) ||
    required.some(
      (key) =>
        typeof key !== "string" ||
        !Object.prototype.hasOwnProperty.call(properties, key),
    ) ||
    entries.length > 1
  )
    return null;
  let detail;
  let content = {};
  if (entries.length === 1) {
    const [key, value] = entries[0];
    const field = asRecord(value);
    if (
      field?.type !== "boolean" ||
      Object.keys(field).some(
        (name) => !["type", "title", "description", "default"].includes(name),
      )
    )
      return null;
    detail = [
      stringField(field, "title") ?? key,
      stringField(field, "description"),
    ]
      .filter(Boolean)
      .join(" — ");
    content = { [key]: true };
  }
  const message = stringField(rec, "message") ?? "Approve request";
  return {
    title: `${stringField(rec, "serverName") ?? "MCP"}: ${message}${detail ? ` — ${detail}` : ""}`,
    content,
  };
}
