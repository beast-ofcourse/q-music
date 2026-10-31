import type { KeyboardEvent } from "react";

import { Icon } from "../Icon";
import type { ProviderView } from "../../lib/extensions";
import { healthDetail, healthLabel, qualityLabel } from "./provider-health";

interface ProviderRowProps {
  provider: ProviderView;
  position: number;
  total: number;
  busy: boolean;
  notice: string | undefined;
  onMove: (direction: -1 | 1) => void;
  onRetry: () => void;
  onHomeFeed?: () => void;
  onBrowse?: () => void;
  register: (element: HTMLLIElement | null) => void;
}

/**
 * One provider. The row itself is the focus target rather than the buttons: after a move
 * the provider that moves to the top has its "up" button disabled, and a disabled button
 * cannot hold focus. The row never becomes unavailable, so focus survives the reorder.
 */
export function ProviderRow({
  provider,
  position,
  total,
  busy,
  notice,
  onMove,
  onRetry,
  onHomeFeed,
  onBrowse,
  register,
}: ProviderRowProps) {
  const canMoveUp = position > 1 && !busy;
  const canMoveDown = position < total && !busy;
  const detail = healthDetail(provider.health);

  function handleKeyDown(event: KeyboardEvent<HTMLLIElement>) {
    if (!event.altKey) {
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      if (canMoveUp) {
        onMove(-1);
      }
      return;
    }
    if (event.key === "ArrowDown") {
      event.preventDefault();
      if (canMoveDown) {
        onMove(1);
      }
    }
  }

  return (
    <li
      className="prov-row"
      data-provider-id={provider.id}
      ref={register}
      tabIndex={0}
      aria-labelledby={`prov-name-${provider.id}`}
      onKeyDown={handleKeyDown}
    >
      <span className="prov-position" aria-hidden="true">
        {total === 1 ? "only" : `${position} of ${total}`}
      </span>

      <span className="prov-ident">
        <span className="prov-name" id={`prov-name-${provider.id}`}>
          {provider.displayName}
        </span>
        <span className="prov-id">
          {provider.id} / {provider.version}
        </span>
        {provider.missingFeatures.length > 0 ? (
          <span className="prov-missing">
            Needs {provider.missingFeatures.join(", ")}, which this host does not implement.
          </span>
        ) : null}
        {detail !== null ? <span className="prov-health-detail">{detail}</span> : null}
      </span>

      <span className="prov-quality">{qualityLabel(provider)}</span>

      <span className="prov-health" data-state={provider.health.state}>
        {healthLabel(provider.health)}
      </span>

      <span className="prov-actions">
        <button
          type="button"
          className="prov-action"
          disabled={!canMoveUp}
          onClick={() => onMove(-1)}
          aria-label={`Move ${provider.displayName} up`}
          title="Move up (Alt+Up)"
        >
          <Icon name="arrow-up" size={12} />
        </button>
        <button
          type="button"
          className="prov-action"
          disabled={!canMoveDown}
          onClick={() => onMove(1)}
          aria-label={`Move ${provider.displayName} down`}
          title="Move down (Alt+Down)"
        >
          <Icon name="arrow-down" size={12} />
        </button>
        {provider.health.state === "healthy" ? null : (
          <button type="button" className="prov-action prov-retry" disabled={busy} onClick={onRetry}>
            {busy ? "Checking..." : "Retry"}
          </button>
        )}
        {onHomeFeed ? (
          <button type="button" className="prov-action" onClick={onHomeFeed} aria-label={`Open home feed for ${provider.displayName}`}>
            Feed
          </button>
        ) : null}
        {onBrowse ? (
          <button type="button" className="prov-action" onClick={onBrowse} aria-label={`Browse categories for ${provider.displayName}`}>
            Browse
          </button>
        ) : null}
      </span>

      {notice === undefined ? null : (
        <p className="prov-notice" role="alert">
          {notice}
        </p>
      )}
    </li>
  );
}
