import { beforeEach, describe, expect, it, vi } from "vitest";
import { commands } from "../../../generated/bindings";
import {
  type CliSessionsFolderLookupEntry,
  type CliSessionsPaginatedMessages,
  type CliSessionsProjectSummary,
  type CliSessionsSessionSummary,
  cliSessionsFolderLookupByIds,
  cliSessionsProjectsList,
  cliSessionsSessionsList,
  cliSessionsMessagesGet,
  cliSessionsSessionDelete,
  escapeShellArg,
} from "../cliSessions";

function makeCliSessionsProjectSummary(
  overrides: Partial<CliSessionsProjectSummary> = {}
): CliSessionsProjectSummary {
  return {
    source: "claude",
    id: "proj-1",
    display_path: "/tmp/project",
    short_name: "project",
    session_count: 1,
    last_modified: null,
    model_provider: null,
    wsl_distro: null,
    ...overrides,
  };
}

function makeCliSessionsSessionSummary(
  overrides: Partial<CliSessionsSessionSummary> = {}
): CliSessionsSessionSummary {
  return {
    source: "claude",
    session_id: "sess-1",
    file_path: "/tmp/session.json",
    first_prompt: null,
    message_count: 0,
    created_at: null,
    modified_at: null,
    git_branch: null,
    project_path: null,
    is_sidechain: null,
    cwd: null,
    model_provider: null,
    cli_version: null,
    wsl_distro: null,
    ...overrides,
  };
}

function makeCliSessionsPaginatedMessages(
  overrides: Partial<CliSessionsPaginatedMessages> = {}
): CliSessionsPaginatedMessages {
  return {
    messages: [],
    total: 0,
    page: 0,
    page_size: 50,
    has_more: false,
    ...overrides,
  };
}

function makeCliSessionsFolderLookupEntry(
  overrides: Partial<CliSessionsFolderLookupEntry> = {}
): CliSessionsFolderLookupEntry {
  return {
    source: "claude",
    session_id: "s1",
    folder_name: "project",
    folder_path: "/tmp/project",
    ...overrides,
  };
}

beforeEach(() => {
  vi.restoreAllMocks();
  vi.spyOn(commands, "cliSessionsProjectsList").mockResolvedValue({
    status: "ok",
    data: [makeCliSessionsProjectSummary()],
  });
  vi.spyOn(commands, "cliSessionsSessionsList").mockResolvedValue({
    status: "ok",
    data: [makeCliSessionsSessionSummary()],
  });
  vi.spyOn(commands, "cliSessionsMessagesGet").mockResolvedValue({
    status: "ok",
    data: makeCliSessionsPaginatedMessages(),
  });
  vi.spyOn(commands, "cliSessionsSessionDelete").mockResolvedValue({ status: "ok", data: [] });
  vi.spyOn(commands, "cliSessionsFolderLookupByIds").mockResolvedValue({
    status: "ok",
    data: [makeCliSessionsFolderLookupEntry()],
  });
});

describe("services/cli/cliSessions", () => {
  describe("escapeShellArg", () => {
    it("wraps normal string in single quotes (Unix)", () => {
      expect(escapeShellArg("hello")).toBe("'hello'");
    });

    it("handles empty string (Unix)", () => {
      expect(escapeShellArg("")).toBe("''");
    });

    it("escapes single quotes in string (Unix)", () => {
      expect(escapeShellArg("it's")).toBe("'it'\\''s'");
    });

    it("handles Windows platform", () => {
      const originalUA = navigator.userAgent;
      Object.defineProperty(navigator, "userAgent", {
        value: "Mozilla/5.0 (Windows NT 10.0; Win64; x64)",
        configurable: true,
      });

      expect(escapeShellArg("hello")).toBe('"hello"');
      expect(escapeShellArg("")).toBe('""');
      expect(escapeShellArg('say "hi"')).toBe('"say ""hi"""');

      Object.defineProperty(navigator, "userAgent", {
        value: originalUA,
        configurable: true,
      });
    });
  });

  describe("cliSessionsProjectsList", () => {
    it("calls generated command with correct args", async () => {
      await cliSessionsProjectsList("claude");
      expect(commands.cliSessionsProjectsList).toHaveBeenCalledWith("claude", null);
    });
  });

  describe("cliSessionsSessionsList", () => {
    it("calls generated command with correct args", async () => {
      await cliSessionsSessionsList("codex", "proj-1");
      expect(commands.cliSessionsSessionsList).toHaveBeenCalledWith("codex", "proj-1", null);
    });
  });

  describe("cliSessionsMessagesGet", () => {
    it("calls generated command with correct args", async () => {
      await cliSessionsMessagesGet({
        source: "claude",
        filePath: "/path/to/file.json",
        page: 0,
        pageSize: 50,
        fromEnd: true,
      });
      expect(commands.cliSessionsMessagesGet).toHaveBeenCalledWith(
        "claude",
        "/path/to/file.json",
        0,
        50,
        true,
        null
      );
    });
  });

  describe("cliSessionsSessionDelete", () => {
    it("calls generated command with correct args", async () => {
      await cliSessionsSessionDelete({
        source: "claude",
        filePaths: ["/f1.json", "/f2.json"],
      });
      expect(commands.cliSessionsSessionDelete).toHaveBeenCalledWith(
        "claude",
        ["/f1.json", "/f2.json"],
        null
      );
    });

    it("passes wsl_distro when provided", async () => {
      await cliSessionsSessionDelete({
        source: "codex",
        filePaths: ["/f.json"],
        wslDistro: "Ubuntu",
      });
      expect(commands.cliSessionsSessionDelete).toHaveBeenCalledWith(
        "codex",
        ["/f.json"],
        "Ubuntu"
      );
    });
  });

  describe("cliSessionsFolderLookupByIds", () => {
    it("passes generated lookup items without any-casts", async () => {
      await cliSessionsFolderLookupByIds([{ source: "claude", session_id: "s1" }], "Ubuntu");
      expect(commands.cliSessionsFolderLookupByIds).toHaveBeenCalledWith(
        [{ source: "claude", session_id: "s1" }],
        "Ubuntu"
      );
    });
  });
});
