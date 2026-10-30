/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import type { ListeningStats } from "../utils/player";
import { listeningInsights } from "../utils/insights";

/** The insights as cards; Home shows the first few of the same. */
export default function InsightGrid({
  stats,
  limit,
}: {
  stats: ListeningStats;
  limit?: number;
}) {
  const insights = listeningInsights(stats).slice(0, limit);
  if (insights.length === 0) return null;
  return (
    <ul className="stats-insights">
      {insights.map((insight) => (
        <li key={insight.id} className="stats-insight">
          <span className="stats-insight-value">{insight.value}</span>
          <span className="stats-insight-text">{insight.text}</span>
        </li>
      ))}
    </ul>
  );
}
