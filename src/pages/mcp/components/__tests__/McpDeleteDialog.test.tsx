import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { McpServerSummary } from "../../../../services/workspace/mcp";
import { McpDeleteDialog } from "../McpDeleteDialog";

function createTarget(overrides: Partial<McpServerSummary> = {}): McpServerSummary {
  return {
    id: 7,
    server_key: "fetch",
    name: "Fetch Tool",
    transport: "http",
    url: "https://example.com/mcp",
    enabled: true,
    command: null,
    args: [],
    env_keys: [],
    cwd: null,
    header_keys: [],
    created_at: 1,
    updated_at: 1,
    ...overrides,
  };
}

describe("pages/mcp/components/McpDeleteDialog", () => {
  it("renders delete confirmation and supports confirm plus close actions", async () => {
    const onConfirm = vi.fn();
    const onClose = vi.fn();

    render(
      <McpDeleteDialog
        target={createTarget()}
        deleting={false}
        onConfirm={onConfirm}
        onClose={onClose}
      />
    );

    expect(screen.getByRole("heading", { name: "确认删除" })).toBeInTheDocument();
    expect(
      screen.getByText("将删除「Fetch Tool」并从已启用的 CLI 配置中移除（不可恢复）。")
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "确认删除" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(onClose).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByLabelText("关闭"));
    await waitFor(() => {
      expect(onClose).toHaveBeenCalledTimes(2);
    });
  });

  it("disables actions while deleting", () => {
    const onConfirm = vi.fn();
    const onClose = vi.fn();

    render(
      <McpDeleteDialog target={createTarget()} deleting onConfirm={onConfirm} onClose={onClose} />
    );

    expect(screen.getByRole("button", { name: "删除中…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "取消" })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "删除中…" }));
    fireEvent.click(screen.getByRole("button", { name: "取消" }));

    expect(onConfirm).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("stays closed when there is no delete target", () => {
    render(
      <McpDeleteDialog target={null} deleting={false} onConfirm={vi.fn()} onClose={vi.fn()} />
    );

    expect(screen.queryByText("确认删除")).not.toBeInTheDocument();
  });
});
