import { useArtwork } from "../lib/artwork";
import { formatSpan } from "../lib/format";
import type { TrackGroup } from "../lib/types";
import { Icon } from "../components/Icon";
import { useStore } from "../state/store";

function GroupRow({ group, open }: { group: TrackGroup; open: () => void }) {
  const artwork = useArtwork(group.artId);
  // Artists carry no subtitle, so the column would be eight em dashes; drop it instead.
  const hasSubtitle = Boolean(group.subtitle);
  return (
    <button type="button" className="group-row" data-sub={hasSubtitle} onClick={open}>
      <span className="group-art">
        {artwork ? <img src={artwork} alt="" /> : <Icon name="disc" size={14} />}
      </span>
      <span className="group-name">{group.name}</span>
      {hasSubtitle ? <span className="group-sub">{group.subtitle}</span> : null}
      <span className="cell-num">{formatSpan(group.durationMs)}</span>
      <span className="cell-num">{group.trackCount}</span>
    </button>
  );
}

/** Albums and artists, grouped and totalled by the core rather than in the window. */
export function BrowseView() {
  const { view, groups, groupsLoading, setView } = useStore();
  if (view.kind !== "albums" && view.kind !== "artists") return null;

  const albums = view.kind === "albums";
  const title = albums ? "Albums" : "Artists";
  const noun = albums ? "album" : "artist";

  return (
    <>
      <div className="view-head">
        <h1 className="view-title">{title}</h1>
        <div className="view-tools">
          <span className="view-meta">
            {groupsLoading
              ? "reading…"
              : `${groups.length.toLocaleString()} ${noun}${groups.length === 1 ? "" : "s"}`}
          </span>
        </div>
      </div>

      <div className="view-body">
        {groupsLoading ? (
          <div aria-hidden="true">
            {Array.from({ length: 8 }, (_, index) => (
              <div key={index} className="skeleton-row" />
            ))}
          </div>
        ) : groups.length === 0 ? (
          <div className="empty">
            <p className="empty-title">No {noun}s indexed</p>
            <p className="empty-body">
              Grouping reads the tags on each file. If a folder has no {noun} tags yet, the
              library still lists its tracks.
            </p>
          </div>
        ) : (
          <div role="list" aria-label={title}>
            {groups.map((group) => (
              <div role="listitem" key={group.name}>
                <GroupRow
                  group={group}
                  open={() =>
                    setView(
                      albums
                        ? { kind: "album", name: group.name }
                        : { kind: "artist", name: group.name },
                    )
                  }
                />
              </div>
            ))}
          </div>
        )}
      </div>
    </>
  );
}
