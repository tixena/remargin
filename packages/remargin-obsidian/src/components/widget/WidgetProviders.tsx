import { createElement, type ReactNode } from "react";
import { BackendContext } from "@/hooks/useBackend";
import { PluginContext } from "@/hooks/usePlugin";
import { PortalContainerContext } from "@/hooks/usePortalContainer";
import type RemarginPlugin from "@/main";

export interface WidgetProvidersProps {
  plugin: RemarginPlugin;
  /**
   * Element that should host Radix portals for this widget's children
   * (tooltips, popovers). Pass the host element the React root is
   * mounted into so Tailwind's `important: ".remargin-container"` scope
   * (or whichever ancestor carries it) still applies. When the host
   * does not sit inside a `.remargin-container` ancestor, pass the
   * widget's own host element — better than `document.body` for class
   * scoping.
   */
  portalContainer: HTMLElement;
  /**
   * Optional in the props object so callers can pass children as the
   * third `createElement` argument (the canonical React pattern) without
   * TypeScript demanding a duplicate `children` field on the props
   * literal. The component still requires children at runtime — passing
   * none renders nothing useful, but never throws.
   */
  children?: ReactNode;
}

/**
 * Wraps a widget's React subtree in the same provider stack the
 * sidebar's own mount uses. Required for any tree that calls
 * `useBackend()`, `usePlugin()`, or `usePortalContainer()` —
 * which `WidgetCommentView` does transitively (CommentHeader →
 * useParticipants → useBackend + usePlugin; Tooltip → usePortalContainer).
 *
 * Without this wrapper, the editor-side mounts (reading-mode and CM6)
 * crash on first render with `useBackend must be used within a
 * BackendContext.Provider` — see ticket rem-ob35.
 */
export function WidgetProviders({ plugin, portalContainer, children }: WidgetProvidersProps) {
  return createElement(
    BackendContext.Provider,
    { value: plugin.backend },
    createElement(
      PluginContext.Provider,
      { value: plugin },
      createElement(PortalContainerContext.Provider, { value: portalContainer }, children)
    )
  );
}
