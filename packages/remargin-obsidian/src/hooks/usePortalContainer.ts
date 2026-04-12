import { createContext, useContext } from "react";

/**
 * Holds a reference to the `.remargin-container` DOM element so that Radix UI
 * portals render **inside** it rather than at `document.body`.
 *
 * Without this, the `important: ".remargin-container"` rule in
 * `tailwind.config.ts` scopes every Tailwind rule under that ancestor — but
 * portals mount outside it, so none of the utility classes apply.
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
