import { describe, expect, it, vi } from "vitest";
import { createProvisionFlow } from "./provisionFlow";
import type { ProvisionApi, ProvisionedLine, ProvisionPreview } from "./provisionFlow";

const link = {
  redeemUrl: "https://api.ariaroute.com/provision/softphone/tok",
  serverHost: "api.ariaroute.com",
};

const preview: ProvisionPreview = {
  tenantName: "Regular Test",
  extension: "1001",
  displayName: "Front Desk",
  domain: "regular-test.ariaroute.com",
  expiresAt: null,
  serverHost: "api.ariaroute.com",
};

const line: ProvisionedLine = {
  accountId: "acc-1",
  updated: false,
  tenantName: "Regular Test",
  displayName: "Front Desk",
  username: "1001",
  authUsername: null,
  domain: "regular-test.ariaroute.com",
  outboundProxy: "sip.ariaroute.com:5061",
  registrar: "sip.ariaroute.com",
  port: 5061,
  transport: "tls",
  voicemail: "*97",
};

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function mockApi(overrides: Partial<ProvisionApi> = {}) {
  return {
    preview: vi.fn<ProvisionApi["preview"]>(async () => preview),
    redeem: vi.fn<ProvisionApi["redeem"]>(async () => line),
    activate: vi.fn<ProvisionApi["activate"]>(async () => null),
    ...overrides,
  };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("provision flow", () => {
  it("shows the preview, and redeems only on confirm", async () => {
    const api = mockApi();
    const flow = createProvisionFlow(api);
    expect(flow.getState().open(link)).toBe(true);
    expect(flow.getState().state.phase).toBe("loading");
    await flush();
    expect(flow.getState().state).toEqual({ phase: "preview", link, preview });
    expect(api.preview).toHaveBeenCalledWith(link.redeemUrl);
    expect(api.redeem).not.toHaveBeenCalled();

    await flow.getState().confirm();
    expect(api.redeem).toHaveBeenCalledTimes(1);
    expect(api.activate).toHaveBeenCalledWith(line);
    expect(flow.getState().state).toEqual({ phase: "done", line, error: null });
  });

  it("Cancel never redeems", async () => {
    const api = mockApi();
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    await flush();
    flow.getState().cancel();
    expect(flow.getState().state.phase).toBe("idle");
    // A stray confirm after cancel (a double-click, a late keypress) does nothing.
    await flow.getState().confirm();
    expect(api.redeem).not.toHaveBeenCalled();
    expect(api.activate).not.toHaveBeenCalled();
  });

  it("cancel while the preview loads stops a late reply reopening the dialog", async () => {
    const pending = deferred<ProvisionPreview>();
    const api = mockApi({ preview: vi.fn(() => pending.promise) });
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    flow.getState().cancel();
    pending.resolve(preview);
    await flush();
    expect(flow.getState().state.phase).toBe("idle");
    await flow.getState().confirm();
    expect(api.redeem).not.toHaveBeenCalled();
  });

  it("confirm before the preview has loaded does nothing", async () => {
    const pending = deferred<ProvisionPreview>();
    const api = mockApi({ preview: vi.fn(() => pending.promise) });
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    await flow.getState().confirm();
    expect(api.redeem).not.toHaveBeenCalled();
  });

  it("a double click redeems once", async () => {
    const api = mockApi();
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    await flush();
    await Promise.all([flow.getState().confirm(), flow.getState().confirm()]);
    expect(api.redeem).toHaveBeenCalledTimes(1);
  });

  it("a second link while one is open is ignored", async () => {
    const api = mockApi();
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    await flush();
    const other = { redeemUrl: "https://evil.example/provision/softphone/x", serverHost: "evil.example" };
    expect(flow.getState().open(other)).toBe(false);
    expect(flow.getState().state).toEqual({ phase: "preview", link, preview });
    expect(api.preview).toHaveBeenCalledTimes(1);
  });

  it("cannot be cancelled mid-redeem", async () => {
    const pending = deferred<ProvisionedLine>();
    const api = mockApi({ redeem: vi.fn(() => pending.promise) });
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    await flush();
    const done = flow.getState().confirm();
    flow.getState().cancel();
    expect(flow.getState().state.phase).toBe("redeeming");
    pending.resolve(line);
    await done;
    expect(flow.getState().state.phase).toBe("done");
  });

  it("reports a preview failure, then can open again", async () => {
    const api = mockApi({
      preview: vi.fn(async () => {
        throw "This setup link has expired or was already used. Ask for a new one.";
      }),
    });
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    await flush();
    expect(flow.getState().state).toEqual({
      phase: "failed",
      message: "This setup link has expired or was already used. Ask for a new one.",
    });
    flow.getState().cancel();
    expect(flow.getState().open(link)).toBe(true);
  });

  it("reports a redeem failure without activating", async () => {
    const api = mockApi({ redeem: vi.fn(async () => Promise.reject(new Error("HTTP 500"))) });
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    await flush();
    await flow.getState().confirm();
    expect(flow.getState().state).toEqual({ phase: "failed", message: "HTTP 500" });
    expect(api.activate).not.toHaveBeenCalled();
  });

  it("keeps the line and reports a registration error", async () => {
    const api = mockApi({ activate: vi.fn(async () => "403 Forbidden") });
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    await flush();
    await flow.getState().confirm();
    expect(flow.getState().state).toEqual({ phase: "done", line, error: "403 Forbidden" });
  });
});
