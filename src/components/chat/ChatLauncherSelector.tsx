// Usage: M0 chat launcher selector. Picks which executable starts the
// session's `claude` process — `auto` (let the backend choose), `reclaude`
// (the user's launcher, with auth/config sync), or `claude` (direct). Mirrors
// the visual style of ChatPermissionModeSelector. The choice is fixed at
// session creation, so the parent locks it via `disabled` once a session
// exists.

import type { ChatLauncherChoice } from "../../stores/chatStore";
import { TabList, type TabListItem } from "../../ui/TabList";
import { cn } from "../../utils/cn";

const LAUNCHERS: Array<TabListItem<ChatLauncherChoice>> = [
  { key: "auto", label: "Auto" },
  { key: "reclaude", label: "reclaude" },
  { key: "claude", label: "claude" },
];

// One-line explanation shown under the control for the active launcher.
const LAUNCHER_HINTS: Record<ChatLauncherChoice, string> = {
  auto: "优先使用 reclaude（带认证同步），不可用时回退 claude。",
  reclaude: "强制走 reclaude 启动器（执行认证 / 配置同步后再委托 claude）。",
  claude: "直连 claude，跳过 reclaude 的同步步骤。",
};

export type ChatLauncherSelectorProps = {
  value: ChatLauncherChoice;
  onChange: (launcher: ChatLauncherChoice) => void;
  /** Locked once the session is created — launcher is fixed for its lifetime in M0. */
  disabled?: boolean;
  className?: string;
};

export function ChatLauncherSelector({
  value,
  onChange,
  disabled = false,
  className,
}: ChatLauncherSelectorProps) {
  return (
    <div className={cn("flex flex-col gap-2", className)}>
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          启动器
        </span>
        <TabList
          ariaLabel="启动器"
          items={LAUNCHERS}
          value={value}
          onChange={onChange}
          size="sm"
          className={cn(disabled && "pointer-events-none opacity-60")}
        />
      </div>

      <p className="text-xs text-muted-foreground">{LAUNCHER_HINTS[value]}</p>
    </div>
  );
}
