/** The clickable ack control shown on a comment the viewer has not acked. */

import { Check, CheckCheck } from "lucide-react";
import { useParticipants } from "@/hooks/useParticipants";
import { ackStateFor } from "@/lib/ack-state";
import { ackVisualFor } from "@/lib/ack-visual";
import { cn } from "@/lib/utils";

/** Props for {@link AckButton}. */
export interface AckButtonProps {
  ack: readonly string[];
  me?: string | null;
  onAck: () => void;
  /** The effective `to:` recipients, which drive the arrow and color precedence. */
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
        "inline-flex items-center gap-1 h-[var(--input-height)] rounded-sm px-2 text-[10px] leading-none font-semibold cursor-pointer transition-colors",
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
