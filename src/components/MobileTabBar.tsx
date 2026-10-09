/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

// Bottom tabs on the mobile layout, so the places people go most are one tap
// away instead of behind the drawer. Hidden on desktop by the stylesheet.

import type { ReactNode } from "react";
import { BiHomeAlt2, BiLibrary, BiSearch, BiSolidHome } from "react-icons/bi";

export type MobileTab = "home" | "search" | "library";

interface MobileTabBarProps {
  active: MobileTab | null;
  onHome: () => void;
  onSearch: () => void;
  onLibrary: () => void;
}

function Tab({
  label,
  icon,
  activeIcon,
  active,
  onClick,
}: {
  label: string;
  icon: ReactNode;
  activeIcon?: ReactNode;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      className={`mobile-tab${active ? " active" : ""}`}
      onClick={onClick}
      type="button"
      aria-current={active ? "page" : undefined}
    >
      <span className="mobile-tab-icon" aria-hidden>
        {active && activeIcon ? activeIcon : icon}
      </span>
      <span className="mobile-tab-label">{label}</span>
    </button>
  );
}

export default function MobileTabBar({
  active,
  onHome,
  onSearch,
  onLibrary,
}: MobileTabBarProps) {
  return (
    <nav className="mobile-tabbar" aria-label="Main">
      <Tab
        label="Home"
        icon={<BiHomeAlt2 />}
        activeIcon={<BiSolidHome />}
        active={active === "home"}
        onClick={onHome}
      />
      <Tab
        label="Search"
        icon={<BiSearch />}
        active={active === "search"}
        onClick={onSearch}
      />
      <Tab
        label="Library"
        icon={<BiLibrary />}
        active={active === "library"}
        onClick={onLibrary}
      />
    </nav>
  );
}
