import { useStore } from "../state/store";
import "../styles/home-feed.css";

export function ResolvedView() {
  const { view } = useStore();
  if (view.kind !== "resolved") return null;

  const { provider, entityType, item } = view;
  const title = item?.title ?? item?.name ?? "Resolved item";
  const subtitle = Array.isArray(item?.artists)
    ? item.artists.join(", ")
    : item?.artist ?? item?.artists ?? item?.album_name ?? "";
  const art =
    Array.isArray(item?.images) && item.images.length > 0
      ? item.images[0]
      : item?.artUrl ?? null;

  return (
    <div className="resolved-view">
      <div className="view-head">
        <h1 className="view-title">Resolved link</h1>
        <div className="view-tools">
          <span className="view-meta">{provider}</span>
        </div>
      </div>
      <div className="view-body">
        <div className="resolved-card">
          <div className="resolved-art">
            {art ? <img src={art} alt="" /> : <span className="resolved-art-fallback" aria-hidden="true" />}
          </div>
          <div className="resolved-info">
            <p className="resolved-type">{entityType}</p>
            <h2 className="resolved-title">{title}</h2>
            <p className="resolved-subtitle">{subtitle}</p>
            <pre className="resolved-raw">{JSON.stringify(item, null, 2)}</pre>
          </div>
        </div>
      </div>
    </div>
  );
}
