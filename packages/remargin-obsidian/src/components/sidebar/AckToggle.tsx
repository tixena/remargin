import { Check, CheckCheck } from "lucide-react";
import { ackStateFor } from "@/lib/ack-state";
import { cn } from "@/lib/utils";

export type { AckState } from "@/lib/ack-state";
export { ackStateFor } from "@/lib/ack-state";

export interface AckToggleProps {
  /** Authors who have acked this comment. */
  ack: readonly string[];
  /** Current identity name; used to pick between me-acked and others-acked. */
  me?: string | null;
  /**
   * Called when the user clicks the pill. `remove` is true when the click
   * removes the current identity's ack (state was `me-acked`).
   */
  onToggle: (remove: boolean) => void;
  /** Disable the button while a request is in flight. */
  disabled?: boolean;
}

/**
 * Always-visible Ack pill with three visual states (see ackStateFor). The
 * control is a single toggle: clicking it adds the current identity's ack
 * when in `unacked`/`others-acked` and removes it when in `me-acked`.
 */
export function AckToggle({ ack, me, onToggle, disabled }: AckToggleProps) {
  const state = ackStateFor(ack, me);
  const Icon = state === "me-acked" ? CheckCheck : Check;
  const label = state === "unacked" ? "unacked" : "acked";
  const count = ack.length;

  return (
    <button
      type="button"
      className={cn(
        "inline-flex items-center gap-1 rounded-sm px-2 py-0.5 text-[10px] leading-none font-semibold transition-colors",
        state === "me-acked" && "bg-green-500/20 text-green-500 border border-green-500/40",
        state === "others-acked" && "bg-bg-hover text-text-muted border border-bg-border",
        state === "unacked" &&
          "bg-transparent text-text-muted border border-bg-border hover:bg-bg-hover",
        disabled && "opacity-50 pointer-events-none"
      )}
      onClick={(e) => {
        e.stopPropagation();
        onToggle(state === "me-acked");
      }}
      aria-label={state === "me-acked" ? "Remove my ack" : "Ack this comment"}
      title={state === "me-acked" ? "Remove my ack" : "Ack this comment"}
      disabled={disabled}
    >
      <Icon className="w-2.5 h-2.5" />
      <span>{label}</span>
      {count > 0 && <span className="font-mono text-[9px] font-semibold">{count}</span>}
    </button>
  );
}
