// Usage: M0 chat model selector. Picks the model passed to `claude --model`
// for the session — a quick preset (Default / Opus / Sonnet / Haiku) or a
// free-text full name via the "Custom" segment. `Default` omits the flag so
// claude uses its own default. Mirrors the visual style of the permission /
// launcher selectors. The choice is fixed at session creation, so the parent
// locks it via `disabled` once a session exists.

import type { ChatModelPreset } from "../../stores/chatStore";
import { Input } from "../../ui/Input";
import { TabList, type TabListItem } from "../../ui/TabList";
import { cn } from "../../utils/cn";

const PRESETS: Array<TabListItem<ChatModelPreset>> = [
  { key: "default", label: "Default" },
  { key: "opus", label: "Opus" },
  { key: "sonnet", label: "Sonnet" },
  { key: "haiku", label: "Haiku" },
  { key: "custom", label: "自定义" },
];

// One-line explanation shown under the control for the active preset.
const PRESET_HINTS: Record<ChatModelPreset, string> = {
  default: "使用 claude 默认模型（不传 --model）。",
  opus: "使用 Opus（传别名 opus）。",
  sonnet: "使用 Sonnet（传别名 sonnet）。",
  haiku: "使用 Haiku（传别名 haiku）。",
  custom: "填写完整模型名，如 claude-opus-4-8。留空则用 claude 默认。",
};

export type ChatModelSelectorProps = {
  value: ChatModelPreset;
  custom: string;
  onChange: (preset: ChatModelPreset) => void;
  onCustomChange: (full: string) => void;
  /** Locked once the session is created — model is fixed for its lifetime in M0. */
  disabled?: boolean;
  className?: string;
};

export function ChatModelSelector({
  value,
  custom,
  onChange,
  onCustomChange,
  disabled = false,
  className,
}: ChatModelSelectorProps) {
  return (
    <div className={cn("flex flex-col gap-2", className)}>
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          模型
        </span>
        <TabList
          ariaLabel="模型"
          items={PRESETS}
          value={value}
          onChange={onChange}
          size="sm"
          className={cn(disabled && "pointer-events-none opacity-60")}
        />
        {value === "custom" ? (
          <Input
            mono
            value={custom}
            onChange={(event) => onCustomChange(event.target.value)}
            placeholder="claude-opus-4-8"
            disabled={disabled}
            aria-label="自定义模型全名"
            className="h-8 w-56 text-xs"
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
          />
        ) : null}
      </div>

      <p className="text-xs text-muted-foreground">{PRESET_HINTS[value]}</p>
    </div>
  );
}
