/** Shared clarifying-question model for every harness that can ask the user. */
export const CUSTOM_OPTION_ID = "__custom__";
export function questionsFromUnknown(value) {
  const rec = asRecord(value);
  const nested =
    asRecord(rec?.input) ?? asRecord(rec?.params) ?? asRecord(rec?.question);
  const raw = firstArray(
    rec?.questions,
    nested?.questions,
    Array.isArray(value) ? value : undefined,
  );
  const usedIds = new Set();
  return raw.flatMap((item, index) => {
    const parsed = questionFromUnknown(item, index, usedIds);
    return parsed ? [parsed] : [];
  });
}
export function questionPromptTitle(questions) {
  const first = questions[0];
  return first?.header || first?.prompt || "Question";
}
export function selectedAnswerLabels(question, reply) {
  const custom = reply.custom?.[question.id]?.trim();
  const selected = reply.answers[question.id] ?? [];
  if (selected.length === 0 && custom && question.allowCustom) return [custom];
  return selected.flatMap((id) => {
    if (isCustomSelection(question, id)) return custom ? [custom] : [];
    const option = question.options.find((item) => item.id === id);
    const label = option?.label.trim() || id.trim();
    return label ? [label] : [];
  });
}
export function isCustomSelection(question, optionId) {
  if (optionId === CUSTOM_OPTION_ID) return true;
  const option = question.options.find((item) => item.id === optionId);
  return option ? isOtherOption(option) : false;
}
export function isOtherOption(option) {
  return option.id === CUSTOM_OPTION_ID || /^other$/i.test(option.label.trim());
}
function questionFromUnknown(value, index, usedIds) {
  const rec = asRecord(value);
  if (!rec) return null;
  const prompt =
    stringField(rec, "question") ??
    stringField(rec, "prompt") ??
    stringField(rec, "text") ??
    stringField(rec, "header");
  const options = optionsFromUnknown(rec.options);
  const allowCustom = customAllowed(rec, options);
  if (!prompt && options.length === 0 && !allowCustom) return null;
  const header = stringField(rec, "header") ?? stringField(rec, "title");
  return {
    id: uniqueId(
      stringField(rec, "id") ?? prompt ?? header ?? `q${index + 1}`,
      usedIds,
    ),
    ...(header ? { header } : {}),
    prompt: prompt || header || `Question ${index + 1}`,
    multiSelect:
      rec.multiSelect === true ||
      rec.allowMultiple === true ||
      rec.multiple === true,
    allowCustom,
    options,
  };
}
function optionsFromUnknown(value) {
  if (!Array.isArray(value)) return [];
  const usedIds = new Set();
  return value.flatMap((item) => {
    if (typeof item === "string" && item.trim()) {
      const label = item.trim();
      return [{ id: uniqueId(label, usedIds), label }];
    }
    const rec = asRecord(item);
    if (!rec) return [];
    const label =
      stringField(rec, "label") ??
      stringField(rec, "value") ??
      stringField(rec, "text") ??
      stringField(rec, "id");
    if (!label) return [];
    const description =
      stringField(rec, "description") ?? stringField(rec, "detail");
    return [
      {
        id: uniqueId(
          stringField(rec, "id") ??
            stringField(rec, "optionId") ??
            stringField(rec, "value") ??
            label,
          usedIds,
        ),
        label,
        ...(description ? { description } : {}),
      },
    ];
  });
}
function customAllowed(rec, options) {
  if (typeof rec.custom === "boolean") return rec.custom;
  if (typeof rec.allowCustom === "boolean") return rec.allowCustom;
  if (options.some(isOtherOption)) return true;
  // Provider options may carry stable IDs; free text adds Other in the UI.
  if (stringField(rec, "prompt") && !stringField(rec, "question")) return false;
  return true;
}
function uniqueId(seed, used) {
  const base = seed.trim() || "option";
  let next = base;
  let n = 2;
  while (used.has(next)) {
    next = `${base}:${n}`;
    n += 1;
  }
  used.add(next);
  return next;
}
function firstArray(...values) {
  for (const value of values) {
    if (Array.isArray(value)) return value;
  }
  return [];
}
function asRecord(value) {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return value;
  }
  return null;
}
function stringField(rec, key) {
  if (!rec) return undefined;
  const value = rec[key];
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}
