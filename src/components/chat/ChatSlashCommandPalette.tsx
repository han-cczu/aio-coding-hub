// Usage: M0 chat slash-command autocomplete palette. A presentational dropdown
// rendered above the chat input when the user types a leading `/`. The parent
// (ChatPage) owns the input text, the open state, and the active index (so the
// textarea's keydown can drive ↑/↓/Enter/Tab/Esc); this component renders the
// filtered candidates, highlights the active one, and reports hover/click.

import type { ChatSlashCommand, ChatSlashCommandSource } from "../../services/chat/chat";
import { cn } from "../../utils/cn";

const SOURCE_LABELS: Record<ChatSlashCommandSource, string> = {
  builtin: "内置",
  skill: "技能",
  command: "命令",
};

const SOURCE_BADGE_CLASS: Record<ChatSlashCommandSource, string> = {
  builtin: "bg-surface-inset text-muted-foreground",
  skill: "bg-primary/10 text-primary",
  command: "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
};

/**
 * Extract the query from raw input when in "slash mode": the input must start
 * with a single `/` and contain no whitespace yet (a space means the command
 * is chosen and the user is typing arguments). Returns the part after `/`
 * (may be empty), or `null` when the palette should not show.
 */
export function slashQueryFromInput(input: string): string | null {
  if (!input.startsWith("/")) return null;
  // A second `/` (e.g. a path) or any whitespace ends command-name entry.
  const rest = input.slice(1);
  if (/\s/.test(rest)) return null;
  return rest;
}

/**
 * Filter + order commands for a query (case-insensitive). Prefix matches rank
 * before mid-string substring matches; ties keep the input order (which is
 * already curated by mergeSlashCommands). An empty query returns all.
 */
export function filterSlashCommands(
  commands: ChatSlashCommand[],
  query: string
): ChatSlashCommand[] {
  const q = query.trim().toLowerCase();
  if (!q) return commands;
  const prefix: ChatSlashCommand[] = [];
  const substring: ChatSlashCommand[] = [];
  for (const cmd of commands) {
    const name = cmd.name.toLowerCase();
    if (name.startsWith(q)) prefix.push(cmd);
    else if (name.includes(q)) substring.push(cmd);
  }
  return [...prefix, ...substring];
}

export type ChatSlashCommandPaletteProps = {
  items: ChatSlashCommand[];
  activeIndex: number;
  onHover: (index: number) => void;
  onPick: (command: ChatSlashCommand) => void;
  className?: string;
};

export function ChatSlashCommandPalette({
  items,
  activeIndex,
  onHover,
  onPick,
  className,
}: ChatSlashCommandPaletteProps) {
  if (items.length === 0) return null;

  return (
    <div
      role="listbox"
      aria-label="斜杠命令"
      className={cn(
        "max-h-64 overflow-y-auto rounded-lg border border-line bg-surface-panel py-1 shadow-lg",
        className
      )}
    >
      {items.map((cmd, index) => {
        const active = index === activeIndex;
        return (
          <button
            key={cmd.name}
            type="button"
            role="option"
            aria-selected={active}
            data-index={index}
            // Use onMouseDown (not onClick) so selecting does not blur the
            // textarea first, which would close the palette before the pick.
            onMouseDown={(event) => {
              event.preventDefault();
              onPick(cmd);
            }}
            onMouseEnter={() => onHover(index)}
            className={cn(
              "flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm transition-colors",
              active ? "bg-state-selected text-state-selected-foreground" : "hover:bg-state-hover"
            )}
          >
            <span className="font-mono font-medium">/{cmd.name}</span>
            {cmd.description ? (
              <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                {cmd.description}
              </span>
            ) : (
              <span className="flex-1" />
            )}
            <span
              className={cn(
                "shrink-0 rounded px-1.5 py-0.5 text-[10px] font-semibold",
                SOURCE_BADGE_CLASS[cmd.source]
              )}
            >
              {SOURCE_LABELS[cmd.source]}
            </span>
          </button>
        );
      })}
    </div>
  );
}
