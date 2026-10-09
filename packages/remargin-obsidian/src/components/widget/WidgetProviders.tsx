/** The provider stack around an editor-side widget's React tree. */

import { createElement, type ReactNode } from "react";
import { BackendContext } from "@/hooks/useBackend";
import { PluginContext } from "@/hooks/usePlugin";
import { PortalContainerContext } from "@/hooks/usePortalContainer";
import type RemarginPlugin from "@/main";

/** Props for {@link WidgetProviders}. */
export interface WidgetProvidersProps {
  plugin: RemarginPlugin;
  /**
   * Hosts Radix portals for the widget's children. Pass the element the React root is mounted
   * into so the `.remargin-container` class scoping still applies.
   */
  portalContainer: HTMLElement;
  /** Optional so callers can pass children as the third `createElement` argument. */
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
 * BackendContext.Provider`.
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
