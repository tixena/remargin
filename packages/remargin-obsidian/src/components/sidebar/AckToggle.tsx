/** The passive ack label shown on a comment card. */

import { Check, CheckCheck } from "lucide-react";
import { useParticipants } from "@/hooks/useParticipants";
import { ackStateFor } from "@/lib/ack-state";
import { ackVisualFor } from "@/lib/ack-visual";
import { cn } from "@/lib/utils";

export type { AckState } from "@/lib/ack-state";
export { ackStateFor } from "@/lib/ack-state";

/** Props for {@link AckToggle}. */
export interface AckToggleProps {
  ack: readonly string[];
  me?: string | null;
  /** `comment.to` when non-empty, else the parent's author for a reply, else `[]`. */
  toTargets?: readonly string[];
}

/**
 * Non-interactive Ack label. Arrow + color are driven by `ackVisualFor`
 * (see lib/ack-visual.ts): double arrow + green when the directed-at
 * recipient has acked (or the comment was directed to nobody and anyone
 * acked), single arrow + green when only an outsider acked, single arrow
 * + muted when there are no acks at all. The label is passive: the Unack
 * action lives in the comment card's ellipsis menu.
 */
export function AckToggle({ ack, me, toTargets = [] }: AckToggleProps) {
  const visual = ackVisualFor(toTargets, ack);
  const Icon = visual.arrow === "double" ? CheckCheck : Check;
  const state = ackStateFor(ack, me);
  const label = state === "unacked" ? "unacked" : "acked";
  const count = ack.length;
  const { resolveDisplayName } = useParticipants();

  const rosterLabel =
    count === 0 ? "" : `acked by ${ack.map((a) => resolveDisplayName(a)).join(", ")}`;
  const tooltip = rosterLabel || "No acknowledgments yet";

  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 h-[var(--input-height)] rounded-sm px-2 text-[10px] leading-none font-semibold",
        visual.tone === "green" && "bg-green-500/20 text-green-500 border border-green-500/40",
        visual.tone === "normal" && "bg-transparent text-text-muted border border-bg-border"
      )}
      aria-label={tooltip}
      title={tooltip}
    >
      <Icon className="w-2.5 h-2.5" />
      <span>{label}</span>
      {count > 0 && <span className="font-mono text-[9px] font-semibold">{count}</span>}
    </span>
  );
}
