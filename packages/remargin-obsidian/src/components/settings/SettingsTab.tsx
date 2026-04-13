import { useCallback, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { expandPath } from "@/lib/expandPath";
import type { RemarginSettings } from "@/types";
import { SettingsField } from "./SettingsField";

interface SettingsTabProps {
  settings: RemarginSettings;
  onSave: (settings: RemarginSettings) => void;
}

type TestState = "idle" | "loading" | "success" | "error";

export function SettingsTab({ settings, onSave }: SettingsTabProps) {
  const [current, setCurrent] = useState(settings);
  const [testState, setTestState] = useState<TestState>("idle");
  const [testMessage, setTestMessage] = useState("");

  const update = useCallback(
    <K extends keyof RemarginSettings>(field: K, value: RemarginSettings[K]) => {
      setCurrent((prev) => {
        const next = { ...prev, [field]: value };
        onSave(next);
        return next;
      });
    },
    [onSave]
  );

  const handleTestCli = useCallback(async () => {
    setTestState("loading");
    setTestMessage("Testing...");
    try {
      const { exec } = require("child_process") as typeof import("child_process");
      // Expand ~ / $HOME so the Test CLI button honours portable paths.
      // Fall back to a bare 'remargin' when the field is empty.
      const binary = expandPath(current.remarginPath) || "remargin";
      const result = await new Promise<string>((resolve, reject) => {
        exec(
          `${binary} --version`,
          { timeout: 5000 },
          (error: Error | null, stdout: string, stderr: string) => {
            if (error) reject(new Error(stderr || error.message));
            else resolve(stdout.trim());
          }
        );
      });
      setTestState("success");
      setTestMessage(`${result} — OK`);
    } catch (err) {
      setTestState("error");
      setTestMessage(err instanceof Error ? err.message : "Unknown error");
    }
  }, [current.remarginPath]);

  return (
    <div className="flex flex-col h-full bg-bg-primary rounded-lg">
      <div className="flex flex-col gap-1 p-5 px-6 border-b border-bg-border">
        <h2 className="text-xl font-semibold text-text-normal font-sans">Remargin</h2>
        <p className="text-xs text-text-muted font-sans">
          Document commenting system for inline review workflows.
        </p>
      </div>

      <div className="flex flex-col gap-5 p-5 px-6 overflow-y-auto">
        <SettingsField
          label="Remargin binary path"
          description="Path to the remargin CLI binary. Supports ~ and $HOME (e.g. ~/.cargo/bin/remargin) for portability across machines. Leave blank to use whatever is on your PATH."
        >
          <Input
            value={current.remarginPath}
            onChange={(e) => update("remarginPath", e.target.value)}
            className="font-mono text-sm bg-bg-primary border-bg-border"
          />
        </SettingsField>

        <SettingsField
          label="Claude binary path"
          description="Path to the claude CLI binary for AI-assisted commenting. Supports ~ and $HOME."
        >
          <Input
            value={current.claudePath}
            onChange={(e) => update("claudePath", e.target.value)}
            className="font-mono text-sm bg-bg-primary border-bg-border"
          />
        </SettingsField>

        <SettingsField
          label="Working directory"
          description="Base directory for remargin operations. Supports ~ and $HOME. Leave empty to use vault root."
        >
          <Input
            value={current.workingDirectory}
            onChange={(e) => update("workingDirectory", e.target.value)}
            placeholder="/home/user/vault"
            className="font-mono text-sm bg-bg-primary border-bg-border"
          />
        </SettingsField>

        <SettingsField
          label="Sidebar side"
          description="Initial dock side when opening the Remargin sidebar. Applies the next time the view is opened; you can always drag it to the other side manually."
        >
          <ToggleGroup
            type="single"
            value={current.sidebarSide}
            onValueChange={(value) => {
              if (value) update("sidebarSide", value as "left" | "right");
            }}
            className="w-full"
          >
            <ToggleGroupItem
              value="left"
              className="flex-1 text-sm font-medium data-[state=on]:bg-accent data-[state=on]:text-white"
            >
              Left
            </ToggleGroupItem>
            <ToggleGroupItem
              value="right"
              className="flex-1 text-sm font-medium data-[state=on]:bg-accent data-[state=on]:text-white"
            >
              Right
            </ToggleGroupItem>
          </ToggleGroup>
        </SettingsField>

        <Separator />

        <SettingsField
          label="Identity configuration"
          description="Your personal identity lives in ~/.remargin.yaml. The vault-root .remargin.yaml is for the reply agent, not you — don't point this at that file. Use Manual to type author and key path directly. Path fields below support ~ and $HOME for cross-machine portability."
        >
          <ToggleGroup
            type="single"
            value={current.identityMode}
            onValueChange={(value) => {
              if (value) update("identityMode", value as "config" | "manual");
            }}
            className="w-full"
          >
            <ToggleGroupItem
              value="config"
              className="flex-1 text-sm font-medium data-[state=on]:bg-accent data-[state=on]:text-white"
            >
              Config file
            </ToggleGroupItem>
            <ToggleGroupItem
              value="manual"
              className="flex-1 text-sm font-medium data-[state=on]:bg-accent data-[state=on]:text-white"
            >
              Manual
            </ToggleGroupItem>
          </ToggleGroup>

          {current.identityMode === "config" ? (
            <Input
              value={current.configFilePath}
              onChange={(e) => update("configFilePath", e.target.value)}
              className="font-mono text-sm bg-bg-primary border-bg-border mt-2"
            />
          ) : (
            <div className="flex flex-col gap-2 mt-2">
              <Input
                value={current.authorName}
                onChange={(e) => update("authorName", e.target.value)}
                placeholder="Author name"
                className="font-mono text-sm bg-bg-primary border-bg-border"
              />
              <Input
                value={current.keyFilePath}
                onChange={(e) => update("keyFilePath", e.target.value)}
                placeholder="Path to signing key"
                className="font-mono text-sm bg-bg-primary border-bg-border"
              />
            </div>
          )}
        </SettingsField>

        <SettingsField
          label="Remargin mode"
          description="Controls comment integrity enforcement level."
        >
          <Select
            value={current.remarginMode}
            onValueChange={(value) => update("remarginMode", value)}
          >
            <SelectTrigger className="font-mono text-sm bg-bg-primary border-bg-border">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="open">open</SelectItem>
              <SelectItem value="signed">signed</SelectItem>
              <SelectItem value="strict">strict</SelectItem>
            </SelectContent>
          </Select>
        </SettingsField>

        <Separator />

        <div className="flex items-center gap-3">
          <Button
            onClick={handleTestCli}
            disabled={testState === "loading"}
            className="bg-accent text-white hover:bg-accent-hover gap-1.5"
          >
            {testState === "loading" ? "Testing..." : "Test CLI"}
          </Button>
          {testState !== "idle" && testState !== "loading" && (
            <div className="flex items-center gap-1.5">
              <span
                className={`w-2 h-2 rounded-full ${
                  testState === "success" ? "bg-green-500" : "bg-red-400"
                }`}
              />
              <span
                className={`font-mono text-xs ${
                  testState === "success" ? "text-green-500" : "text-red-400"
                }`}
              >
                {testMessage}
              </span>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
