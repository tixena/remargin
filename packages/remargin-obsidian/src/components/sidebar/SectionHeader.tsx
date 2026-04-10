import { CollapsibleTrigger } from "@/components/ui/collapsible";
import { Badge } from "@/components/ui/badge";
import { ChevronDown, ChevronRight, type LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";

interface SectionHeaderProps {
  icon: LucideIcon;
  title: string;
  badge?: number | string;
  badgeVariant?: "default" | "warning";
  open: boolean;
  actions?: React.ReactNode;
}

export function SectionHeader({
  icon: Icon,
  title,
  badge,
  badgeVariant = "default",
  open,
  actions,
}: SectionHeaderProps) {
  const Chevron = open ? ChevronDown : ChevronRight;
  return (
    <CollapsibleTrigger className="flex items-center justify-between w-full px-4 py-2 gap-2 bg-bg-border hover:bg-bg-hover">
      <div className="flex items-center gap-1.5">
        <Chevron className="w-3 h-3 text-text-faint" />
        <Icon className="w-3.5 h-3.5 text-text-muted" />
        <span className="text-xs font-medium text-text-muted">{title}</span>
        {badge != null && (
          <Badge
            className={cn(
              "px-1.5 py-0 text-[10px] font-semibold leading-4 rounded-full",
              badgeVariant === "warning"
                ? "bg-amber-400 text-bg-primary"
                : "bg-accent text-white"
            )}
          >
            {badge}
          </Badge>
        )}
      </div>
      {actions && (
        <div className="flex items-center gap-1" onClick={(e) => e.stopPropagation()}>
          {actions}
        </div>
      )}
    </CollapsibleTrigger>
  );
}
