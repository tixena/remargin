export interface RemarginSettings {
  remarginPath: string;
  claudePath: string;
  workingDirectory: string;
  identityMode: "config" | "manual";
  configFilePath: string;
  authorName: string;
  keyFilePath: string;
  remarginMode: string;
  sidebarSide: "left" | "right";
}

export const DEFAULT_SETTINGS: RemarginSettings = {
  remarginPath: "remargin",
  claudePath: "claude",
  workingDirectory: "",
  identityMode: "manual",
  configFilePath: "",
  authorName: "user",
  keyFilePath: "",
  remarginMode: "open",
  sidebarSide: "left",
};
