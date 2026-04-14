import { ChevronDown, ChevronRight, type LucideIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { CollapsibleTrigger } from "@/components/ui/collapsible";
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
    <CollapsibleTrigger className="flex items-center w-full px-4 py-2 gap-2 bg-bg-border hover:bg-bg-hover overflow-hidden">
      <div className="flex items-center gap-1.5 flex-1 min-w-0 text-left">
        <Chevron className="w-3 h-3 text-text-faint shrink-0" />
        <Icon className="w-3.5 h-3.5 text-text-muted shrink-0" />
        <span className="text-xs font-medium text-text-muted truncate min-w-0">{title}</span>
        {badge != null && (
          <Badge
            className={cn(
              "px-1.5 py-0 text-[10px] font-semibold leading-4 rounded-full shrink-0",
              badgeVariant === "warning" ? "bg-amber-400 text-bg-primary" : "bg-accent text-white"
            )}
          >
            {badge}
          </Badge>
        )}
      </div>
      {actions && (
        <div
          className="flex items-center gap-1 ml-auto shrink-0"
          onClick={(e) => e.stopPropagation()}
        >
          {actions}
        </div>
      )}
    </CollapsibleTrigger>
  );
}
