/** The Updates section of the settings tab. */

import { useCallback, useState } from "react";
import { Button } from "@/components/ui/button";
import type { ComponentCheck, UpdateCheckState, UpdateComponent } from "@/lib/githubReleases";
import { cn } from "@/lib/utils";
import {
  canUpdatePlugin as canUpdatePluginCheck,
  messageForCheck,
  messageForUpdate,
  statusChipClasses,
  statusLabel,
} from "./updatesSection.helpers";

/**
 * Props for the Settings → Updates section: two rows (Plugin / CLI) with installed/latest/status
 * chip, plus a section-level "Check now" button. All I/O arrives as callbacks: the caller owns
 * the cache refresh (`onCheckNow`) and the shell-out (`onUpdatePlugin`), and the component only
 * orchestrates local in-progress state and status messages.
 */
export interface UpdatesSectionProps {
  state: UpdateCheckState | undefined;
  /** Resolves once the plugin data is persisted; the parent then re-renders with a new `state`. */
  onCheckNow: () => Promise<void>;
  /** Shells out to `remargin obsidian install`; the stderr tail is shown verbatim on failure. */
  onUpdatePlugin: () => Promise<{ ok: boolean; stderr: string }>;
}

/** What the section is doing: idle, checking, installing, or showing an outcome message. */
type ActionState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "installing" }
  | { kind: "message"; ok: boolean; text: string };

/** Props for {@link ComponentRow}. */
interface RowProps {
  label: string;
  component: UpdateComponent;
  check: ComponentCheck | undefined;
  children?: React.ReactNode;
}

function ComponentRow({ label, component, check, children }: RowProps) {
  const installed = check?.installed?.trim() || "unknown";
  const latest = check?.latest?.trim() || "—";
  return (
    <div
      className="flex flex-col gap-1.5 px-3 py-2.5 rounded-md border border-bg-border bg-bg-primary"
      data-component={component}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="text-sm font-medium text-text-normal font-sans">{label}</span>
        {check ? (
          <span
            className={cn(
              "inline-flex items-center rounded-full px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide",
              statusChipClasses(check)
            )}
            data-status={check.status}
          >
            {statusLabel(check)}
          </span>
        ) : (
          <span className="text-[10px] font-medium text-text-muted uppercase tracking-wide">
            checking…
          </span>
        )}
      </div>
      <div className="flex items-center gap-3 font-mono text-xs text-text-muted">
        <span>
          installed: <span className="text-text-normal">{installed}</span>
        </span>
        <span>
          latest: <span className="text-text-normal">{latest}</span>
        </span>
      </div>
      {children}
    </div>
  );
}

export function UpdatesSection({ state, onCheckNow, onUpdatePlugin }: UpdatesSectionProps) {
  const [action, setAction] = useState<ActionState>({ kind: "idle" });

  const plugin = state?.plugin;
  const cli = state?.cli;
  const canUpdate = canUpdatePluginCheck(plugin);

  const handleCheckNow = useCallback(async () => {
    setAction({ kind: "checking" });
    try {
      await onCheckNow();
      const message = messageForCheck();
      if (message) setAction({ kind: "message", ...message });
      else setAction({ kind: "idle" });
    } catch (err) {
      const message = messageForCheck(err instanceof Error ? err : new Error("Check failed"));
      if (message) setAction({ kind: "message", ...message });
    }
  }, [onCheckNow]);

  const handleUpdate = useCallback(async () => {
    setAction({ kind: "installing" });
    try {
      const result = await onUpdatePlugin();
      const message = messageForUpdate(result);
      if (message) setAction({ kind: "message", ...message });
    } catch (err) {
      const message = messageForUpdate(err instanceof Error ? err : new Error("unknown error"));
      if (message) setAction({ kind: "message", ...message });
    }
  }, [onUpdatePlugin]);

  const lastChecked = state?.lastCheckedAt
    ? new Date(state.lastCheckedAt).toLocaleString()
    : "never";

  return (
    <section className="flex flex-col gap-3 w-full" aria-label="Updates">
      <header className="flex items-center justify-between gap-2">
        <div className="flex flex-col gap-0.5">
          <h3 className="text-sm font-semibold text-text-normal font-sans">Updates</h3>
          <p className="text-xs text-text-muted font-sans">
            Last checked: <span className="font-mono">{lastChecked}</span>
          </p>
        </div>
        <Button
          variant="outline"
          size="sm"
          onClick={() => {
            void handleCheckNow();
          }}
          disabled={action.kind === "checking" || action.kind === "installing"}
        >
          {action.kind === "checking" ? "Checking…" : "Check now"}
        </Button>
      </header>

      <ComponentRow label="Plugin" component="plugin" check={plugin}>
        <div className="flex items-center gap-2 pt-1">
          <Button
            variant="default"
            size="sm"
            onClick={() => {
              void handleUpdate();
            }}
            disabled={!canUpdate || action.kind === "installing"}
            className="bg-accent text-white hover:bg-accent-hover"
          >
            {action.kind === "installing" ? "Updating…" : canUpdate ? "Update" : "Up to date"}
          </Button>
          {plugin?.releaseUrl ? (
            <a
              href={plugin.releaseUrl}
              target="_blank"
              rel="noreferrer noopener"
              className="text-xs text-text-muted hover:text-accent underline-offset-2 hover:underline"
            >
              release notes
            </a>
          ) : null}
        </div>
      </ComponentRow>

      <ComponentRow label="CLI" component="cli" check={cli}>
        <p
          className={cn(
            "text-xs font-sans",
            cli?.status === "update-available" ? "text-text-normal" : "text-text-muted"
          )}
        >
          Update via your install method (cargo, brew, manual).
        </p>
      </ComponentRow>

      {action.kind === "message" ? (
        <p
          className={cn("text-xs font-sans", action.ok ? "text-green-500" : "text-red-400")}
          role="status"
        >
          {action.text}
        </p>
      ) : null}
    </section>
  );
}
