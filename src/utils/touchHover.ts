/**
 * Android / touch WebViews often keep :hover (and button :focus) stuck after a
 * tap until the next tap elsewhere. Desktop Chrome can also lie about
 * `(hover: none)`, so we mark the document from UA/host signals and clear
 * leftover focus after pointer activation.
 */

const NO_HOVER_CLASS = "wave-no-hover";

function isAndroidUserAgent(): boolean {
  return /Android/i.test(navigator.userAgent || "");
}

function mediaPrefersNoHover(): boolean {
  return window.matchMedia("(hover: none), (pointer: coarse)").matches;
}

export function enableNoHoverMode(): void {
  document.documentElement.classList.add(NO_HOVER_CLASS);
}

export function shouldEnableNoHoverMode(): boolean {
  return isAndroidUserAgent() || mediaPrefersNoHover();
}

/** Call once at startup (before paint if possible). */
export function initTouchHoverGuards(): void {
  if (shouldEnableNoHoverMode()) {
    enableNoHoverMode();
  }

  const media = window.matchMedia("(hover: none), (pointer: coarse)");
  const onMediaChange = () => {
    if (media.matches || isAndroidUserAgent()) {
      enableNoHoverMode();
    } else if (!isAndroidUserAgent()) {
      document.documentElement.classList.remove(NO_HOVER_CLASS);
    }
  };
  media.addEventListener("change", onMediaChange);

  const clearStickyFocus = (event: Event) => {
    if (!document.documentElement.classList.contains(NO_HOVER_CLASS)) return;
    const target = event.target;
    if (!(target instanceof Element)) return;
    if (target.closest("input, textarea, select, [contenteditable='true']")) {
      return;
    }
    const active = document.activeElement;
    if (
      !(active instanceof HTMLElement) ||
      active === document.body ||
      active.matches("input, textarea, select, [contenteditable='true']")
    ) {
      return;
    }
    // After the activating click so we don't cancel the tap's default action.
    queueMicrotask(() => {
      if (document.activeElement === active) {
        active.blur();
      }
    });
  };

  // `click` covers touch + mouse synthesized activation in the WebView.
  document.addEventListener("click", clearStickyFocus, true);
}
