import { useEffect, useState } from "react";
import { resolveCoverSrc, type Track } from "../utils/player";
import { getTrackTitle } from "../utils/track";

export default function Artwork({
  track,
  fallback,
  className,
  overrideSrc,
}: {
  track?: Track | null;
  fallback: string;
  className: string;
  /** Optional full-resolution cover (lyrics panel). */
  overrideSrc?: string | null;
}) {
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const raw = overrideSrc || track?.cover_art_data_url || null;
    void resolveCoverSrc(raw).then((resolved) => {
      if (!cancelled) setSrc(resolved);
    });
    return () => {
      cancelled = true;
    };
  }, [track?.cover_art_data_url, overrideSrc]);

  if (src) {
    return (
      <img
        className={className}
        src={src}
        alt={`${getTrackTitle(track)} cover`}
        draggable={false}
      />
    );
  }

  return <div className={className}>{fallback}</div>;
}
