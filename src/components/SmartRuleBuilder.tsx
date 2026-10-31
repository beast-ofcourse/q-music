import { useState } from "react";

import { Icon } from "./Icon";
import type { SmartField, SmartOp, SmartRule, SmartRules } from "../lib/types";

interface FieldSpec {
  value: SmartField;
  label: string;
  ops: SmartOp[];
  placeholder: string;
  /** Fixed wording for a field that has no meaningful spectrum. */
  hint?: string;
}

/** Every field/op pair here is one the core actually builds a clause for. */
const ARTIST_FIELD: FieldSpec = {
  value: "artist",
  label: "Artist",
  ops: ["is", "is-not", "contains"],
  placeholder: "Kaito Mori",
};

const FIELDS: FieldSpec[] = [
  ARTIST_FIELD,
  { value: "album", label: "Album", ops: ["is", "is-not", "contains"], placeholder: "Harbour" },
  { value: "genre", label: "Genre", ops: ["is", "is-not", "contains"], placeholder: "Jazz" },
  { value: "title", label: "Title", ops: ["contains"], placeholder: "harbour" },
  { value: "format", label: "Format", ops: ["is", "is-not"], placeholder: "FLAC" },
  { value: "year", label: "Year", ops: ["is", "is-not", "greater-than", "less-than"], placeholder: "2015" },
  { value: "play-count", label: "Play count", ops: ["greater-than", "less-than"], placeholder: "3" },
  {
    value: "duration-seconds",
    label: "Duration (seconds)",
    ops: ["greater-than", "less-than"],
    placeholder: "180",
  },
  {
    value: "added-days",
    label: "Days since added",
    ops: ["less-than", "greater-than", "in-last"],
    placeholder: "30",
  },
  {
    value: "favorite",
    label: "Favourite",
    ops: ["is"],
    placeholder: "true",
    hint: "true or false",
  },
];

const OP_LABELS: Record<SmartOp, string> = {
  is: "is",
  "is-not": "is not",
  contains: "contains",
  "greater-than": "is greater than",
  "less-than": "is less than",
  "in-last": "in the last",
};

const DEFAULT_LIMIT = 200;

function specFor(field: SmartField): FieldSpec {
  return FIELDS.find((candidate) => candidate.value === field) ?? ARTIST_FIELD;
}

function emptyRule(): SmartRule {
  return { field: "genre", op: "is", value: "" };
}

interface SmartRuleBuilderProps {
  initial?: SmartRules | null;
  submitLabel: string;
  onSubmit: (rules: SmartRules) => void;
  onCancel?: () => void;
}

export function SmartRuleBuilder({
  initial,
  submitLabel,
  onSubmit,
  onCancel,
}: SmartRuleBuilderProps) {
  const [rules, setRules] = useState<SmartRule[]>(initial?.rules.length ? initial.rules : [emptyRule()]);
  const [matchAny, setMatchAny] = useState(initial?.matchAny ?? false);
  const [limit, setLimit] = useState(initial?.limit ?? DEFAULT_LIMIT);

  const usable = rules.filter((rule) => rule.value.trim().length > 0);

  function updateRule(index: number, patch: Partial<SmartRule>) {
    setRules((previous) =>
      previous.map((rule, position) => (position === index ? { ...rule, ...patch } : rule)),
    );
  }

  function changeField(index: number, field: SmartField) {
    const spec = specFor(field);
    const op = spec.ops[0] ?? "is";
    updateRule(index, { field, op });
  }

  return (
    <div className="rule-editor">
      <div className="field-row">
        <label className="field">
          <span className="field-label">Match</span>
          <select
            className="select"
            value={matchAny ? "any" : "all"}
            onChange={(event) => setMatchAny(event.target.value === "any")}
          >
            <option value="all">Every rule</option>
            <option value="any">Any rule</option>
          </select>
        </label>
        <label className="field">
          <span className="field-label">Limit</span>
          <input
            className="input"
            type="number"
            min={1}
            max={2000}
            value={limit}
            onChange={(event) => {
              const next = Number(event.target.value);
              setLimit(Number.isFinite(next) ? Math.min(2000, Math.max(1, next)) : DEFAULT_LIMIT);
            }}
          />
        </label>
      </div>

      <div className="rule-list">
        {rules.map((rule, index) => {
          const spec = specFor(rule.field);
          return (
            <div className="rule-row" key={index}>
              <select
                className="select"
                aria-label="Rule field"
                value={rule.field}
                onChange={(event) => changeField(index, event.target.value as SmartField)}
              >
                {FIELDS.map((field) => (
                  <option key={field.value} value={field.value}>
                    {field.label}
                  </option>
                ))}
              </select>

              <select
                className="select"
                aria-label="Rule comparison"
                value={rule.op}
                onChange={(event) => updateRule(index, { op: event.target.value as SmartOp })}
              >
                {spec.ops.map((op) => (
                  <option key={op} value={op}>
                    {OP_LABELS[op]}
                  </option>
                ))}
              </select>

              <input
                className="input"
                aria-label="Rule value"
                value={rule.value}
                placeholder={spec.hint ?? spec.placeholder}
                onChange={(event) => updateRule(index, { value: event.target.value })}
              />

              <button
                type="button"
                className="tbtn"
                aria-label="Remove this rule"
                disabled={rules.length === 1}
                onClick={() =>
                  setRules((previous) => previous.filter((_, position) => position !== index))
                }
              >
                <Icon name="close" size={13} />
              </button>
            </div>
          );
        })}
      </div>

      <div className="actions">
        <button
          type="button"
          className="button button-quiet"
          onClick={() => setRules((previous) => [...previous, emptyRule()])}
        >
          <Icon name="plus" size={12} />
          Add rule
        </button>

        <button
          type="button"
          className="button button-primary"
          disabled={usable.length === 0}
          onClick={() =>
            onSubmit({
              matchAny,
              rules: usable.map((rule) => ({ ...rule, value: rule.value.trim() })),
              limit,
            })
          }
        >
          {submitLabel}
        </button>

        {onCancel ? (
          <button type="button" className="button button-quiet" onClick={onCancel}>
            Cancel
          </button>
        ) : null}

        {usable.length === 0 ? (
          <span className="hint">A rule needs a value before it can match anything.</span>
        ) : null}
      </div>
    </div>
  );
}
