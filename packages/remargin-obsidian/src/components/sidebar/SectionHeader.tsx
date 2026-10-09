/** The header of a top-level sidebar section. */

import { ChevronDown, type LucideIcon } from "lucide-react";
import { CollapsibleTrigger } from "@/components/ui/collapsible";

/** Props for {@link SectionHeader}. */
interface SectionHeaderProps {
  icon: LucideIcon;
  title: string;
  badge?: number | string;
  badgeVariant?: "default" | "warning";
  open: boolean;
  actions?: React.ReactNode;
}

/**
 * Top-level section header (Sandbox, Inbox, …). Styled by the `.rmg-l1-head`
 * rules in sandbox-hierarchy.css — plain, unlayered CSS with defensive
 * resets, because Obsidian's unlayered `button` styles beat Tailwind v4's
 * `@layer utilities` regardless of specificity. The chevron is a single
 * icon that the CSS rotates on `data-open`.
 */
export function SectionHeader({
  icon: Icon,
  title,
  badge,
  badgeVariant = "default",
  open,
  actions,
}: SectionHeaderProps) {
  const badgeClass =
    badgeVariant === "warning"
      ? "rmg-l1-head__badge rmg-l1-head__badge--warning"
      : "rmg-l1-head__badge";

  // `actions` render as a sibling of the trigger so their buttons never nest inside its <button>.
  return (
    <div className="rmg-l1-head" data-open={open ? "true" : "false"}>
      <CollapsibleTrigger className="rmg-l1-head__trigger">
        <ChevronDown className="rmg-l1-head__chev" />
        <Icon className="rmg-l1-head__icon" />
        <span className="rmg-l1-head__title">{title}</span>
        {badge != null && <span className={badgeClass}>{badge}</span>}
      </CollapsibleTrigger>
      {actions && <span className="rmg-l1-head__actions">{actions}</span>}
    </div>
  );
}
