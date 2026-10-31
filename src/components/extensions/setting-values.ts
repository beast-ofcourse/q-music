import type { SettingDescriptor } from "../../lib/extensions";

/**
 * The user's layer of settings, before any quality override.
 *
 * Text and number fields keep exactly what the user typed so a failed save never loses it;
 * numbers are coerced only when the payload is built.
 */
export type SettingDraft = Record<string, unknown>;

export function textOf(value: unknown): string {
  if (value === null || value === undefined) {
    return "";
  }
  return String(value);
}

/** A number field may be blank (unset); anything else must parse as a finite number. */
export function isNumberTextValid(value: unknown): boolean {
  const text = textOf(value).trim();
  return text === "" || Number.isFinite(Number(text));
}

function numberFromText(value: unknown): number | null {
  const text = textOf(value).trim();
  if (text === "") {
    return null;
  }
  const parsed = Number(text);
  return Number.isFinite(parsed) ? parsed : null;
}

export function draftFromSaved(
  descriptors: SettingDescriptor[],
  saved: Record<string, unknown>,
): SettingDraft {
  const draft: SettingDraft = {};
  descriptors.forEach((descriptor) => {
    switch (descriptor.type) {
      case "boolean":
        draft[descriptor.key] = saved[descriptor.key] === true;
        return;
      case "number":
      case "select":
      case "string":
      case "text":
        draft[descriptor.key] = textOf(saved[descriptor.key]);
        return;
      case "button":
        return;
      default:
        // A type this host cannot draw is never edited, so it never enters the draft.
        return;
    }
  });
  return draft;
}

/** Only declared, non-button settings go back to the host. */
export function valuesForSave(
  descriptors: SettingDescriptor[],
  draft: SettingDraft,
): Record<string, unknown> {
  const values: Record<string, unknown> = {};
  descriptors.forEach((descriptor) => {
    const drafted = draft[descriptor.key];
    switch (descriptor.type) {
      case "boolean":
        values[descriptor.key] = drafted === true;
        return;
      case "number":
        values[descriptor.key] = numberFromText(drafted);
        return;
      case "select":
      case "string":
      case "text":
        values[descriptor.key] = textOf(drafted);
        return;
      case "button":
        return;
      default:
        return;
    }
  });
  return values;
}

export function invalidNumberSettings(
  descriptors: SettingDescriptor[],
  draft: SettingDraft,
): SettingDescriptor[] {
  return descriptors.filter(
    (descriptor) =>
      descriptor.type === "number" && !isNumberTextValid(draft[descriptor.key]),
  );
}

/** Values arrive as JSON, so structural equality is the honest comparison here. */
export function sameSettingValue(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}
