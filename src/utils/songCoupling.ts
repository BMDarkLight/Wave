/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { createContext } from "react";
import type { SongCouple } from "./player";

/** Coupled songs while song coupling is on, and none while it is off, so
 * every song list can mark them without passing them down by hand. */
export const SongCouplingContext = createContext<SongCouple[]>([]);
