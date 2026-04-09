import { Button } from "@/components/ui/button";
import type RemarginPlugin from "@/main";

interface RemarginSidebarProps {
  plugin: RemarginPlugin;
}

export function RemarginSidebar({ plugin }: RemarginSidebarProps) {
  return (
    <div className="flex flex-col gap-4 p-4">
      <h2 className="text-lg font-semibold text-text-normal">Remargin</h2>
      <p className="text-sm text-text-muted">
        Inline review and structured commenting.
      </p>
      <Button
        variant="default"
        className="bg-accent text-white hover:bg-accent-hover"
        onClick={() => {
          const file = plugin.app.workspace.getActiveFile();
          if (file) {
            console.log("Active file:", file.path);
          }
        }}
      >
        Scan Active File
      </Button>
    </div>
  );
}
