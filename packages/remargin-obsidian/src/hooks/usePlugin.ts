import { createContext, useContext } from "react";
import type RemarginPlugin from "@/main";

export const PluginContext = createContext<RemarginPlugin | null>(null);

export function usePlugin(): RemarginPlugin {
  const plugin = useContext(PluginContext);
  if (!plugin) {
    throw new Error("usePlugin must be used within a PluginContext.Provider");
  }
  return plugin;
}
