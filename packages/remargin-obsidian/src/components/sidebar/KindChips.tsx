import { Tag } from "lucide-react";

interface KindChipsProps {
  kinds: readonly string[] | undefined;
}

/** One chip per `remargin_kind` entry, in stored order. */
export function KindChips({ kinds }: KindChipsProps) {
  return (
    <>
      {(kinds ?? []).map((kind) => (
        <span
          key={kind}
          className="inline-flex items-center gap-1 h-[var(--input-height)] rounded-full border border-bg-border px-2 text-[11px] leading-none font-medium text-text-muted shrink-0"
          aria-label={`Kind: ${kind}`}
          title={`Kind: ${kind}`}
        >
          <Tag className="w-3 h-3" aria-hidden="true" />
          {kind}
        </span>
      ))}
    </>
  );
}
