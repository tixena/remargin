import { Check, CheckCheck } from "lucide-react";
import { useParticipants } from "@/hooks/useParticipants";
import { ackStateFor } from "@/lib/ack-state";
import { ackVisualFor } from "@/lib/ack-visual";
import { cn } from "@/lib/utils";

export type { AckState } from "@/lib/ack-state";
export { ackStateFor } from "@/lib/ack-state";

export interface AckToggleProps {
  /** Authors who have acked this comment. */
  ack: readonly string[];
  /** Current identity name; used to pick between me-acked and others-acked. */
  me?: string | null;
  /**
   * Effective `to:` recipients the card is showing — `comment.to` when
   * non-empty, else the parent comment's author for replies, else `[]`.
   * Drives the arrow + color precedence defined in ackVisualFor.
   */
  toTargets?: readonly string[];
}

/**
 * Non-interactive Ack label. Arrow + color are driven by `ackVisualFor`
 * (see lib/ack-visual.ts): double arrow + green when the directed-at
 * recipient has acked (or the comment was directed to nobody and anyone
 * acked), single arrow + green when only an outsider acked, single arrow
 * + muted when there are no acks at all. The green "me-acked" special
 * case still stands because a viewer who is in `to:` and also in `ack`
 * trips rule 2 on the double-green branch.
 *
 * Originally a click-to-toggle button, this was downgraded to a passive
 * label after repeated mis-clicks accidentally removed acks. The Unack
 * action now lives in the comment card's ellipsis menu.
 */
export function AckToggle({ ack, me, toTargets = [] }: AckToggleProps) {
  const visual = ackVisualFor(toTargets, ack);
  const Icon = visual.arrow === "double" ? CheckCheck : Check;
  // `ackStateFor` still drives the label text so existing tests / tooltips
  // stay meaningful — the visual variant is orthogonal.
  const state = ackStateFor(ack, me);
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
