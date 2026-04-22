import {
  commands,
  type ClaudeModels as GeneratedClaudeModels,
  type DailyResetMode as GeneratedDailyResetMode,
  type ProviderAuthMode as GeneratedProviderAuthMode,
  type ProviderBaseUrlMode as GeneratedProviderBaseUrlMode,
  type ProviderOAuthDisconnectResult,
  type ProviderOAuthLimitsResult,
  type ProviderOAuthRefreshResult,
  type ProviderOAuthStartFlowResult,
  type ProviderOAuthStatusResult,
  type ProviderSummary as GeneratedProviderSummary,
  type ProviderUpsertInput as GeneratedProviderUpsertInput,
} from "../../generated/bindings";
import {
  invokeGeneratedIpc,
  mapGeneratedCommandResponse,
  type GeneratedCommandResult,
} from "../generatedIpc";
import {
  narrowGeneratedStringUnion,
  type NullableGeneratedKeys,
  type RemapGeneratedKeys,
  type Override,
} from "../generatedTypeUtils";

export type {
  ProviderOAuthDisconnectResult,
  ProviderOAuthLimitsResult,
  ProviderOAuthRefreshResult,
  ProviderOAuthStartFlowResult,
  ProviderOAuthStatusResult,
};

export type CliKey = "claude" | "codex" | "gemini";

export type ClaudeModels = GeneratedClaudeModels;
export type DailyResetMode = GeneratedDailyResetMode;
export type ProviderAuthMode = GeneratedProviderAuthMode;
export type ProviderBaseUrlMode = GeneratedProviderBaseUrlMode;

const CLI_KEY_VALUES = ["claude", "codex", "gemini"] as const satisfies readonly CliKey[];
const PROVIDER_AUTH_MODE_VALUES = ["api_key", "oauth"] as const satisfies readonly ProviderAuthMode[];

export type ProviderSummary = Override<
  GeneratedProviderSummary,
  {
    cli_key: CliKey;
    auth_mode: ProviderAuthMode;
  }
>;

const providerUpsertFieldMap = {
  providerId: "providerId",
  cliKey: "cliKey",
  name: "name",
  baseUrls: "baseUrls",
  baseUrlMode: "baseUrlMode",
  authMode: "authMode",
  apiKey: "apiKey",
  enabled: "enabled",
  costMultiplier: "costMultiplier",
  priority: "priority",
  claudeModels: "claudeModels",
  limit5hUsd: "limit5hUsd",
  limitDailyUsd: "limitDailyUsd",
  dailyResetMode: "dailyResetMode",
  dailyResetTime: "dailyResetTime",
  limitWeeklyUsd: "limitWeeklyUsd",
  limitMonthlyUsd: "limitMonthlyUsd",
  limitTotalUsd: "limitTotalUsd",
  tags: "tags",
  note: "note",
  sourceProviderId: "sourceProviderId",
  bridgeType: "bridgeType",
  streamIdleTimeoutSeconds: "streamIdleTimeoutSeconds",
} as const satisfies Record<keyof GeneratedProviderUpsertInput, keyof GeneratedProviderUpsertInput>;

type ProviderUpsertAuthority = RemapGeneratedKeys<
  GeneratedProviderUpsertInput,
  typeof providerUpsertFieldMap
>;

type ProviderUpsertOptionalKeys =
  | NullableGeneratedKeys<ProviderUpsertAuthority>
  | "streamIdleTimeoutSeconds";

export type ProviderUpsertInput = Omit<
  ProviderUpsertAuthority,
  ProviderUpsertOptionalKeys | "cliKey"
> &
  {
    cliKey: CliKey;
  } & Partial<
    Pick<ProviderUpsertAuthority, ProviderUpsertOptionalKeys>
  >;

type ProviderUpsertTransportInput = Omit<
  GeneratedProviderUpsertInput,
  "streamIdleTimeoutSeconds"
> & {
  streamIdleTimeoutSeconds?: GeneratedProviderUpsertInput["streamIdleTimeoutSeconds"];
};

function toCliKey(value: string, label: string): CliKey {
  return narrowGeneratedStringUnion(value, CLI_KEY_VALUES, label);
}

function toProviderAuthMode(value: string, label: string): ProviderAuthMode {
  return narrowGeneratedStringUnion(value, PROVIDER_AUTH_MODE_VALUES, label);
}

function toProviderSummary(value: GeneratedProviderSummary): ProviderSummary {
  return {
    ...value,
    cli_key: toCliKey(value.cli_key, "providers.cli_key"),
    auth_mode: toProviderAuthMode(value.auth_mode, "providers.auth_mode"),
  };
}

function toProviderUpsertPayload(input: ProviderUpsertInput): ProviderUpsertTransportInput {
  const payloadBase = {
    providerId: input.providerId ?? null,
    cliKey: input.cliKey,
    name: input.name,
    baseUrls: input.baseUrls,
    baseUrlMode: input.baseUrlMode,
    authMode: input.authMode ?? null,
    apiKey: input.apiKey ?? null,
    enabled: input.enabled,
    costMultiplier: input.costMultiplier,
    priority: input.priority ?? null,
    claudeModels: input.claudeModels ?? null,
    limit5hUsd: input.limit5hUsd ?? null,
    limitDailyUsd: input.limitDailyUsd ?? null,
    dailyResetMode: input.dailyResetMode ?? null,
    dailyResetTime: input.dailyResetTime ?? null,
    limitWeeklyUsd: input.limitWeeklyUsd ?? null,
    limitMonthlyUsd: input.limitMonthlyUsd ?? null,
    limitTotalUsd: input.limitTotalUsd ?? null,
    tags: input.tags ?? null,
    note: input.note ?? null,
    sourceProviderId: input.sourceProviderId ?? null,
    bridgeType: input.bridgeType ?? null,
  } satisfies Omit<GeneratedProviderUpsertInput, "streamIdleTimeoutSeconds">;

  if (Object.prototype.hasOwnProperty.call(input, "streamIdleTimeoutSeconds")) {
    return {
      ...payloadBase,
      streamIdleTimeoutSeconds: input.streamIdleTimeoutSeconds ?? 0,
    } satisfies ProviderUpsertTransportInput;
  }

  return payloadBase;
}

export async function providersList(cliKey: CliKey) {
  return invokeGeneratedIpc<ProviderSummary[]>({
    title: "读取供应商列表失败",
    cmd: "providers_list",
    args: { cliKey },
    invoke: async () =>
      mapGeneratedCommandResponse(await commands.providersList(cliKey), (rows) =>
        rows.map(toProviderSummary)
      ),
  });
}

export async function providerUpsert(input: ProviderUpsertInput) {
  const payload = toProviderUpsertPayload(input);
  const logPayload = {
    ...payload,
    apiKey: payload.apiKey == null ? payload.apiKey : "[REDACTED]",
  };

  return invokeGeneratedIpc<ProviderSummary>({
    title: "保存供应商失败",
    cmd: "provider_upsert",
    args: { input: logPayload },
    invoke: async () =>
      mapGeneratedCommandResponse(
        await commands.providerUpsert(payload as GeneratedProviderUpsertInput),
        toProviderSummary
      ),
  });
}

export async function baseUrlPingMs(baseUrl: string) {
  return invokeGeneratedIpc<number>({
    title: "测试 Base URL 延迟失败",
    cmd: "base_url_ping_ms",
    args: { baseUrl },
    invoke: () => commands.baseUrlPingMs(baseUrl) as Promise<GeneratedCommandResult<number>>,
  });
}

export async function providerSetEnabled(
  providerId: number,
  enabled: boolean
): Promise<ProviderSummary | null> {
  return invokeGeneratedIpc<ProviderSummary>({
    title: "更新供应商启用状态失败",
    cmd: "provider_set_enabled",
    args: { providerId, enabled },
    invoke: async () =>
      mapGeneratedCommandResponse(
        await commands.providerSetEnabled(providerId, enabled),
        toProviderSummary
      ),
  });
}

export async function providerDelete(providerId: number) {
  return invokeGeneratedIpc<boolean>({
    title: "删除供应商失败",
    cmd: "provider_delete",
    args: { providerId },
    invoke: () =>
      commands.providerDelete(providerId) as Promise<GeneratedCommandResult<boolean>>,
  });
}

export async function providersReorder(
  cliKey: CliKey,
  orderedProviderIds: number[]
): Promise<ProviderSummary[] | null> {
  return invokeGeneratedIpc<ProviderSummary[]>({
    title: "调整供应商顺序失败",
    cmd: "providers_reorder",
    args: { cliKey, orderedProviderIds },
    invoke: async () =>
      mapGeneratedCommandResponse(await commands.providersReorder(cliKey, orderedProviderIds), (rows) =>
        rows.map(toProviderSummary)
      ),
  });
}

export async function providerDuplicate(providerId: number): Promise<ProviderSummary | null> {
  return invokeGeneratedIpc<ProviderSummary>({
    title: "复制供应商失败",
    cmd: "provider_duplicate",
    args: { providerId },
    invoke: async () =>
      mapGeneratedCommandResponse(await commands.providerDuplicate(providerId), toProviderSummary),
  });
}

export async function providerCopyApiKeyToClipboard(providerId: number) {
  return invokeGeneratedIpc<boolean>({
    title: "复制 API Key 失败",
    cmd: "provider_copy_api_key_to_clipboard",
    args: { providerId },
    invoke: () =>
      commands.providerCopyApiKeyToClipboard(providerId) as Promise<GeneratedCommandResult<boolean>>,
  });
}

export async function providerClaudeTerminalLaunchCommand(providerId: number) {
  return invokeGeneratedIpc<string>({
    title: "生成 Claude 终端启动命令失败",
    cmd: "provider_claude_terminal_launch_command",
    args: { providerId },
    invoke: () =>
      commands.providerClaudeTerminalLaunchCommand(providerId) as Promise<
        GeneratedCommandResult<string>
      >,
  });
}

export async function providerOAuthStartFlow(
  cliKey: string,
  providerId: number
): Promise<ProviderOAuthStartFlowResult> {
  return invokeGeneratedIpc<ProviderOAuthStartFlowResult>({
    title: "启动 OAuth 登录失败",
    cmd: "provider_oauth_start_flow",
    args: { cliKey, providerId },
    invoke: () =>
      commands.providerOauthStartFlow(cliKey, providerId) as Promise<
        GeneratedCommandResult<ProviderOAuthStartFlowResult>
      >,
  });
}

export async function providerOAuthRefresh(providerId: number): Promise<ProviderOAuthRefreshResult> {
  return invokeGeneratedIpc<ProviderOAuthRefreshResult>({
    title: "刷新 OAuth 登录失败",
    cmd: "provider_oauth_refresh",
    args: { providerId },
    invoke: () =>
      commands.providerOauthRefresh(providerId) as Promise<
        GeneratedCommandResult<ProviderOAuthRefreshResult>
      >,
  });
}

export async function providerOAuthDisconnect(
  providerId: number
): Promise<ProviderOAuthDisconnectResult> {
  return invokeGeneratedIpc<ProviderOAuthDisconnectResult>({
    title: "断开 OAuth 登录失败",
    cmd: "provider_oauth_disconnect",
    args: { providerId },
    invoke: () =>
      commands.providerOauthDisconnect(providerId) as Promise<
        GeneratedCommandResult<ProviderOAuthDisconnectResult>
      >,
  });
}

export async function providerOAuthStatus(providerId: number): Promise<ProviderOAuthStatusResult> {
  return invokeGeneratedIpc<ProviderOAuthStatusResult>({
    title: "读取 OAuth 状态失败",
    cmd: "provider_oauth_status",
    args: { providerId },
    invoke: () =>
      commands.providerOauthStatus(providerId) as Promise<
        GeneratedCommandResult<ProviderOAuthStatusResult>
      >,
  });
}

export type OAuthLimitsResult = ProviderOAuthLimitsResult;

export async function providerOAuthFetchLimits(
  providerId: number
): Promise<OAuthLimitsResult | null> {
  return invokeGeneratedIpc<OAuthLimitsResult>({
    title: "读取 OAuth 限额失败",
    cmd: "provider_oauth_fetch_limits",
    args: { providerId },
    invoke: () =>
      commands.providerOauthFetchLimits(providerId) as Promise<
        GeneratedCommandResult<OAuthLimitsResult>
      >,
  });
}

// ---------------------------------------------------------------------------
// Provider Type Info — centralised auth-mode / bridge derivation
// ---------------------------------------------------------------------------

export interface ProviderTypeInfo {
  /** Whether this is a CX2CC bridge (has source_provider_id or bridge_type is cx2cc) */
  isCx2cc: boolean;
  /** Whether this is a CX2CC gateway (bridge_type=cx2cc but no source_provider_id) */
  isCx2ccGateway: boolean;
  /** Whether this is OAuth mode */
  isOAuth: boolean;
  /** Effective auth mode: api_key / oauth / cx2cc */
  effectiveAuthMode: "api_key" | "oauth" | "cx2cc";
}

export function getProviderTypeInfo(
  provider:
    | Pick<ProviderSummary, "auth_mode" | "bridge_type" | "source_provider_id">
    | null
    | undefined
): ProviderTypeInfo {
  if (!provider) {
    return { isCx2cc: false, isCx2ccGateway: false, isOAuth: false, effectiveAuthMode: "api_key" };
  }
  const isCx2cc = provider.source_provider_id != null || provider.bridge_type === "cx2cc";
  const isCx2ccGateway = provider.bridge_type === "cx2cc" && provider.source_provider_id == null;
  const isOAuth = provider.auth_mode === "oauth";
  const effectiveAuthMode: ProviderTypeInfo["effectiveAuthMode"] = isCx2cc
    ? "cx2cc"
    : isOAuth
      ? "oauth"
      : "api_key";
  return { isCx2cc, isCx2ccGateway, isOAuth, effectiveAuthMode };
}
