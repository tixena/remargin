/** The "edited" marker on a comment card. */

import { Pencil } from "lucide-react";
import { formatFullTime, formatRelative } from "@/lib/relative-time";

/** Props for {@link EditedLabel}. */
interface EditedLabelProps {
  editedAt: Date | string;
}

/** Marks a comment changed by `remargin edit`; the exact time shows on hover. */
export function EditedLabel({ editedAt }: EditedLabelProps) {
  const full = `Edited ${formatFullTime(editedAt)}`;
  return (
    <span
      className="inline-flex items-center gap-1 h-[var(--input-height)] rounded-sm border border-bg-border px-2 text-[11px] leading-none font-medium text-text-muted shrink-0"
      aria-label={full}
      title={full}
    >
      <Pencil className="w-3 h-3" aria-hidden="true" />
      {`edited ${formatRelative(editedAt)}`}
    </span>
  );
}
