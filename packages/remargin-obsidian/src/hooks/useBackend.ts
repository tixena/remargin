import { createContext, useContext } from "react";
import type { RemarginBackend } from "@/backend";

export const BackendContext = createContext<RemarginBackend | null>(null);

export function useBackend(): RemarginBackend {
  const backend = useContext(BackendContext);
  if (!backend) {
    throw new Error("useBackend must be used within a BackendContext.Provider");
  }
  return backend;
}
