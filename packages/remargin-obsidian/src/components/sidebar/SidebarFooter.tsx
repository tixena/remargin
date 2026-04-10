import { useEffect, useState } from "react";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { useBackend } from "@/hooks/useBackend";

interface SidebarFooterProps {
  commentCount: number;
  pendingCount: number;
}

export function SidebarFooter({ commentCount, pendingCount }: SidebarFooterProps) {
  const backend = useBackend();
  const [cliStatus, setCliStatus] = useState<"connected" | "error" | "checking">("checking");
  const [cliVersion, setCliVersion] = useState("");

  useEffect(() => {
    const check = async () => {
      try {
        const version = await backend.version();
        setCliVersion(version);
        setCliStatus("connected");
      } catch {
        setCliStatus("error");
      }
    };
    check();
    const interval = setInterval(check, 60000);
    return () => clearInterval(interval);
  }, [backend]);

  return (
    <div className="flex items-center justify-between px-4 py-2 gap-2 bg-bg-secondary border-t border-bg-border">
      <span className="font-mono text-[10px] text-text-faint">
        {commentCount} comments &middot; {pendingCount} pending
      </span>
      <TooltipProvider>
        <Tooltip>
          <TooltipTrigger asChild>
            <div className="flex items-center gap-1">
              <span
                className={`w-1.5 h-1.5 rounded-full ${
                  cliStatus === "connected"
                    ? "bg-green-500"
                    : cliStatus === "error"
                      ? "bg-red-400"
                      : "bg-amber-400"
                }`}
              />
              <span className="font-mono text-[10px] text-text-faint">
                {cliStatus === "connected"
                  ? "CLI connected"
                  : cliStatus === "error"
                    ? "CLI not found"
                    : "Checking..."}
              </span>
            </div>
          </TooltipTrigger>
          <TooltipContent>
            <p className="text-xs">
              {cliStatus === "connected" ? cliVersion : "Check remargin binary path in settings"}
            </p>
          </TooltipContent>
        </Tooltip>
      </TooltipProvider>
    </div>
  );
}
