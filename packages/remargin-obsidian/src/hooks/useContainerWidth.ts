/** Hook that tracks an element's width. */

import type { RefObject } from "react";
import { useEffect, useState } from "react";

/**
 * Tracks the content-box width of a DOM element via `ResizeObserver`.
 *
 * Returns `0` until the first observation fires (which is typically
 * immediate after mount). The hook cleans up the observer on unmount.
 */
export function useContainerWidth(ref: RefObject<HTMLElement | null>): number {
  const [width, setWidth] = useState(0);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;

    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const inlineSize = entry.contentBoxSize?.[0]?.inlineSize;
        if (inlineSize !== undefined) {
          setWidth(inlineSize);
        } else {
          setWidth(entry.contentRect.width);
        }
      }
    });

    observer.observe(el);
    return () => observer.disconnect();
  }, [ref]);

  return width;
}
