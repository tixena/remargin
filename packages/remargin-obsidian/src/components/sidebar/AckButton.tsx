import { Check, CheckCheck } from "lucide-react";
import { useParticipants } from "@/hooks/useParticipants";
import { ackStateFor } from "@/lib/ack-state";
import { ackVisualFor } from "@/lib/ack-visual";
import { cn } from "@/lib/utils";

export interface AckButtonProps {
  /** Authors who have acked this comment. */
  ack: readonly string[];
  /** Current identity name; used to pick between unacked and others-acked. */
  me?: string | null;
  /** Adds the viewer's ack. Invoked on click. */
  onAck: () => void;
  /**
   * Effective `to:` recipients the card is showing. Drives the arrow +
   * color precedence (see ackVisualFor).
   */
  toTargets?: readonly string[];
}

/**
 * Interactive counterpart to AckToggle, rendered on the comment card only
 * when the viewer has NOT yet acked the comment. Clicking adds the
 * viewer's ack; once added, CommentCard flips to the non-interactive
 * AckToggle label and the Unack action migrates to the ellipsis menu.
 *
 * Arrow + color follow the same `ackVisualFor` precedence as AckToggle
 * so the swap between button and label on click is visually continuous.
 */
export function AckButton({ ack, me, onAck, toTargets = [] }: AckButtonProps) {
  const visual = ackVisualFor(toTargets, ack);
  const Icon = visual.arrow === "double" ? CheckCheck : Check;
  // `ackStateFor` drives the label text ("unacked" vs "acked") so the
  // copy stays stable across refactors; the visual variant is orthogonal.
  const state = ackStateFor(ack, me);
  const count = ack.length;
  const { resolveDisplayName } = useParticipants();

  const rosterLabel =
    count === 0 ? "" : `acked by ${ack.map((a) => resolveDisplayName(a)).join(", ")}`;
  const tooltip = rosterLabel || "No acknowledgments yet";

  return (
    <button
      type="button"
      className={cn(
        "inline-flex items-center gap-1 rounded-sm px-2 py-0.5 text-[10px] leading-none font-semibold cursor-pointer transition-colors",
        visual.tone === "green" &&
          "bg-green-500/20 text-green-500 border border-green-500/40 hover:bg-green-500/30",
        visual.tone === "normal" &&
          "bg-transparent text-text-muted border border-bg-border hover:bg-bg-hover"
      )}
      aria-label={tooltip}
      title={tooltip}
      onClick={(e) => {
        e.stopPropagation();
        onAck();
      }}
    >
      <Icon className="w-2.5 h-2.5" />
      <span>{state === "unacked" ? "unacked" : "acked"}</span>
      {count > 0 && <span className="font-mono text-[9px] font-semibold">{count}</span>}
    </button>
  );
}
