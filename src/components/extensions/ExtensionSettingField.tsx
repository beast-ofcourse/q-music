import { useId, useState } from "react";

import type { ActionOutcome, SettingDescriptor } from "../../lib/extensions";
import { toExtensionFailure } from "../../lib/extensions";
import { isNumberTextValid, textOf } from "./setting-values";

interface ExtensionSettingFieldProps {
  descriptor: SettingDescriptor;
  /** Effective value: the user's draft, or the selected quality's value when it overrides. */
  value: unknown;
  /** The quality label when this field's value comes from the quality, otherwise null. */
  overriddenBy: string | null;
  onChange: (key: string, value: unknown) => void;
  onInvoke: (action: string) => Promise<ActionOutcome>;
}

/**
 * One descriptor, drawn from its declared type alone. Nothing here knows what an extension
 * is called or which keys it ships; an unknown type is a disabled control with a reason,
 * never a crash and never a silently dropped row.
 */
export function ExtensionSettingField({
  descriptor,
  value,
  overriddenBy,
  onChange,
  onInvoke,
}: ExtensionSettingFieldProps) {
  const fieldId = useId();
  const controlId = `${fieldId}-control`;
  const descriptionId = `${fieldId}-description`;
  const overrideId = `${fieldId}-override`;
  const invalidId = `${fieldId}-invalid`;

  const disabled = overriddenBy !== null;
  const numberInvalid = descriptor.type === "number" && !isNumberTextValid(value);
  const describedBy = [
    descriptor.description === null ? null : descriptionId,
    overriddenBy === null ? null : overrideId,
    numberInvalid ? invalidId : null,
  ]
    .filter((id): id is string => id !== null)
    .join(" ");

  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<ActionOutcome | null>(null);

  async function runAction(action: string) {
    setBusy(true);
    setOutcome(null);
    try {
      setOutcome(await onInvoke(action));
    } catch (error) {
      setOutcome({ ok: false, message: toExtensionFailure(error).message });
    } finally {
      setBusy(false);
    }
  }

  function drawControl() {
    switch (descriptor.type) {
      case "text":
      case "string":
        return (
          <input
            id={controlId}
            className="input"
            type="text"
            value={textOf(value)}
            disabled={disabled}
            spellCheck={false}
            autoComplete="off"
            aria-describedby={describedBy === "" ? undefined : describedBy}
            onChange={(event) => onChange(descriptor.key, event.target.value)}
          />
        );

      case "number":
        return (
          <input
            id={controlId}
            className="input"
            type="number"
            step="any"
            value={textOf(value)}
            disabled={disabled}
            aria-invalid={numberInvalid}
            aria-describedby={describedBy === "" ? undefined : describedBy}
            onChange={(event) => onChange(descriptor.key, event.target.value)}
          />
        );

      case "boolean":
        return (
          <span className="ext-setting-toggle">
            <input
              id={controlId}
              className="ext-setting-checkbox"
              type="checkbox"
              checked={value === true}
              disabled={disabled}
              aria-describedby={describedBy === "" ? undefined : describedBy}
              onChange={(event) => onChange(descriptor.key, event.target.checked)}
            />
            <span className="ext-setting-state" aria-hidden="true">
              {value === true ? "On" : "Off"}
            </span>
          </span>
        );

      case "select": {
        const current = textOf(value);
        const offered = descriptor.options.some((option) => option.value === current);
        return (
          <select
            id={controlId}
            className="select"
            value={current}
            disabled={disabled}
            aria-describedby={describedBy === "" ? undefined : describedBy}
            onChange={(event) => onChange(descriptor.key, event.target.value)}
          >
            {offered ? null : current === "" ? (
              <option value="">
                {descriptor.options.length === 0 ? "No choices offered" : "Choose..."}
              </option>
            ) : (
              <option value={current}>{current}</option>
            )}
            {descriptor.options.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        );
      }

      case "button": {
        const action = descriptor.action;
        return (
          <div className="ext-setting-action">
            <button
              id={controlId}
              type="button"
              className="button"
              disabled={busy || action === null}
              aria-describedby={describedBy === "" ? undefined : describedBy}
              onClick={() => {
                if (action !== null) {
                  void runAction(action);
                }
              }}
            >
              {busy ? "Working..." : descriptor.label}
            </button>

            {action === null ? (
              <p className="ext-setting-note">
                This button declares no action, so there is nothing to run.
              </p>
            ) : null}
          </div>
        );
      }

      default: {
        const unknownId = `${fieldId}-unknown`;
        return (
          <div className="ext-setting-action">
            <input
              id={controlId}
              className="input"
              type="text"
              value=""
              disabled
              aria-describedby={unknownId}
            />
            <p className="ext-setting-note" id={unknownId}>
              This host does not understand the &quot;{String(descriptor.type)}&quot;
              setting type, so it cannot draw a control for it.
            </p>
          </div>
        );
      }
    }
  }

  const isButton = descriptor.type === "button";

  return (
    <div
      className="ext-setting"
      data-kind={descriptor.type}
      data-overridden={disabled ? "true" : "false"}
    >
      {isButton ? null : (
        <div className="ext-setting-head">
          <label className="ext-setting-name" htmlFor={controlId}>
            {descriptor.label}
          </label>
          {overriddenBy === null ? null : (
            <span className="ext-setting-badge" id={overrideId}>
              From {overriddenBy}
              <span className="sr-only">, set by the quality rather than by you</span>
            </span>
          )}
        </div>
      )}

      {drawControl()}

      {numberInvalid ? (
        <p className="ext-setting-note" id={invalidId}>
          Enter a number, or leave the field empty to unset it.
        </p>
      ) : null}

      {descriptor.description === null ? null : (
        <p className="ext-setting-desc" id={descriptionId}>
          {descriptor.description}
        </p>
      )}

      {isButton ? (
        <p
          className="ext-action-outcome"
          role={outcome !== null && !outcome.ok ? "alert" : "status"}
          data-tone={outcome === null ? undefined : outcome.ok ? "ok" : "error"}
        >
          {outcomeText(outcome)}
        </p>
      ) : null}
    </div>
  );
}

function outcomeText(outcome: ActionOutcome | null): string {
  if (outcome === null) {
    return "";
  }
  if (outcome.message !== null && outcome.message.trim() !== "") {
    return outcome.message;
  }
  return outcome.ok ? "The action completed." : "The action failed without a message.";
}
