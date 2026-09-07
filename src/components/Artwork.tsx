/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useState } from "react";
import { resolveCoverSrc, type Track } from "../utils/player";
import { getTrackTitle } from "../utils/track";

export default function Artwork({
  track,
  fallback,
  className,
  overrideSrc,
  alt,
}: {
  track?: Track | null;
  fallback: string;
  className: string;
  /** Optional full-resolution cover (lyrics panel). */
  overrideSrc?: string | null;
  /** Alt text for covers that aren't a track's — an album hit, say. */
  alt?: string;
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
        alt={alt ?? `${getTrackTitle(track)} cover`}
        draggable={false}
      />
    );
  }

  return <div className={className}>{fallback}</div>;
}
