export function isTaskListToolName(value) {
  const name = value
    .trim()
    .toLowerCase()
    .replace(/[\s_-]+/g, "");
  return ["todowrite", "writetodos", "updatetodos"].some(
    (candidate) => name === candidate || name.endsWith(candidate),
  );
}
/** Common todo-write payload used by Claude and Pi extensions. */
export function taskListFromToolInput(toolName, input) {
  if (!isTaskListToolName(toolName)) return null;
  const rec = asRecord(input);
  const todos = rec?.todos;
  if (!Array.isArray(todos)) return null;
  return todos.flatMap((todo) => {
    const row = asRecord(todo);
    const text = [row?.content, row?.activeForm, row?.text]
      .find((value) => typeof value === "string" && !!value.trim())
      ?.trim();
    if (!text) return [];
    const id = taskListItemId(row?.id);
    return [
      {
        ...(id ? { id } : {}),
        text,
        status: normalizeTaskListStatus(row?.status),
      },
    ];
  });
}
export function normalizeTaskListStatus(value) {
  const status = String(value ?? "pending")
    .trim()
    .toLowerCase()
    .replace(/[\s_-]+/g, "");
  if (status === "completed" || status === "complete" || status === "done") {
    return "completed";
  }
  if (status === "inprogress" || status === "active" || status === "running") {
    return "in_progress";
  }
  if (status === "cancelled" || status === "canceled" || status === "skipped") {
    return "cancelled";
  }
  return "pending";
}
export function taskListText(items) {
  return items
    .map((item) => `${taskListMark(item.status)} ${item.text}`)
    .join("\n");
}
function taskListMark(status) {
  if (status === "completed") return "[x]";
  if (status === "in_progress") return "[~]";
  if (status === "cancelled") return "[-]";
  return "[ ]";
}
function asRecord(value) {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return value;
  }
  return null;
}
function taskListItemId(value) {
  if (typeof value === "string") return value.trim() || undefined;
  if (typeof value === "number" && Number.isFinite(value)) return String(value);
  return undefined;
}
