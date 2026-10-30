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
import { BiChevronRight } from "react-icons/bi";
import {
  formatListenDuration,
  getListeningStats,
  type ListeningStats,
} from "../utils/player";
import { hasListening } from "../utils/insights";
import InsightGrid from "./InsightGrid";

const count = new Intl.NumberFormat();

/** The end of Home: a few lines about how you listen, and the way to more. */
export default function HomeListening({
  refreshKey,
  onOpenStats,
}: {
  /** Bumped after a library refresh. */
  refreshKey: number;
  onOpenStats: () => void;
}) {
  const [stats, setStats] = useState<ListeningStats | null>(null);

  useEffect(() => {
    let cancelled = false;
    getListeningStats(3)
      .then((next) => {
        if (!cancelled) setStats(next);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [refreshKey]);

  if (!stats || !hasListening(stats)) return null;

  return (
    <section
      className="home-section home-listening"
      aria-label="Your listening"
    >
      <div className="home-section-head home-listening-head">
        <div>
          <h3>Your listening</h3>
          <p>
            {formatListenDuration(stats.total_listen_seconds)} over{" "}
            {count.format(stats.total_plays)}{" "}
            {stats.total_plays === 1 ? "play" : "plays"}
          </p>
        </div>
        <button
          className="btn-ghost btn-sm home-listening-more"
          type="button"
          onClick={onOpenStats}
        >
          All stats <BiChevronRight aria-hidden />
        </button>
      </div>
      <InsightGrid stats={stats} limit={3} />
    </section>
  );
}
