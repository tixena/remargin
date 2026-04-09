export interface RemarginSettings {
  remarginPath: string;
  claudePath: string;
  workingDirectory: string;
  identityMode: "config" | "manual";
  configFilePath: string;
  authorName: string;
  keyFilePath: string;
  remarginMode: string;
}

export const DEFAULT_SETTINGS: RemarginSettings = {
  remarginPath: "remargin",
  claudePath: "claude",
  workingDirectory: "",
  identityMode: "config",
  configFilePath: ".remargin.yaml",
  authorName: "",
  keyFilePath: "",
  remarginMode: "open",
};
