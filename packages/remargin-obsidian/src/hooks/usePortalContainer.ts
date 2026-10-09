/** React context and hook for the element that hosts Radix portals. */

import { createContext, useContext } from "react";

/**
 * The `.remargin-container` element, so Radix portals render inside it: Tailwind's rules are
 * scoped under that ancestor and would not reach a portal mounted at `document.body`.
 */
export const PortalContainerContext = createContext<HTMLElement | null>(null);

/**
 * Returns the portal container element, or `undefined` when unavailable.
 * Radix `Portal` components accept `container?: HTMLElement` — passing
 * `undefined` falls back to `document.body`.
 */
export function usePortalContainer(): HTMLElement | undefined {
  const el = useContext(PortalContainerContext);
  return el ?? undefined;
}
