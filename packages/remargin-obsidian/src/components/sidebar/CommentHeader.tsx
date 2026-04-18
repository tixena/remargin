import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import type { Comment } from "@/generated";
import { useParticipants } from "@/hooks/useParticipants";
import { authorLabel } from "@/lib/authorLabel";
import { formatRelative } from "@/lib/relative-time";

interface CommentHeaderProps {
  comment: Comment;
  /**
   * True when the author is currently present in the session. Only rendered
   * for human authors. Defaults to false.
   */
  isOnline?: boolean;
}

/**
 * Rich header for a comment card: avatar circle, comment-id badge, line
 * badge, username, optional online dot, right-aligned relative timestamp.
 */
export function CommentHeader({ comment, isOnline = false }: CommentHeaderProps) {
  const isAgent = comment.author_type === "agent";
  const initials = isAgent ? "AI" : "H";
  const avatarClass = isAgent ? "bg-purple-400 text-white" : "bg-blue-400 text-white";

  const tsFull = formatFullTime(comment.ts);
  const { resolveDisplayName } = useParticipants();
  const { label: authorDisplay, title: authorTitle } = authorLabel(
    comment.author,
    resolveDisplayName
  );

  return (
    <div className="flex items-center justify-between gap-2 w-full">
      <div className="flex items-center gap-1.5 min-w-0">
        {/*
         * The author-type badge, the id badge, and the line badge share
         * the same base Badge styling (px-1 py-0 text-[9px] leading-none)
         * so they all settle at the same visual height. The avatar's
         * rounded-full shape and colored background are the only
         * overrides; stray sizing (`h-5 w-5`) would make it taller than
         * its siblings and break the header row's visual rhythm.
         */}
        <Badge
          className={`px-1 py-0 rounded-full font-mono text-[9px] font-semibold leading-none ${avatarClass}`}
          aria-label={isAgent ? "AI agent" : "Human"}
        >
          {initials}
        </Badge>
        {comment.id && (
          <Badge className="px-1 py-0 rounded-sm bg-slate-500 text-white font-mono text-[9px] font-semibold leading-none">
            {comment.id}
          </Badge>
        )}
        {comment.line > 0 && (
          <Badge
            variant="outline"
            className="px-1 py-0 rounded-sm font-mono text-[9px] font-normal leading-none"
          >
            L{comment.line}
          </Badge>
        )}
        <span className="text-xs font-semibold text-text-normal truncate" title={authorTitle}>
          {authorDisplay}
        </span>
        {!isAgent && isOnline && (
          <span
            className="inline-block h-1.5 w-1.5 rounded-full bg-[#22C55E] shrink-0"
            aria-label="Online"
          />
        )}
      </div>
      <TooltipProvider>
        <Tooltip>
          <TooltipTrigger asChild>
            <span className="font-mono text-[10px] text-text-faint whitespace-nowrap shrink-0">
              {formatRelative(comment.ts)}
            </span>
          </TooltipTrigger>
          <TooltipContent>
            <p className="text-xs">{tsFull}</p>
          </TooltipContent>
        </Tooltip>
      </TooltipProvider>
    </div>
  );
}

function formatFullTime(ts?: string): string {
  if (!ts) return "";
  try {
    return new Date(ts).toLocaleString(undefined, {
      year: "numeric",
      month: "long",
      day: "numeric",
      hour: "numeric",
      minute: "2-digit",
    });
  } catch {
    return ts;
  }
}
