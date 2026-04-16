import { Check, CheckCheck } from "lucide-react";
import { useParticipants } from "@/hooks/useParticipants";
import { ackStateFor } from "@/lib/ack-state";
import { cn } from "@/lib/utils";

export type { AckState } from "@/lib/ack-state";
export { ackStateFor } from "@/lib/ack-state";

export interface AckToggleProps {
  /** Authors who have acked this comment. */
  ack: readonly string[];
  /** Current identity name; used to pick between me-acked and others-acked. */
  me?: string | null;
}

/**
 * Non-interactive Ack label with three visual states (see ackStateFor).
 *
 * Originally a click-to-toggle button, this was downgraded to a passive
 * label after repeated mis-clicks accidentally removed acks. The Unack
 * action now lives in the comment card's ellipsis menu (see CommentCard),
 * where intent is explicit.
 */
export function AckToggle({ ack, me }: AckToggleProps) {
  const state = ackStateFor(ack, me);
  const Icon = state === "me-acked" ? CheckCheck : Check;
  const label = state === "unacked" ? "unacked" : "acked";
  const count = ack.length;
  const { resolveDisplayName } = useParticipants();

  // The roster of people who have acked, used as a tooltip so hovering the
  // label still surfaces the same information the old button exposed.
  const rosterLabel =
    count === 0 ? "" : `acked by ${ack.map((a) => resolveDisplayName(a)).join(", ")}`;
  const tooltip = rosterLabel || "No acknowledgments yet";

  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 rounded-sm px-2 py-0.5 text-[10px] leading-none font-semibold",
        state === "me-acked" && "bg-green-500/20 text-green-500 border border-green-500/40",
        state === "others-acked" && "bg-bg-hover text-text-muted border border-bg-border",
        state === "unacked" && "bg-transparent text-text-muted border border-bg-border"
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
