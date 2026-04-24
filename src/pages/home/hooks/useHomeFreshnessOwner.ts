import { useCallback, useEffect, useRef } from "react";
import { gatewayEventNames } from "../../../constants/gatewayEvents";
import { useWindowForeground } from "../../../hooks/useWindowForeground";
import { logToConsole } from "../../../services/consoleLog";
import { subscribeGatewayEvent } from "../../../services/gateway/gatewayEventBus";
import { isGatewayRequestSignalEvent } from "../../../services/gateway/gatewayEvents";
import { isRequestSignalComplete } from "../../../services/gateway/requestLogState";

type RefreshSource = "request_signal.complete" | "foreground" | "manual";

type UseHomeFreshnessOwnerOptions = {
  overviewActive: boolean;
  foregroundActive: boolean;
  requestLogsRefreshWindowMs?: number;
  foregroundThrottleMs?: number;
  onRefreshRequestLogs: () => Promise<unknown>;
};

function resolveRequestLogsRefreshWindowMs(input: number | undefined) {
  if (!Number.isFinite(input) || input == null) return 1000;
  return Math.max(200, Math.min(2_000, Math.trunc(input)));
}

export function useHomeFreshnessOwner({
  overviewActive,
  foregroundActive,
  requestLogsRefreshWindowMs,
  foregroundThrottleMs = 1000,
  onRefreshRequestLogs,
}: UseHomeFreshnessOwnerOptions) {
  const active = overviewActive && foregroundActive;
  const refreshWindowMs = resolveRequestLogsRefreshWindowMs(requestLogsRefreshWindowMs);
  const timerRef = useRef<number | null>(null);
  const queuedRef = useRef(false);
  const inFlightRef = useRef(false);
  const activeRef = useRef(active);
  const previousActiveRef = useRef(active);
  const onRefreshRequestLogsRef = useRef(onRefreshRequestLogs);

  useEffect(() => {
    onRefreshRequestLogsRef.current = onRefreshRequestLogs;
  }, [onRefreshRequestLogs]);

  const clearQueuedRefresh = useCallback(() => {
    if (timerRef.current != null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    queuedRef.current = false;
  }, []);

  const flushRequestLogs = useCallback(
    (source: RefreshSource): Promise<unknown> | null => {
      if (!activeRef.current) {
        clearQueuedRefresh();
        return null;
      }

      if (inFlightRef.current) {
        queuedRef.current = true;
        return null;
      }

      queuedRef.current = false;
      inFlightRef.current = true;

      return onRefreshRequestLogsRef
        .current()
        .catch((error) => {
          logToConsole("warn", "首页请求记录刷新失败", {
            source,
            error: String(error),
          });
          throw error;
        })
        .finally(() => {
          inFlightRef.current = false;
          if (!queuedRef.current || !activeRef.current) {
            queuedRef.current = false;
            return;
          }
          queuedRef.current = false;
          void flushRequestLogs(source);
        });
    },
    [clearQueuedRefresh]
  );

  const scheduleRequestLogsRefresh = useCallback(
    (source: RefreshSource) => {
      if (!activeRef.current) {
        return;
      }

      if (timerRef.current != null) {
        queuedRef.current = true;
        return;
      }

      timerRef.current = window.setTimeout(() => {
        timerRef.current = null;
        void flushRequestLogs(source);
      }, refreshWindowMs);
    },
    [flushRequestLogs, refreshWindowMs]
  );

  const refreshRequestLogsNow = useCallback(() => {
    return flushRequestLogs("manual") ?? Promise.resolve(null);
  }, [flushRequestLogs]);

  useWindowForeground({
    enabled: overviewActive,
    throttleMs: foregroundThrottleMs,
    onForeground: () => {
      scheduleRequestLogsRefresh("foreground");
    },
  });

  useEffect(() => {
    const wasActive = previousActiveRef.current;
    previousActiveRef.current = active;
    activeRef.current = active;
    if (!active) {
      clearQueuedRefresh();
      return;
    }

    if (!wasActive) {
      scheduleRequestLogsRefresh("foreground");
    }
  }, [active, clearQueuedRefresh, scheduleRequestLogsRefresh]);

  useEffect(() => {
    if (!active) {
      return;
    }

    let cancelled = false;
    const requestSignalSub = subscribeGatewayEvent(gatewayEventNames.requestSignal, (payload) => {
      if (cancelled || !isGatewayRequestSignalEvent(payload)) {
        return;
      }

      if (!isRequestSignalComplete(payload)) {
        return;
      }

      scheduleRequestLogsRefresh("request_signal.complete");
    });

    void Promise.allSettled([requestSignalSub.ready]).then((results) => {
      if (cancelled) {
        return;
      }

      const subscribeFailed = results.some((result) => result.status === "rejected");
      if (!subscribeFailed) {
        return;
      }

      requestSignalSub.unsubscribe();
      const failedResult = results.find((result) => result.status === "rejected");
      logToConsole("warn", "首页请求记录实时监听初始化失败", {
        stage: "useHomeFreshnessOwner",
        error: String(failedResult?.status === "rejected" ? failedResult.reason : "unknown"),
      });
    });

    return () => {
      cancelled = true;
      requestSignalSub.unsubscribe();
    };
  }, [active, scheduleRequestLogsRefresh]);

  useEffect(() => {
    return () => {
      clearQueuedRefresh();
      inFlightRef.current = false;
    };
  }, [clearQueuedRefresh]);

  return {
    refreshRequestLogsNow,
  };
}
