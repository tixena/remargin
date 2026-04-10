interface SettingsFieldProps {
  label: string;
  description: string;
  children: React.ReactNode;
}

export function SettingsField({ label, description, children }: SettingsFieldProps) {
  return (
    <div className="flex flex-col gap-1.5 w-full">
      <label className="text-sm font-medium text-text-normal">{label}</label>
      <p className="text-xs text-text-muted">{description}</p>
      {children}
    </div>
  );
}
