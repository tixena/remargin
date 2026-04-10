import { SidebarShell } from "@/components/sidebar/SidebarShell";
import type RemarginPlugin from "@/main";

interface RemarginSidebarProps {
  plugin: RemarginPlugin;
}

export function RemarginSidebar({ plugin }: RemarginSidebarProps) {
  return <SidebarShell plugin={plugin} />;
}
