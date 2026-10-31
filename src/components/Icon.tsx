/**
 * The window's icon set. Every glyph is authored here at a 24×24 grid with a 1.5
 * stroke so weight and optical size stay consistent; nothing relies on an emoji or a
 * platform glyph, and filled shapes are only used where a stroked version would read
 * as a wireframe at 14px (transport, favourites).
 */

export type IconName =
  | "play"
  | "pause"
  | "previous"
  | "next"
  | "stop"
  | "search"
  | "close"
  | "heart"
  | "shuffle"
  | "repeat"
  | "repeat-one"
  | "volume"
  | "mute"
  | "equaliser"
  | "lyrics"
  | "queue"
  | "folder"
  | "disc"
  | "artist"
  | "library"
  | "home"
  | "clock"
  | "settings"
  | "puzzle"
  | "plus"
  | "trash"
  | "edit"
  | "check"
  | "chevron-left"
  | "chevron-right"
  | "arrow-up"
  | "arrow-down"
  | "more";

const STROKE = {
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.5,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
};

const SOLID = { fill: "currentColor", stroke: "none" };

function shapes(name: IconName) {
  switch (name) {
    case "play":
      return <path d="M8.5 5.6 18.4 12 8.5 18.4Z" {...SOLID} />;
    case "pause":
      return (
        <>
          <rect x="8" y="5.5" width="2.6" height="13" rx="0.6" {...SOLID} />
          <rect x="13.4" y="5.5" width="2.6" height="13" rx="0.6" {...SOLID} />
        </>
      );
    case "previous":
      return (
        <>
          <path d="M18.5 6.2 10 12l8.5 5.8Z" {...SOLID} />
          <rect x="6" y="6" width="1.9" height="12" rx="0.6" {...SOLID} />
        </>
      );
    case "next":
      return (
        <>
          <path d="M5.5 6.2 14 12l-8.5 5.8Z" {...SOLID} />
          <rect x="16.1" y="6" width="1.9" height="12" rx="0.6" {...SOLID} />
        </>
      );
    case "stop":
      return <rect x="7" y="7" width="10" height="10" rx="1" {...SOLID} />;
    case "search":
      return (
        <>
          <circle cx="10.8" cy="10.8" r="6.3" {...STROKE} />
          <path d="m15.6 15.6 4 4" {...STROKE} />
        </>
      );
    case "close":
      return <path d="m6.5 6.5 11 11m0-11-11 11" {...STROKE} />;
    case "heart":
      return (
        <path
          d="M12 19.2 4.9 12.4a4.2 4.2 0 0 1 5.9-6l1.2 1.1 1.2-1.1a4.2 4.2 0 0 1 5.9 6Z"
          {...SOLID}
        />
      );
    case "shuffle":
      return (
        <>
          <path d="M4 7h3.2c1.2 0 2.3.6 3 1.6l3.6 6.8c.7 1 1.8 1.6 3 1.6H20" {...STROKE} />
          <path d="M4 17h3.2c1.2 0 2.3-.6 3-1.6l.9-1.7" {...STROKE} />
          <path d="m15.2 7.4.9-.4H20" {...STROKE} />
          <path d="m17.6 5 2.4 2-2.4 2M17.6 15l2.4 2-2.4 2" {...STROKE} />
        </>
      );
    case "repeat":
      return (
        <>
          <path d="M6.5 8.5h9.8a2 2 0 0 1 2 2v1.2" {...STROKE} />
          <path d="M17.5 15.5H7.7a2 2 0 0 1-2-2v-1.2" {...STROKE} />
          <path d="m14.4 6.3 2.2 2.2-2.2 2.2M9.6 17.7 7.4 15.5l2.2-2.2" {...STROKE} />
        </>
      );
    case "repeat-one":
      return (
        <>
          <path d="M6.5 8.5h9.8a2 2 0 0 1 2 2v1.2" {...STROKE} />
          <path d="M17.5 15.5H7.7a2 2 0 0 1-2-2v-1.2" {...STROKE} />
          <path d="m14.4 6.3 2.2 2.2-2.2 2.2M9.6 17.7 7.4 15.5l2.2-2.2" {...STROKE} />
          <path d="M12 10.4v3.4" {...STROKE} />
        </>
      );
    case "volume":
      return (
        <>
          <path d="M5 10h2.6L12 6.6v10.8L7.6 14H5Z" {...STROKE} />
          <path d="M15.2 9.2a4 4 0 0 1 0 5.6M17.6 7a7.4 7.4 0 0 1 0 10" {...STROKE} />
        </>
      );
    case "mute":
      return (
        <>
          <path d="M5 10h2.6L12 6.6v10.8L7.6 14H5Z" {...STROKE} />
          <path d="m15.4 9.8 4.4 4.4m0-4.4-4.4 4.4" {...STROKE} />
        </>
      );
    case "equaliser":
      return (
        <>
          <path d="M7 4.5v15M12 4.5v15M17 4.5v15" {...STROKE} />
          <rect x="5" y="9" width="4" height="2.6" rx="0.7" {...SOLID} />
          <rect x="10" y="14" width="4" height="2.6" rx="0.7" {...SOLID} />
          <rect x="15" y="7" width="4" height="2.6" rx="0.7" {...SOLID} />
        </>
      );
    case "lyrics":
      return (
        <>
          <path d="M4.5 7h11M4.5 11.5h8M4.5 16h6" {...STROKE} />
          <path d="M18.4 8.2v7.1a2.2 2.2 0 1 1-1.4-2V6.4l1.4 1.8Z" {...STROKE} />
        </>
      );
    case "queue":
      return (
        <>
          <path d="M4.5 7h11M4.5 12h11M4.5 17h7" {...STROKE} />
          <path d="M19 8.4v6.8" {...STROKE} />
        </>
      );
    case "folder":
      return (
        <path
          d="M4 7.6a1.6 1.6 0 0 1 1.6-1.6h3.3l1.7 2h7.8A1.6 1.6 0 0 1 20 9.6v6.8a1.6 1.6 0 0 1-1.6 1.6H5.6A1.6 1.6 0 0 1 4 16.4Z"
          {...STROKE}
        />
      );
    case "disc":
      return (
        <>
          <circle cx="12" cy="12" r="7.6" {...STROKE} />
          <circle cx="12" cy="12" r="2.1" {...STROKE} />
        </>
      );
    case "artist":
      return (
        <>
          <circle cx="12" cy="8.6" r="3.5" {...STROKE} />
          <path d="M5.2 19.2a7.2 7.2 0 0 1 13.6 0" {...STROKE} />
        </>
      );
    case "library":
      return (
        <>
          <path d="M4.5 10v4M8 6.5v11M11.5 9v6M15 5v14M18.5 10.5v3" {...STROKE} />
        </>
      );
    case "clock":
      return (
        <>
          <circle cx="12" cy="12" r="7.6" {...STROKE} />
          <path d="M12 7.8V12l3 1.9" {...STROKE} />
        </>
      );
    case "settings":
      return (
        <>
          <path d="M4.5 8h15M4.5 16h15" {...STROKE} />
          <circle cx="9.5" cy="8" r="2.3" {...STROKE} />
          <circle cx="15" cy="16" r="2.3" {...STROKE} />
        </>
      );
    case "puzzle":
      return (
        <path
          d="M5.6 7.4h3.9a2.5 2.5 0 0 1 5 0h3.9a.6.6 0 0 1 .6.6v3.9a2.5 2.5 0 0 1 0 5v.9a.6.6 0 0 1-.6.6H5.6a.6.6 0 0 1-.6-.6V8a.6.6 0 0 1 .6-.6Z"
          {...STROKE}
        />
      );
    case "plus":
      return <path d="M12 5.5v13M5.5 12h13" {...STROKE} />;
    case "trash":
      return (
        <>
          <path d="M5.5 7.5h13M9.5 7.5V5.6h5v1.9M7 7.5l.8 11h8.4l.8-11" {...STROKE} />
          <path d="M10.5 10.5v5M13.5 10.5v5" {...STROKE} />
        </>
      );
    case "edit":
      return (
        <>
          <path d="M15.6 5.6 18.4 8.4 9.8 17H7v-2.8Z" {...STROKE} />
          <path d="m13.6 7.6 2.8 2.8" {...STROKE} />
        </>
      );
    case "check":
      return <path d="m5.8 12.6 4 4L18.2 8" {...STROKE} />;
    case "chevron-left":
      return <path d="m14.5 6-6 6 6 6" {...STROKE} />;
    case "chevron-right":
      return <path d="m9.5 6 6 6-6 6" {...STROKE} />;
    case "arrow-up":
      return <path d="M12 19V6m-5 5 5-5 5 5" {...STROKE} />;
    case "arrow-down":
      return <path d="M12 5v13m-5-5 5 5 5-5" {...STROKE} />;
    case "home":
      return (
        <>
          <path d="M4 11.5 12 5l8 6.5" {...STROKE} />
          <path d="M6.5 10.5V19h11V10.5" {...STROKE} />
        </>
      );
    case "more":
      return (
        <>
          <circle cx="6" cy="12" r="1.3" {...SOLID} />
          <circle cx="12" cy="12" r="1.3" {...SOLID} />
          <circle cx="18" cy="12" r="1.3" {...SOLID} />
        </>
      );
    default:
      return null;
  }
}

interface IconProps {
  name: IconName;
  size?: number;
  className?: string;
}

export function Icon({ name, size = 16, className }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      className={className}
      aria-hidden="true"
      focusable="false"
    >
      {shapes(name)}
    </svg>
  );
}
