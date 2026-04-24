import { afterEach, describe, expect, it, vi } from "vitest";
import { createRiskyIpcConfirm } from "../ipcConfirm";

const originalCrypto = globalThis.crypto;

afterEach(() => {
  vi.unstubAllGlobals();
  Object.defineProperty(globalThis, "crypto", {
    configurable: true,
    value: originalCrypto,
  });
});

describe("services/ipcConfirm", () => {
  it("uses randomUUID when available and accepts custom ttl", () => {
    vi.stubGlobal("crypto", {
      randomUUID: vi.fn(() => "uuid-nonce"),
    });

    const payload = createRiskyIpcConfirm("action", "resource", { ttlMs: 1234 });

    expect(payload.confirm).toMatchObject({
      action: "action",
      resource: "resource",
      nonce: "uuid-nonce",
      ttlMs: 1234,
    });
  });

  it("falls back to getRandomValues when randomUUID is unavailable", () => {
    vi.stubGlobal("crypto", {
      getRandomValues: vi.fn((bytes: Uint8Array) => {
        bytes.fill(15);
        return bytes;
      }),
    });

    const payload = createRiskyIpcConfirm("action", "resource");

    expect(payload.confirm.nonce).toBe("0f".repeat(16));
    expect(payload.confirm.ttlMs).toBe(60_000);
  });

  it("falls back to timestamp and Math.random when Web Crypto is unavailable", () => {
    vi.setSystemTime(1_700_000_000_000);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    Object.defineProperty(globalThis, "crypto", {
      configurable: true,
      value: undefined,
    });

    const payload = createRiskyIpcConfirm("action", "resource");

    expect(payload.confirm.nonce).toContain("-");
    expect(payload.confirm.issuedAtMs).toBe(1_700_000_000_000);
    vi.useRealTimers();
  });
});
