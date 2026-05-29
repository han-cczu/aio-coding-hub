// Usage: M0 chat permission-mode selector. Exposes Claude's *native*
// permission modes (the `--permission-mode` values), not a per-tool prompt
// UX. Primary segmented control covers the three everyday modes; an
// "advanced" disclosure reveals the power-user / dangerous modes. The chosen
// mode is fixed at session creation, so the parent locks this via `disabled`
// once a session exists.

import { useState } from "react";
import { AlertTriangle, ChevronDown } from "lucide-react";
import type { ChatPermissionMode } from "../../services/chat/chat";
import { TabList, type TabListItem } from "../../ui/TabList";
import { cn } from "../../utils/cn";

const PRIMARY_MODES: Array<TabListItem<ChatPermissionMode>> = [
  { key: "plan", label: "Plan" },
  { key: "acceptEdits", label: "Accept Edits" },
  { key: "default", label: "Default" },
];

const ADVANCED_MODES: Array<TabListItem<ChatPermissionMode>> = [
  { key: "auto", label: "Auto" },
  { key: "dontAsk", label: "Don't Ask" },
  { key: "bypassPermissions", label: "Bypass" },
];

const ADVANCED_KEYS = new Set<ChatPermissionMode>(ADVANCED_MODES.map((m) => m.key));

// Short, human-readable explanation shown under the control for the active
// mode. Mirrors the Claude Agent SDK semantics.
const MODE_HINTS: Record<ChatPermissionMode, string> = {
  plan: "Claude 先制定计划，经批准后才执行工具。",
  acceptEdits: "自动接受文件编辑，其余按规则处理。",
  default: "按 settings.json 的 allow / ask / deny 规则处理。",
  auto: "由模型自行决定是否需要询问。",
  dontAsk: "仅允许列出的工具，其余一律拒绝（不询问）。",
  bypassPermissions: "自动批准所有工具调用，跳过全部权限检查。",
};

export type ChatPermissionModeSelectorProps = {
  value: ChatPermissionMode;
  onChange: (mode: ChatPermissionMode) => void;
  /** Locked once the session is created — mode is fixed for its lifetime in M0. */
  disabled?: boolean;
  className?: string;
};

export function ChatPermissionModeSelector({
  value,
  onChange,
  disabled = false,
  className,
}: ChatPermissionModeSelectorProps) {
  // Auto-open the advanced section when the active mode lives inside it, so a
  // locked session created with e.g. `bypassPermissions` still shows its mode.
  const [advancedOpen, setAdvancedOpen] = useState(() => ADVANCED_KEYS.has(value));
  const isAdvancedActive = ADVANCED_KEYS.has(value);
  const isBypass = value === "bypassPermissions";

  return (
    <div className={cn("flex flex-col gap-2", className)}>
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          权限模式
        </span>
        <TabList
          ariaLabel="权限模式"
          items={PRIMARY_MODES}
          // When an advanced mode is active there is no primary selection;
          // pass a sentinel so none of the primary tabs render as active.
          value={isAdvancedActive ? ("" as ChatPermissionMode) : value}
          onChange={onChange}
          size="sm"
          className={cn(disabled && "pointer-events-none opacity-60")}
        />
        <button
          type="button"
          onClick={() => setAdvancedOpen((prev) => !prev)}
          aria-expanded={advancedOpen}
          className={cn(
            "inline-flex items-center gap-1 rounded-lg px-2 py-1 text-xs font-medium text-muted-foreground transition-colors hover:bg-state-hover hover:text-foreground",
            "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/35",
            isAdvancedActive && "text-foreground"
          )}
        >
          高级
          <ChevronDown
            className={cn("h-3.5 w-3.5 transition-transform", advancedOpen && "rotate-180")}
          />
        </button>
      </div>

      {advancedOpen ? (
        <TabList
          ariaLabel="高级权限模式"
          items={ADVANCED_MODES}
          value={isAdvancedActive ? value : ("" as ChatPermissionMode)}
          onChange={onChange}
          size="sm"
          className={cn("self-start", disabled && "pointer-events-none opacity-60")}
        />
      ) : null}

      <p className="text-xs text-muted-foreground">{MODE_HINTS[value]}</p>

      {isBypass ? (
        <div
          role="alert"
          className="flex items-start gap-2 rounded-lg border border-destructive/50 bg-destructive/10 px-3 py-2 text-xs text-destructive"
        >
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>
            <strong className="font-semibold">危险：</strong>
            Bypass 模式会跳过所有权限检查，Claude
            可不经确认执行任意命令与文件操作。仅在完全信任的环境中使用。
          </span>
        </div>
      ) : null}
    </div>
  );
}
