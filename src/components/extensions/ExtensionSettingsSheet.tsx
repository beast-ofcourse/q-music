import { useCallback, useEffect, useId, useRef, useState } from "react";

import { Icon } from "../Icon";
import { backend } from "../../lib/ipc";
import {
  toExtensionFailure,
  type ActionOutcome,
  type ExtensionSettingsView,
  type QualityDescriptor,
  type SettingDescriptor,
} from "../../lib/extensions";
import { ExtensionSettingField } from "./ExtensionSettingField";
import {
  draftFromSaved,
  invalidNumberSettings,
  sameSettingValue,
  valuesForSave,
  type SettingDraft,
} from "./setting-values";
import "../../styles/extensions-settings.css";

interface ExtensionSettingsSheetProps {
  extensionId: string;
  onClose: () => void;
}

interface SaveMessage {
  tone: "ok" | "error";
  text: string;
}

const SKELETON_ROWS = 4;

/**
 * Settings for one installed package, generated from its descriptors.
 *
 * The sheet replaces the catalogue rather than floating over it: the now panel is already
 * a right-hand drawer below 1181px, and two drawers over one body would leave no honest
 * place to read the values. Replacing the view also gives a long manifest the whole window
 * height, with the head and the save bar pinned and only the fields scrolling.
 */
export function ExtensionSettingsSheet({ extensionId, onClose }: ExtensionSettingsSheetProps) {
  const [view, setView] = useState<ExtensionSettingsView | null>(null);
  const [draft, setDraft] = useState<SettingDraft>({});
  const [quality, setQuality] = useState("");
  const [loading, setLoading] = useState(true);
  const [loadFailure, setLoadFailure] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const [saving, setSaving] = useState(false);
  const [saveMessage, setSaveMessage] = useState<SaveMessage | null>(null);
  const [confirmingLeave, setConfirmingLeave] = useState(false);

  const sheetRef = useRef<HTMLElement>(null);
  const saveButtonRef = useRef<HTMLButtonElement>(null);
  const keepEditingRef = useRef<HTMLButtonElement>(null);

  const titleId = useId();
  const qualityLegendId = useId();
  const qualityDescriptionId = useId();
  const qualityOverrideId = useId();
  const settingsHintId = useId();

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setLoadFailure(null);
    setSaveMessage(null);

    backend
      .extensionSettings(extensionId)
      .then((next) => {
        if (cancelled) return;
        setView(next);
        setDraft(draftFromSaved(next.settings, next.savedValues));
        setQuality(next.selectedQuality);
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        setView(null);
        setLoadFailure(toExtensionFailure(error).message);
      })
      .finally(() => {
        if (cancelled) return;
        setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [extensionId, reloadKey]);

  useEffect(() => {
    sheetRef.current?.focus();
  }, []);

  useEffect(() => {
    if (!confirmingLeave) return;
    keepEditingRef.current?.focus();
  }, [confirmingLeave]);

  const activeQuality =
    view === null
      ? null
      : view.qualities.find((candidate) => candidate.id === quality) ?? null;
  const overrideValues = view === null ? new Map<string, unknown>() : qualityOverrides(activeQuality, view);
  const invalidSettings = view === null ? [] : invalidNumberSettings(view.settings, draft);
  const savedSignature =
    view === null
      ? ""
      : JSON.stringify({
          values: valuesForSave(view.settings, draftFromSaved(view.settings, view.savedValues)),
          quality: view.selectedQuality,
        });
  const draftSignature =
    view === null ? "" : JSON.stringify({ values: valuesForSave(view.settings, draft), quality });
  const dirty = view !== null && savedSignature !== draftSignature;
  const overriddenLabels = view === null
    ? []
    : view.settings
        .filter((descriptor) => overrideValues.has(descriptor.key))
        .map((descriptor) => descriptor.label);
  const canSave = view !== null && !saving && invalidSettings.length === 0;
  const displayName = view?.displayName ?? extensionId;
  const saveStatus =
    saveMessage === null
      ? ""
      : saveMessage.tone === "ok"
        ? saveMessage.text
        : `Settings were not saved. ${saveMessage.text}`;

  function changeSetting(key: string, next: unknown) {
    setDraft((current) => ({ ...current, [key]: next }));
    setSaveMessage(null);
  }

  function changeQuality(next: string) {
    setQuality(next);
    setSaveMessage(null);
  }

  const requestLeave = useCallback(() => {
    if (dirty) {
      setConfirmingLeave(true);
      return;
    }
    onClose();
  }, [dirty, onClose]);

  const keepEditing = useCallback(() => {
    setConfirmingLeave(false);
    saveButtonRef.current?.focus();
  }, []);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      // Capture phase, so the window shortcut that closes the now panel never also fires.
      event.preventDefault();
      event.stopPropagation();
      if (confirmingLeave) {
        keepEditing();
        return;
      }
      requestLeave();
    }

    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [confirmingLeave, keepEditing, requestLeave]);

  const invokeAction = useCallback(
    async (action: string): Promise<ActionOutcome> =>
      backend.invokeExtensionAction(extensionId, action),
    [extensionId],
  );

  async function saveSettings() {
    if (view === null || saving || invalidSettings.length > 0) return;
    setSaving(true);
    setSaveMessage(null);
    try {
      const saved = await backend.saveExtensionSettings(
        extensionId,
        valuesForSave(view.settings, draft),
        quality,
      );
      setView(saved);
      setDraft(draftFromSaved(saved.settings, saved.savedValues));
      setQuality(saved.selectedQuality);
      setSaveMessage({ tone: "ok", text: "Settings saved." });
    } catch (error) {
      setSaveMessage({ tone: "error", text: toExtensionFailure(error).message });
    } finally {
      setSaving(false);
    }
  }

  function fieldValue(descriptor: SettingDescriptor): unknown {
    if (overrideValues.has(descriptor.key)) {
      return overrideValues.get(descriptor.key);
    }
    return draft[descriptor.key];
  }

  return (
    <section
      className="ext-settings"
      aria-labelledby={titleId}
      tabIndex={-1}
      ref={sheetRef}
    >
      <header className="ext-settings-head">
        <button
          type="button"
          className="button button-quiet ext-settings-back"
          aria-label="Back to the extension catalogue"
          onClick={requestLeave}
        >
          <Icon name="chevron-left" size={14} />
          Extensions
        </button>

        <div className="ext-settings-identity">
          <p className="ext-settings-eyebrow">Extension settings</p>
          <div className="ext-settings-title-row">
            <h1 className="view-title ext-settings-title" id={titleId}>
              {displayName}
            </h1>
            <p className="ext-settings-meta">
              <span>{extensionId}</span>
              <span aria-hidden="true">/</span>
              <span>{view === null ? "reading version" : `v${view.version}`}</span>
            </p>
          </div>
        </div>

        {dirty ? <span className="ext-settings-dirty">Unsaved changes</span> : null}
      </header>

      <div className="ext-settings-scroll">
        {loading ? (
          <div className="ext-settings-inner">
            <div className="ext-settings-loading" aria-hidden="true">
              {Array.from({ length: SKELETON_ROWS }, (_, index) => (
                <div key={index} className="ext-settings-skeleton" />
              ))}
            </div>
            <p className="sr-only" role="status">
              Reading settings for {displayName}.
            </p>
          </div>
        ) : loadFailure !== null ? (
          <div className="ext-settings-inner">
            <div className="ext-settings-error" role="alert">
              <p className="ext-settings-error-title">The settings could not be read</p>
              <p className="ext-settings-error-body">{loadFailure}</p>
              <div className="actions">
                <button
                  type="button"
                  className="button"
                  onClick={() => setReloadKey((current) => current + 1)}
                >
                  Try again
                </button>
                <button type="button" className="button button-quiet" onClick={onClose}>
                  Back to Extensions
                </button>
              </div>
            </div>
          </div>
        ) : view === null ? null : (
          <div className="ext-settings-inner">
            {view.qualities.length === 0 ? null : (
              <fieldset className="ext-settings-quality">
                <legend className="ext-settings-legend" id={qualityLegendId}>
                  Quality
                </legend>
                <div className="ext-quality-row">
                  <select
                    className="select ext-quality-select"
                    value={quality}
                    aria-labelledby={qualityLegendId}
                    aria-describedby={`${qualityDescriptionId} ${qualityOverrideId}`}
                    onChange={(event) => changeQuality(event.target.value)}
                  >
                    {view.qualities.some((candidate) => candidate.id === quality) ? null : (
                      <option value={quality}>{quality} (not offered by this package)</option>
                    )}
                    {view.qualities.map((candidate) => (
                      <option key={candidate.id} value={candidate.id}>
                        {candidate.label}
                      </option>
                    ))}
                  </select>
                  <p className="ext-quality-desc" id={qualityDescriptionId}>
                    {activeQuality?.description ??
                      "No description was published for this quality."}
                  </p>
                </div>
                <p className="ext-quality-override" id={qualityOverrideId}>
                  {overriddenLabels.length > 0 ? (
                    <>
                      This quality overrides <strong>{overriddenLabels.join(", ")}</strong>.
                      The fields it sets are marked below.
                    </>
                  ) : (
                    "This quality overrides nothing; the values you saved apply unchanged."
                  )}
                </p>
              </fieldset>
            )}

            <fieldset
              className="ext-settings-group"
              aria-describedby={overriddenLabels.length > 0 ? settingsHintId : undefined}
            >
              <legend className="ext-settings-legend">Settings</legend>

              {overriddenLabels.length > 0 ? (
                <p className="ext-settings-hint" id={settingsHintId}>
                  Fields marked &quot;From ...&quot; show the value this quality applies.
                  Choose another quality to edit them.
                </p>
              ) : null}

              {view.settings.length === 0 ? (
                <div className="ext-settings-empty">
                  <p className="ext-settings-empty-title">No settings declared</p>
                  <p className="ext-settings-empty-body">
                    This package declares no settings, so there is nothing to change.
                    {view.qualities.length > 0
                      ? " Its quality options still apply."
                      : " It also declares no quality options."}
                  </p>
                </div>
              ) : (
                <div className="ext-settings-fields">
                  {view.settings.map((descriptor) => (
                    <ExtensionSettingField
                      key={descriptor.key}
                      descriptor={descriptor}
                      value={fieldValue(descriptor)}
                      overriddenBy={
                        overrideValues.has(descriptor.key)
                          ? (activeQuality?.label ?? null)
                          : null
                      }
                      onChange={changeSetting}
                      onInvoke={invokeAction}
                    />
                  ))}
                </div>
              )}
            </fieldset>
          </div>
        )}
      </div>

      {confirmingLeave ? (
        <div className="ext-settings-confirm" role="group" aria-labelledby={`${settingsHintId}-leave`}>
          <p className="ext-settings-confirm-text" id={`${settingsHintId}-leave`}>
            You have unsaved changes.
            <span>Leaving now discards them; the package keeps its last saved values.</span>
          </p>
          <p className="sr-only" role="status">
            Unsaved changes. Keep editing, or discard and leave.
          </p>
          <div className="ext-settings-confirm-actions">
            <button type="button" className="button button-danger" onClick={() => onClose()}>
              Discard and leave
            </button>
            <button
              type="button"
              className="button button-primary"
              ref={keepEditingRef}
              onClick={keepEditing}
            >
              Keep editing
            </button>
          </div>
        </div>
      ) : null}

      <footer className="ext-settings-foot">
        {invalidSettings.length > 0 ? (
          <p className="ext-settings-foot-hint">
            Save needs a number for{" "}
            {invalidSettings.map((descriptor) => descriptor.label).join(", ")}.
          </p>
        ) : null}

        <p
          className="ext-settings-save-status"
          role={saveMessage?.tone === "error" ? "alert" : "status"}
          data-tone={saveMessage?.tone}
        >
          {saveStatus}
        </p>

        <div className="ext-settings-foot-actions">
          <button
            type="button"
            className="button button-primary"
            ref={saveButtonRef}
            disabled={!canSave}
            aria-busy={saving}
            onClick={() => void saveSettings()}
          >
            {saving ? "Saving..." : "Save changes"}
          </button>
          <button
            type="button"
            className="button button-quiet"
            aria-label={`Close ${displayName} settings`}
            onClick={requestLeave}
          >
            Close
          </button>
        </div>
      </footer>
    </section>
  );
}

/**
 * The keys the selected quality actually changes. A quality that repeats a saved value is
 * not overriding anything a reader could see, so it is not marked.
 */
function qualityOverrides(
  quality: QualityDescriptor | null,
  view: ExtensionSettingsView,
): Map<string, unknown> {
  const overrides = new Map<string, unknown>();
  if (quality === null) {
    return overrides;
  }

  Object.keys(quality.settings).forEach((key) => {
    const declared = view.settings.some(
      (descriptor) => descriptor.key === key && descriptor.type !== "button",
    );
    if (!declared) return;
    if (sameSettingValue(quality.settings[key], view.savedValues[key])) return;
    overrides.set(key, quality.settings[key]);
  });

  return overrides;
}
