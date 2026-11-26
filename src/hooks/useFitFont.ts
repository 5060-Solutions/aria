import { useLayoutEffect, type RefObject } from "react";

/**
 * Sets the element's font size to the largest in [minPx, maxPx] at which
 * `text` fits on one line, and re-fits when the element's width changes.
 * Below the minimum the text keeps the minimum and the element scrolls
 * sideways (set `overflow-x: auto` on it): a number is never ellipsized.
 */
export function useFitFont(ref: RefObject<HTMLElement | null>, text: string, maxPx: number, minPx: number): void {
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const fit = () => {
      let size = maxPx;
      el.style.fontSize = `${size}px`;
      while (size > minPx && el.scrollWidth > el.clientWidth) {
        size -= 1;
        el.style.fontSize = `${size}px`;
      }
      // Keep the end of a number that still overflows (the digits just typed) in view.
      el.scrollLeft = el.scrollWidth;
    };
    fit();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(fit);
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref, text, maxPx, minPx]);
}
